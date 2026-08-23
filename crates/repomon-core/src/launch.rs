//! Daemon discovery, launch, and startup retry shared by local UI clients.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};

#[cfg(windows)]
use crate::process::{WINDOWS_CREATE_NEW_PROCESS_GROUP, WINDOWS_CREATE_NO_WINDOW};
use crate::{Config, client::DaemonClient, config, service};

/// Default connection timeout for starting/reconnecting to a newly launched daemon process (25s).
/// Long enough to ride out cold VM boot, slow disk I/O, initial SQLite migrations, and Defender
/// on-access scanning overhead, while keeping the fast path instantaneous via tiered backoff.
pub const DEFAULT_DAEMON_CONNECT_TIMEOUT: Duration = Duration::from_secs(25);

/// Connect to a running daemon, or start a detached `repomond` and connect to that.
///
/// Returns `Err` if no daemon is running and `repomond` cannot be launched. A caller that embeds
/// the daemon may use the error as its signal to start the in-process fallback.
pub async fn ensure_daemon(
    config: &Config,
    socket_override: Option<PathBuf>,
) -> Result<DaemonClient> {
    let socket = socket_override.unwrap_or_else(|| config::socket_path(config));
    if let Ok(client) = DaemonClient::connect(&socket).await {
        return Ok(client);
    }
    spawn_daemon(&socket)?;
    // A first start may run SQLite migrations or require AV scan passes before binding the socket.
    connect_with_backoff(&socket, DEFAULT_DAEMON_CONNECT_TIMEOUT).await
}

/// Launch `repomond` as a detached background process and send its output to the daemon log.
pub fn spawn_daemon(socket: &Path) -> Result<()> {
    use std::process::{Command, Stdio};

    let program = service::repomond_path();
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
        .with_context(|| format!("starting daemon `{}`", program.display()))?;
    Ok(())
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
