//! Daemon discovery, launch, and startup retry shared by local UI clients.

use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};

#[cfg(windows)]
use crate::process::{WINDOWS_CREATE_NEW_PROCESS_GROUP, WINDOWS_CREATE_NO_WINDOW};
use crate::{Config, client::DaemonClient, config, service};

/// Default connection timeout for starting/reconnecting to a newly launched daemon process (25s).
/// Long enough to ride out cold VM boot, slow disk I/O, initial SQLite migrations, and Defender
/// on-access scanning overhead, while keeping the fast path instantaneous via tiered backoff.
pub const DEFAULT_DAEMON_CONNECT_TIMEOUT: Duration = Duration::from_secs(25);

/// How long a freshly spawned daemon is watched for an immediate exit before the caller falls
/// back to ordinary connect backoff. A daemon that dies inside this window (a missing runtime
/// DLL, a corrupt database, a port/pipe already owned) is the failure the connection pill used to
/// render as an endless "Retrying" with nothing to act on.
pub const BOOT_WATCH_WINDOW: Duration = Duration::from_secs(3);

/// How many trailing lines of the daemon log a boot failure carries to the UI.
pub const LOG_TAIL_LINES: usize = 20;

/// At most this many bytes are read off the end of the daemon log to build a tail. The log is
/// append-only across every run, so it can be arbitrarily large.
const LOG_TAIL_MAX_BYTES: u64 = 64 * 1024;

/// The x64 Visual C++ redistributable. Offered as the fix when a Windows daemon dies with
/// `STATUS_DLL_NOT_FOUND` because it was built against a dynamic CRT (pre-0.8.2 bundles; newer
/// ones link the CRT statically, see `.cargo/config.toml`).
pub const VC_REDIST_URL: &str = "https://aka.ms/vs/17/release/vc_redist.x64.exe";

/// Why a daemon could not be started or reached, in a shape a UI can act on.
///
/// The desktop app spawns `repomond` detached and windowless, so a spawn that fails or a daemon
/// that exits at once produces no console, no dialog, and no clue: the connection pill just says
/// "Retrying" forever. Each variant carries what the user needs to get unstuck, and [`Self::hint`]
/// turns the two failures nobody can be expected to diagnose into one actionable line.
#[derive(Debug, thiserror::Error)]
pub enum DaemonLaunchError {
    /// The resolved `repomond` path does not exist. A broken or partial install.
    #[error("no daemon binary at {path}")]
    DaemonMissing { path: PathBuf },

    /// The OS refused to create the process at all.
    #[error("could not start the daemon at {path}: {source}")]
    SpawnFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The process started and then exited before it bound its endpoint.
    #[error("the daemon started and exited immediately ({code})")]
    DaemonExited {
        code: String,
        log_tail: String,
        log_path: PathBuf,
    },

    /// The process is alive but nothing answers on the endpoint within the connect timeout.
    /// `detail` is already a full, actionable sentence (see [`connect_with_backoff`]).
    #[error("{detail}")]
    NotReachable {
        endpoint: String,
        detail: String,
        log_path: PathBuf,
    },
}

impl DaemonLaunchError {
    /// One actionable line for the failures a user cannot reasonably diagnose, or `None` when the
    /// error message already says everything there is to say.
    pub fn hint(&self) -> Option<String> {
        match self {
            Self::DaemonMissing { path } => Some(format!(
                "Reinstall Repomon: the daemon should sit next to the app at {}.",
                path.display()
            )),
            Self::SpawnFailed { source, .. } => launch_hint(&source.to_string()),
            Self::DaemonExited { code, log_tail, .. } => launch_hint(&format!("{code}\n{log_tail}")),
            Self::NotReachable {
                endpoint, detail, ..
            } => connect_hint(endpoint, detail),
        }
    }

    /// The daemon log to offer as "Show log", when this failure has one.
    pub fn log_path(&self) -> Option<&Path> {
        match self {
            Self::DaemonExited { log_path, .. } | Self::NotReachable { log_path, .. } => {
                Some(log_path.as_path())
            }
            _ => None,
        }
    }

