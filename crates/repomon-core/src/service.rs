//! Daemon service management (launchd on macOS, systemd user units on Linux, a Task Scheduler
//! logon task on Windows).
//!
//! Lives in `repomon-core` because the `repomon daemon …` subcommands run from the TUI
//! binary and must drive install/start/stop without depending on the daemon crate. A
//! service is optional on every platform — the TUI auto-spawns `repomond` on demand —
//! so failures here surface as advice, not as a wall.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The launchd label / service identifier.
pub const LABEL: &str = "com.repomon.daemon";

/// The systemd user unit name (Linux twin of [`LABEL`]).
pub const UNIT_NAME: &str = "repomon.service";

/// Where logs are written (`<data_dir>/logs`).
pub fn log_dir() -> PathBuf {
    crate::config::data_dir().join("logs")
}

/// Generate the launchd plist XML for running `program --socket <socket>`.
pub fn generate_plist(program: &Path, socket: &Path) -> String {
    let logs = log_dir();
    let out = logs.join("repomond.out.log");
    let err = logs.join("repomond.err.log");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{program}</string>
        <string>--socket</string>
        <string>{socket}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ProcessType</key>
    <string>Background</string>
    <key>StandardOutPath</key>
    <string>{out}</string>
    <key>StandardErrorPath</key>
    <string>{err}</string>
</dict>
</plist>
"#,
        label = LABEL,
        program = program.display(),
        socket = socket.display(),
        out = out.display(),
        err = err.display(),
    )
}

/// Generate the systemd user unit for running `program --socket <socket>`. `append:` logging
/// keeps `repomon daemon logs` and the TUI log view reading the same files launchd writes on
/// macOS (needs systemd ≥ 240; `journalctl --user -u repomon` works regardless).
pub fn generate_unit(program: &Path, socket: &Path) -> String {
    let logs = log_dir();
    let out = logs.join("repomond.out.log");
    let err = logs.join("repomond.err.log");
    format!(
        r#"[Unit]
Description=repomon daemon (repomond)

[Service]
ExecStart="{program}" --socket "{socket}"
Restart=always
RestartSec=2
StandardOutput=append:{out}
StandardError=append:{err}

[Install]
WantedBy=default.target
"#,
        program = program.display(),
        socket = socket.display(),
        out = out.display(),
        err = err.display(),
    )
}

/// The operations `repomon daemon …` drives through `systemctl --user`.
pub enum ServiceOp {
    DaemonReload,
    EnableNow,
    DisableNow,
    Stop,
    Restart,
    IsActive,
}

/// The `systemctl` argv for an operation — pure, so the shapes are testable on every platform.
pub fn systemctl_user_args(op: ServiceOp) -> Vec<&'static str> {
    match op {
        ServiceOp::DaemonReload => vec!["--user", "daemon-reload"],
        ServiceOp::EnableNow => vec!["--user", "enable", "--now", UNIT_NAME],
        ServiceOp::DisableNow => vec!["--user", "disable", "--now", UNIT_NAME],
        ServiceOp::Stop => vec!["--user", "stop", UNIT_NAME],
        ServiceOp::Restart => vec!["--user", "restart", UNIT_NAME],
        ServiceOp::IsActive => vec!["--user", "is-active", UNIT_NAME],
    }
}

/// The Task Scheduler task name (Windows twin of [`LABEL`] / [`UNIT_NAME`]).
pub const TASK_NAME: &str = "RepomonDaemon";

/// The `/TR` command line for the logon task: `"program" --socket "socket"`. Both sides are
/// quoted so `C:\Program Files\…` paths and pipe names survive Task Scheduler's re-parse.
pub fn task_run_command(program: &Path, socket: &Path) -> String {
    format!(
        r#""{program}" --socket "{socket}""#,
        program = program.display(),
        socket = socket.display(),
    )
}

/// The operations `repomon daemon …` drives through `schtasks` (Windows twin of [`ServiceOp`]).
pub enum TaskOp {
    Delete,
    Run,
    End,
    Query,
}

/// The `schtasks` argv for an operation — pure, so the shapes are testable on every platform.
/// Query uses `/FO CSV /NH` because the status field's *position* is locale-independent even
/// where `/FO LIST`'s labels are not.
pub fn schtasks_args(op: TaskOp) -> Vec<&'static str> {
    match op {
        TaskOp::Delete => vec!["/Delete", "/TN", TASK_NAME, "/F"],
        TaskOp::Run => vec!["/Run", "/TN", TASK_NAME],
        TaskOp::End => vec!["/End", "/TN", TASK_NAME],
        TaskOp::Query => vec!["/Query", "/TN", TASK_NAME, "/FO", "CSV", "/NH"],
    }
}