    /// The captured tail of the daemon log, when this failure carries one.
    pub fn log_tail(&self) -> Option<&str> {
        match self {
            Self::DaemonExited { log_tail, .. } => Some(log_tail.as_str()),
            _ => None,
        }
    }
}

/// Recognise a launch failure whose cause is invisible from the message alone, and return the fix.
///
/// Pure string matching on purpose: the two shapes below only ever occur on Windows, and a
/// diagnostic nobody can run on the other two platforms is a diagnostic nobody maintains.
pub fn launch_hint(detail: &str) -> Option<String> {
    let lowered = detail.to_lowercase();
    // 0xC0000135 STATUS_DLL_NOT_FOUND: a dependency of the image is not on the loader's search
    // path. The operator's VM hit exactly this: the launcher's own retry lines were the only
    // thing in repomond.out.log, because the child died in the loader before any Rust ran.
    let dll_not_found = lowered.contains("vcruntime140")
        || lowered.contains("msvcp140")
        || lowered.contains("0xc0000135")
        || lowered.contains("c0000135")
        || lowered.contains("3221225781")
        || lowered.contains("status_dll_not_found");
    // 0xC000007B STATUS_INVALID_IMAGE_FORMAT: the loader found a dependency but of the wrong
    // architecture. The same redistributable fixes it (an x86 runtime against an x64 daemon).
    let bad_image = lowered.contains("0xc000007b")
        || lowered.contains("c000007b")
        || lowered.contains("3221225595")
        || lowered.contains("status_invalid_image_format");
    if dll_not_found || bad_image {
        return Some(format!(
            "The Visual C++ runtime is missing or is the wrong architecture, so the daemon dies before it starts. Install the x64 redistributable from {VC_REDIST_URL}, then start the daemon again."
        ));
    }
    None
}

/// Recognise a connect failure against a Windows named pipe that a *different* process already
/// owns, and return the fix. Returns `None` for a unix socket endpoint, and for the ordinary
/// "nothing is listening yet" shapes, which are not a user's problem to solve.
pub fn connect_hint(endpoint: &str, detail: &str) -> Option<String> {
    if !endpoint.to_lowercase().starts_with(r"\\.\pipe\") {
        return None;
    }
    let lowered = detail.to_lowercase();
    let owned_elsewhere = lowered.contains("access is denied")
        || lowered.contains("os error 5")
        || lowered.contains("all pipe instances are busy")
        || lowered.contains("os error 231");
    if owned_elsewhere {
        return Some(format!(
            "{endpoint} is already owned by another session or a stale daemon. End repomond.exe on the Details tab of Task Manager, then start the daemon again."
        ));
    }
    None
}

/// The last `lines` lines of `path`, or an empty string when there is nothing to read. Only the
/// trailing [`LOG_TAIL_MAX_BYTES`] are touched, so an old install's multi-megabyte log costs the
/// same as a fresh one's.
pub fn log_tail(path: &Path, lines: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(LOG_TAIL_MAX_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let tail: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(lines)
        .collect();
    tail.into_iter().rev().collect::<Vec<_>>().join("\n")
}

/// A human-readable label for how a child process ended. Windows status codes are shown in hex
/// as well, because that is the form every search result and every Microsoft page uses.
pub fn exit_code_label(status: &ExitStatus) -> String {
    match status.code() {
        Some(code) => {
            let unsigned = code as u32;
            if unsigned > 0xFFFF {
                format!("exit code {code} / 0x{unsigned:08X}")
            } else {
                format!("exit code {code}")
            }
        }
        None => format!("terminated without an exit code: {status}"),
    }
}

/// The endpoint as the platform actually names it: the socket path on unix, the pipe name on
/// Windows. What the user has to look for in Task Manager or `Get-ChildItem \\.\pipe\`.
pub fn endpoint_label(socket: &Path) -> String {
    #[cfg(windows)]
    {
        crate::transport::pipe_name_from_path(socket)
    }
    #[cfg(not(windows))]
    {
        socket.display().to_string()
    }
}

/// Connect to a running daemon, or start a detached `repomond` and connect to that.
///
/// Returns `Err` if no daemon is running and `repomond` cannot be launched. A caller that embeds
/// the daemon may use the error as its signal to start the in-process fallback. The error is
/// typed rather than opaque so a GUI can render the cause and offer the fix; `?` still converts
/// it into an `anyhow::Error` for the CLI callers that only print it.
pub async fn ensure_daemon(
    config: &Config,
    socket_override: Option<PathBuf>,
) -> std::result::Result<DaemonClient, DaemonLaunchError> {
    let socket = socket_override.unwrap_or_else(|| config::socket_path(config));
    if let Ok(client) = DaemonClient::connect(&socket).await {
        return Ok(client);
    }
    spawn_and_watch_boot(&socket, BOOT_WATCH_WINDOW).await?;
    // A first start may run SQLite migrations or require AV scan passes before binding the socket.
    connect_with_backoff(&socket, DEFAULT_DAEMON_CONNECT_TIMEOUT)
        .await
        .map_err(|error| DaemonLaunchError::NotReachable {
            endpoint: endpoint_label(&socket),
            detail: error.to_string(),
            log_path: service::log_file(),
        })
}

/// Spawn the daemon and watch it for `window`: return as soon as the endpoint answers, and fail
/// the moment the child exits instead of leaving the caller to time out against a dead process.
///
/// Staying alive past `window` without binding yet is *not* an error. Cold VM boots, first-run
/// SQLite migrations, and Defender scans all push the first successful connect well past three
/// seconds, so the caller's own backoff takes it from here.
pub async fn spawn_and_watch_boot(
    socket: &Path,
    window: Duration,
) -> std::result::Result<(), DaemonLaunchError> {
    let mut child = spawn_daemon(socket)?;
    let endpoint = crate::transport::Endpoint::from_path(socket);
    let deadline = Instant::now() + window;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let log_path = service::log_file();
                let code = exit_code_label(&status);
                append_daemon_log(&format!(
                    "daemon exited immediately after launch ({code}); see the tail above"
                ));
                return Err(DaemonLaunchError::DaemonExited {
                    code,
                    log_tail: log_tail(&log_path, LOG_TAIL_LINES),
                    log_path,
                });
            }
            // Still running, or the child can no longer be observed (already reaped elsewhere).
            // Either way the endpoint probe below is the authority on whether it came up.
            Ok(None) | Err(_) => {}
        }
        if crate::transport::connect(&endpoint).await.is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Launch `repomond` as a detached background process and send its output to the daemon log.
///
/// The returned [`Child`] is only for observing an immediate exit (see [`spawn_and_watch_boot`]);
/// dropping it does not stop the daemon, which is the point of launching it detached.
pub fn spawn_daemon(socket: &Path) -> std::result::Result<Child, DaemonLaunchError> {
    use std::process::{Command, Stdio};

    let program = service::repomond_path();
    // `repomond_path` falls back to the bare name `repomond`, which is still resolvable through
    // PATH; only a resolved absolute path that is not there means a broken install.
    if program.is_absolute() && !program.exists() {
        return Err(DaemonLaunchError::DaemonMissing { path: program });
    }
    let _ = std::fs::create_dir_all(service::log_dir());
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(service::log_file())
        .ok();

    let mut cmd = Command::new(&program);
    cmd.arg("--socket").arg(socket).stdin(Stdio::null());
    match log {
        Some(out) => {
            let err = out.try_clone().ok();
            cmd.stdout(Stdio::from(out));
            if let Some(err) = err {
                cmd.stderr(Stdio::from(err));
            }
        }
        None => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }

    // Detach from the launching process group so the daemon survives its UI closing.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    // Windows twin: start without a console window and in a new Ctrl-C group so the daemon
    // survives its UI closing without flashing a console during launch.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(WINDOWS_CREATE_NO_WINDOW | WINDOWS_CREATE_NEW_PROCESS_GROUP);
    }

    cmd.spawn()
        .map_err(|source| DaemonLaunchError::SpawnFailed {
            path: program,
            source,
        })
}