/// The `schtasks /Create` argv registering the logon task (`/SC ONLOGON`; `/F` overwrites a
/// stale registration, the parity of the launchd bootout-before-bootstrap dance).
pub fn schtasks_create_args(task_run: &str) -> Vec<String> {
    [
        "/Create", "/TN", TASK_NAME, "/SC", "ONLOGON", "/TR", task_run, "/F",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Extract the status field ("Ready", "Running", …) from `schtasks /Query /FO CSV /NH` output:
/// the last field of the first row (`"TaskName","Next Run Time","Status"`).
pub fn parse_query_status(csv: &str) -> Option<String> {
    let row = csv.lines().find(|l| !l.trim().is_empty())?;
    let status = row.rsplit(',').next()?.trim().trim_matches('"').trim();
    if status.is_empty() {
        None
    } else {
        Some(status.to_string())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use std::process::Command;

    pub fn service_file_path() -> PathBuf {
        dirs_home()
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist"))
    }

    fn dirs_home() -> PathBuf {
        directories::BaseDirs::new()
            .map(|b| b.home_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn uid() -> Result<String> {
        let out = Command::new("id").arg("-u").output().map_err(Error::Io)?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn launchctl(args: &[&str]) -> Result<String> {
        let out = Command::new("launchctl")
            .args(args)
            .output()
            .map_err(Error::Io)?;
        if !out.status.success() {
            return Err(Error::Other(format!(
                "launchctl {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn install(program: &Path, socket: &Path) -> Result<()> {
        std::fs::create_dir_all(log_dir())?;
        let plist = service_file_path();
        if let Some(parent) = plist.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&plist, generate_plist(program, socket))?;
        let domain = format!("gui/{}", uid()?);
        // bootout first in case a stale instance is registered (ignore failure).
        let _ = launchctl(&["bootout", &domain, &plist.to_string_lossy()]);
        launchctl(&["bootstrap", &domain, &plist.to_string_lossy()])?;
        Ok(())
    }

    pub fn uninstall() -> Result<()> {
        let plist = service_file_path();
        let domain = format!("gui/{}", uid()?);
        let _ = launchctl(&["bootout", &domain, &plist.to_string_lossy()]);
        if plist.exists() {
            std::fs::remove_file(&plist)?;
        }
        Ok(())
    }

    pub fn start() -> Result<()> {
        let target = format!("gui/{}/{LABEL}", uid()?);
        launchctl(&["kickstart", "-k", &target]).map(|_| ())
    }

    pub fn stop() -> Result<()> {
        let target = format!("gui/{}/{LABEL}", uid()?);
        launchctl(&["kill", "TERM", &target]).map(|_| ())
    }

    pub fn status() -> Result<String> {
        match launchctl(&["list", LABEL]) {
            Ok(out) => Ok(out),
            Err(_) => Ok("not installed".to_string()),
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::process::Command;

    /// `$XDG_CONFIG_HOME/systemd/user/repomon.service` (`~/.config/…` by default).
    pub fn service_file_path() -> PathBuf {
        let base = match std::env::var("XDG_CONFIG_HOME") {
            Ok(x) if !x.is_empty() => PathBuf::from(x),
            _ => dirs_home().join(".config"),
        };
        base.join("systemd").join("user").join(UNIT_NAME)
    }

    fn dirs_home() -> PathBuf {
        directories::BaseDirs::new()
            .map(|b| b.home_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// `Err(reason)` when systemd user services can't work here. The daemon still runs without
    /// one — the TUI auto-spawns `repomond` — so callers surface this as advice, not a wall.
    fn systemd_available() -> std::result::Result<(), String> {
        const HINT: &str = "the TUI auto-starts repomond, so a service is optional";
        // The documented probe for "is systemd PID 1 here" — absent in containers and on
        // non-systemd inits.
        if !Path::new("/run/systemd/system").exists() {
            return Err(format!(
                "systemd is not managing this system (e.g. a container); {HINT}"
            ));
        }
        match Command::new("systemctl")
            .args(["--user", "is-system-running"])
            .output()
        {
            Err(_) => Err(format!("systemctl not found; {HINT}")),
            Ok(out) => {
                let state = String::from_utf8_lossy(&out.stdout).trim().to_string();
                // Any live user manager will do; "offline" or a DBus connection error mean
                // there is no user session for `--user` units (common over bare SSH).
                if matches!(
                    state.as_str(),
                    "running" | "degraded" | "starting" | "initializing" | "maintenance"
                ) {
                    Ok(())
                } else {
                    let state = if state.is_empty() {
                        "unreachable".to_string()
                    } else {
                        state
                    };
                    Err(format!(
                        "no systemd user session (state: {state}); try `loginctl enable-linger $USER`; {HINT}"
                    ))
                }
            }
        }
    }

    fn systemctl(op: ServiceOp) -> Result<String> {
        let args = systemctl_user_args(op);
        let out = Command::new("systemctl")
            .args(&args)
            .output()
            .map_err(Error::Io)?;
        if !out.status.success() {
            return Err(Error::Other(format!(
                "systemctl {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn install(program: &Path, socket: &Path) -> Result<()> {
        systemd_available().map_err(Error::Other)?;
        std::fs::create_dir_all(log_dir())?;
        let unit = service_file_path();
        if let Some(parent) = unit.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&unit, generate_unit(program, socket))?;
        systemctl(ServiceOp::DaemonReload)?;
        systemctl(ServiceOp::EnableNow)?;
        Ok(())
    }

    pub fn uninstall() -> Result<()> {
        let _ = systemctl(ServiceOp::DisableNow);
        let unit = service_file_path();
        if unit.exists() {
            std::fs::remove_file(&unit)?;
        }
        let _ = systemctl(ServiceOp::DaemonReload);
        Ok(())
    }

    pub fn start() -> Result<()> {
        // restart = start-or-restart, the parity of `launchctl kickstart -k`.
        systemctl(ServiceOp::Restart).map(|_| ())
    }

    pub fn stop() -> Result<()> {
        systemctl(ServiceOp::Stop).map(|_| ())
    }

    pub fn status() -> Result<String> {
        if !service_file_path().exists() {
            return Ok("not installed".to_string());
        }
        // `is-active` exits non-zero for inactive/failed, but stdout still carries the state.
        let out = Command::new("systemctl")
            .args(systemctl_user_args(ServiceOp::IsActive))
            .output()
            .map_err(Error::Io)?;
        let state = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(if state.is_empty() {
            "unknown".to_string()
        } else {
            state
        })
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use crate::process::background_command;

    /// The task's path in the Task Scheduler library — no file on disk, but callers print it
    /// after install the way the Unix arms print the plist/unit path.
    pub fn service_file_path() -> PathBuf {
        PathBuf::from(format!(r"\{TASK_NAME}"))
    }

    fn schtasks(args: &[&str]) -> Result<String> {
        let out = background_command("schtasks")
            .args(args)
            .output()
            .map_err(Error::Io)?;
        if !out.status.success() {
            return Err(Error::Other(format!(
                "schtasks {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn schtasks_op(op: TaskOp) -> Result<String> {
        schtasks(&schtasks_args(op))
    }

    pub fn install(program: &Path, socket: &Path) -> Result<()> {
        std::fs::create_dir_all(log_dir())?;
        let run = task_run_command(program, socket);
        let args = schtasks_create_args(&run);
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        schtasks(&argv)?;
        // Start it now too — parity with launchd bootstrap and `systemctl enable --now`.
        schtasks_op(TaskOp::Run)?;
        Ok(())
    }

    pub fn uninstall() -> Result<()> {
        if schtasks_op(TaskOp::Query).is_err() {
            // Not installed — nothing to do, like the Unix arms.
            return Ok(());
        }
        let _ = schtasks_op(TaskOp::End);
        schtasks_op(TaskOp::Delete)?;
        Ok(())
    }

    pub fn start() -> Result<()> {
        // End-then-run = start-or-restart, the parity of `launchctl kickstart -k`.
        let _ = schtasks_op(TaskOp::End);
        schtasks_op(TaskOp::Run).map(|_| ())
    }

    pub fn stop() -> Result<()> {
        schtasks_op(TaskOp::End).map(|_| ())
    }

    pub fn status() -> Result<String> {
        match schtasks_op(TaskOp::Query) {
            Err(_) => Ok("not installed".to_string()),
            Ok(out) => Ok(parse_query_status(&out).unwrap_or_else(|| "unknown".to_string())),
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod platform {
    use super::*;

    fn unsupported() -> Error {
        Error::Other(
            "service management is supported on macOS (launchd), Linux (systemd user units), and Windows (Task Scheduler)"
                .into(),
        )
    }

    pub fn service_file_path() -> PathBuf {
        PathBuf::new()
    }
    pub fn install(_program: &Path, _socket: &Path) -> Result<()> {
        Err(unsupported())
    }
    pub fn uninstall() -> Result<()> {
        Err(unsupported())
    }
    pub fn start() -> Result<()> {
        Err(unsupported())
    }
    pub fn stop() -> Result<()> {
        Err(unsupported())
    }
    pub fn status() -> Result<String> {
        Err(unsupported())
    }
}

pub use platform::{install, service_file_path, start, status, stop, uninstall};

/// Path to the daemon log file (stdout).
pub fn log_file() -> PathBuf {
    log_dir().join("repomond.out.log")
}

/// Best-effort guess at the `repomond` binary path (sibling of the current exe).
pub fn repomond_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // EXE_SUFFIX is ".exe" on Windows and "" on Unix.
            let cand = dir.join(format!("repomond{}", std::env::consts::EXE_SUFFIX));
            if cand.exists() {
                return cand;
            }
        }
    }
    PathBuf::from("repomond")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_contains_program_and_socket() {
        let xml = generate_plist(
            Path::new("/usr/local/bin/repomond"),
            Path::new("/tmp/r.sock"),
        );
        assert!(xml.contains("com.repomon.daemon"));
        assert!(xml.contains("/usr/local/bin/repomond"));
        assert!(xml.contains("/tmp/r.sock"));
        assert!(xml.contains("<key>RunAtLoad</key>"));
    }

    #[test]
    fn unit_contains_exec_socket_and_logs() {
        let unit = generate_unit(
            Path::new("/usr/local/bin/repomond"),
            Path::new("/tmp/r.sock"),
        );
        assert!(unit.contains(r#"ExecStart="/usr/local/bin/repomond" --socket "/tmp/r.sock""#));
        assert!(unit.contains("Restart=always"));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(unit.contains("StandardOutput=append:"));
        assert!(unit.contains("repomond.err.log"));
    }

    #[test]
    fn systemctl_argv_shapes() {
        assert_eq!(
            systemctl_user_args(ServiceOp::EnableNow),
            ["--user", "enable", "--now", "repomon.service"]
        );
        assert_eq!(
            systemctl_user_args(ServiceOp::DaemonReload),
            ["--user", "daemon-reload"]
        );
        assert_eq!(
            systemctl_user_args(ServiceOp::IsActive),
            ["--user", "is-active", "repomon.service"]
        );
        assert_eq!(
            systemctl_user_args(ServiceOp::Stop),
            ["--user", "stop", "repomon.service"]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn service_file_is_the_user_unit() {
        assert!(service_file_path().ends_with("systemd/user/repomon.service"));
    }

    #[test]
    fn schtasks_create_argv_is_a_logon_task() {
        let run = task_run_command(
            Path::new(r"C:\Users\u\.cargo\bin\repomond.exe"),
            Path::new(r"\\.\pipe\repomon-u"),
        );
        assert_eq!(
            run,
            r#""C:\Users\u\.cargo\bin\repomond.exe" --socket "\\.\pipe\repomon-u""#
        );
        let args = schtasks_create_args(&run);
        assert_eq!(
            args,
            [
                "/Create",
                "/TN",
                "RepomonDaemon",
                "/SC",
                "ONLOGON",
                "/TR",
                run.as_str(),
                "/F"
            ]
        );
    }

    #[test]
    fn schtasks_argv_shapes() {
        assert_eq!(
            schtasks_args(TaskOp::Delete),
            ["/Delete", "/TN", "RepomonDaemon", "/F"]
        );
        assert_eq!(schtasks_args(TaskOp::Run), ["/Run", "/TN", "RepomonDaemon"]);
        assert_eq!(schtasks_args(TaskOp::End), ["/End", "/TN", "RepomonDaemon"]);
        assert_eq!(
            schtasks_args(TaskOp::Query),
            ["/Query", "/TN", "RepomonDaemon", "/FO", "CSV", "/NH"]
        );
    }

    #[test]
    fn query_status_parses_csv_row() {
        let csv = "\"RepomonDaemon\",\"7/19/2026 9:00:00 AM\",\"Ready\"\r\n";
        assert_eq!(parse_query_status(csv).as_deref(), Some("Ready"));
        assert_eq!(parse_query_status(""), None);
        assert_eq!(parse_query_status("\r\n"), None);
    }
}