fn append_daemon_log(msg: &str) {
    let _ = std::fs::create_dir_all(service::log_dir());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(service::log_file())
    {
        use std::io::Write;
        let ts = chrono::Utc::now().to_rfc3339();
        let _ = writeln!(f, "[{ts}] [launch] {msg}");
    }
}

/// Connect to the daemon, retrying with tiered backoff up to `timeout`.
///
/// Tiered schedule:
/// - Tier 1 (0s - 2s): 40ms interval (fast path for warm daemon/local launch)
/// - Tier 2 (2s - 6s): 100ms interval
/// - Tier 3 (6s - 12s): 250ms interval (cold VM start, SQLite schema migrations)
/// - Tier 4 (12s+): 500ms interval (heavy load, VM disk I/O, Defender scanning)
pub async fn connect_with_backoff(socket: &Path, timeout: Duration) -> Result<DaemonClient> {
    let start = Instant::now();
    let mut attempt = 0usize;
    let mut current_tier = 1u8;

    let last_error = loop {
        attempt += 1;
        match DaemonClient::connect(socket).await {
            Ok(client) => {
                let elapsed = start.elapsed();
                if attempt > 1 || elapsed > Duration::from_millis(200) {
                    let msg = format!(
                        "connected to daemon at {} in {:.2}s (attempt {})",
                        socket.display(),
                        elapsed.as_secs_f64(),
                        attempt
                    );
                    append_daemon_log(&msg);
                    tracing::info!(
                        socket = %socket.display(),
                        elapsed_secs = elapsed.as_secs_f64(),
                        attempts = attempt,
                        "connected to daemon"
                    );
                }
                return Ok(client);
            }
            Err(error) => {
                let elapsed = start.elapsed();
                let is_timeout = elapsed >= timeout;

                // Determine tier and delay based on elapsed time
                let (tier, delay) = if elapsed < Duration::from_secs(2) {
                    (1, Duration::from_millis(40))
                } else if elapsed < Duration::from_secs(6) {
                    (2, Duration::from_millis(100))
                } else if elapsed < Duration::from_secs(12) {
                    (3, Duration::from_millis(250))
                } else {
                    (4, Duration::from_millis(500))
                };

                if !is_timeout && tier > current_tier {
                    current_tier = tier;
                    let msg = format!(
                        "daemon at {} not ready after {:.1}s (attempt {}, last error: {error}); entering tier {} retry (delay: {:?})",
                        socket.display(),
                        elapsed.as_secs_f64(),
                        attempt,
                        tier,
                        delay
                    );
                    append_daemon_log(&msg);
                    tracing::warn!(
                        socket = %socket.display(),
                        elapsed_secs = elapsed.as_secs_f64(),
                        attempts = attempt,
                        tier = tier,
                        last_error = %error,
                        "daemon connection retry tier transitioned"
                    );
                }

                if is_timeout {
                    break error;
                }
                tokio::time::sleep(delay).await;
            }
        }
    };

    let elapsed = start.elapsed();
    let log_path = service::log_file();
    let err_detail = last_error.to_string();

    let fail_log = format!(
        "failed to connect to daemon at {} after {:.2}s ({} attempts); last error: {err_detail}",
        socket.display(),
        elapsed.as_secs_f64(),
        attempt
    );
    append_daemon_log(&fail_log);
    tracing::error!(
        socket = %socket.display(),
        elapsed_secs = elapsed.as_secs_f64(),
        attempts = attempt,
        last_error = %err_detail,
        "failed to connect to daemon"
    );

    Err(anyhow!(
        "could not connect to daemon at {} after {:.1}s ({} attempts): {}\n\
         Check the daemon log at {} or run `repomond --socket {}` manually to inspect startup errors.",
        socket.display(),
        elapsed.as_secs_f64(),
        attempt,
        err_detail,
        log_path.display(),
        socket.display()
    ))
}

/// Connect to the daemon, retrying a specified number of times with 40ms interval.
///
/// Kept for callers / test harnesses that specify a discrete try count; callers wishing for
/// cold-start / VM resilience should prefer [`connect_with_backoff`].
pub async fn connect_with_retry(socket: &Path, tries: usize) -> Result<DaemonClient> {
    let start = Instant::now();
    let mut last = None;
    for attempt in 1..=tries {
        match DaemonClient::connect(socket).await {
            Ok(client) => return Ok(client),
            Err(error) => {
                last = Some(error);
                if attempt < tries {
                    tokio::time::sleep(Duration::from_millis(40)).await;
                }
            }
        }
    }

    let elapsed = start.elapsed();
    let log_path = service::log_file();
    let err_detail = last
        .map(|e| e.to_string())
        .unwrap_or_else(|| "no response from daemon endpoint".to_string());

    Err(anyhow!(
        "could not connect to daemon at {} after {:.1}s ({} attempts): {}\n\
         Check the daemon log at {} or run `repomond --socket {}` manually.",
        socket.display(),
        elapsed.as_secs_f64(),
        tries,
        err_detail,
        log_path.display(),
        socket.display()
    ))
}

// Platform-independent: every helper below is pure string/file logic, so the Windows failure
// shapes are covered on macOS and Linux CI too.
#[cfg(test)]
mod diagnostics_tests {
    use std::path::{Path, PathBuf};

    use super::{
        DaemonLaunchError, LOG_TAIL_LINES, VC_REDIST_URL, connect_hint, launch_hint, log_tail,
    };

    const PIPE: &str = r"\\.\pipe\repomon-azaleas";

    #[test]
    fn missing_vc_runtime_is_named_from_a_spawn_error() {
        let hint = launch_hint(
            "The code execution cannot proceed because VCRUNTIME140.dll was not found.",
        )
        .expect("a VCRUNTIME140 failure should be recognised");
        assert!(hint.contains(VC_REDIST_URL));
    }

    #[test]
    fn missing_vc_runtime_is_named_from_an_exit_status() {
        for detail in [
            "exit code -1073741515 / 0xC0000135",
            "STATUS_DLL_NOT_FOUND",
            "3221225781",
            "msvcp140.dll is missing",
            "exit code -1073741701 / 0xC000007B",
            "STATUS_INVALID_IMAGE_FORMAT",
        ] {
            assert!(
                launch_hint(detail).is_some(),
                "expected a hint for {detail:?}"
            );
        }
    }

    #[test]
    fn an_ordinary_failure_gets_no_invented_hint() {
        assert!(launch_hint("No such file or directory (os error 2)").is_none());
        assert!(launch_hint("").is_none());
    }

    #[test]
    fn a_pipe_owned_elsewhere_points_at_the_stale_daemon() {
        for detail in [
            "Access is denied. (os error 5)",
            "All pipe instances are busy. (os error 231)",
        ] {
            let hint = connect_hint(PIPE, detail).expect("expected a stale-pipe hint");
            assert!(hint.contains(PIPE));
            assert!(hint.contains("repomond.exe"));
        }
    }

    #[test]
    fn a_pipe_that_is_merely_not_up_yet_gets_no_hint() {
        assert!(connect_hint(PIPE, "The system cannot find the file specified. (os error 2)").is_none());
    }

    #[test]
    fn a_unix_socket_endpoint_never_gets_a_pipe_hint() {
        assert!(connect_hint("/tmp/repomon-test.sock", "Access is denied. (os error 5)").is_none());
    }

    #[test]
    fn log_tail_reads_the_last_lines_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repomond.out.log");
        let body: String = (1..=60).map(|n| format!("line {n}\n")).collect();
        std::fs::write(&path, body).unwrap();

        let tail = log_tail(&path, LOG_TAIL_LINES);
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines.len(), LOG_TAIL_LINES);
        assert_eq!(lines.first().copied(), Some("line 41"));
        assert_eq!(lines.last().copied(), Some("line 60"));
    }

    #[test]
    fn log_tail_of_a_missing_file_is_empty() {
        assert_eq!(log_tail(Path::new("/nonexistent/repomond.out.log"), 5), "");
    }

    #[test]
    fn a_missing_binary_error_names_the_path_and_offers_a_reinstall() {
        let error = DaemonLaunchError::DaemonMissing {
            path: PathBuf::from("/Applications/Repomon.app/Contents/MacOS/repomond"),
        };
        assert!(error.to_string().contains("repomond"));
        assert!(error.hint().unwrap().contains("Reinstall Repomon"));
        assert!(error.log_path().is_none());
    }

    #[test]
    fn an_immediate_exit_carries_the_log_tail_and_its_path() {
        let error = DaemonLaunchError::DaemonExited {
            code: "exit code -1073741515 / 0xC0000135".into(),
            log_tail: "[launch] starting".into(),
            log_path: PathBuf::from("/tmp/logs/repomond.out.log"),
        };
        assert!(error.to_string().contains("exited immediately"));
        assert_eq!(error.log_tail(), Some("[launch] starting"));
        assert_eq!(
            error.log_path(),
            Some(Path::new("/tmp/logs/repomond.out.log"))
        );
        assert!(error.hint().unwrap().contains(VC_REDIST_URL));
    }

    #[test]
    fn an_unreachable_endpoint_keeps_its_own_actionable_sentence() {
        let error = DaemonLaunchError::NotReachable {
            endpoint: PIPE.into(),
            detail: "could not connect to daemon at ... : Access is denied. (os error 5)".into(),
            log_path: PathBuf::from("/tmp/logs/repomond.out.log"),
        };
        assert!(error.to_string().starts_with("could not connect to daemon"));
        assert!(error.hint().unwrap().contains("Task Manager"));
    }

    #[test]
    fn a_launch_error_converts_into_anyhow_for_cli_callers() {
        let error: anyhow::Error = DaemonLaunchError::DaemonMissing {
            path: PathBuf::from("/nowhere/repomond"),
        }
        .into();
        assert!(error.to_string().contains("/nowhere/repomond"));
    }
}

// Unix-only: the delayed server binds a real Unix socket (Windows named-pipe servers need a
// different setup and connect_with_retry is transport-agnostic either way).
#[cfg(all(test, unix))]
mod tests {
    use std::time::Duration;

    use tokio::net::UnixListener;

    use super::{connect_with_backoff, connect_with_retry};

    #[tokio::test]
    async fn connects_after_startup_gap() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("delayed.sock");
        let server_socket = socket.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            let listener = UnixListener::bind(server_socket).unwrap();
            let _ = listener.accept().await;
        });

        let client = connect_with_retry(&socket, 10).await.unwrap();
        drop(client);
    }

    #[tokio::test]
    async fn backoff_connects_after_startup_gap() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("delayed_backoff.sock");
        let server_socket = socket.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            let listener = UnixListener::bind(server_socket).unwrap();
            let _ = listener.accept().await;
        });

        let client = connect_with_backoff(&socket, Duration::from_secs(2))
            .await
            .unwrap();
        drop(client);
    }

    #[tokio::test]
    async fn backoff_timeout_surfaces_actionable_error() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("nonexistent.sock");

        let err = match connect_with_backoff(&socket, Duration::from_millis(150)).await {
            Ok(_) => panic!("expected connect_with_backoff to fail on nonexistent socket"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(msg.contains("could not connect to daemon at"));
        assert!(msg.contains("nonexistent.sock"));
        assert!(msg.contains("Check the daemon log at"));
    }
}
