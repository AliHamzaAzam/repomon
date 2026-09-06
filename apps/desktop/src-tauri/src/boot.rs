//! Probes detached daemon startup and exposes logs and diagnostics without requiring a console.

use std::path::PathBuf;
use std::time::Duration;

use repomon_core::launch;
use repomon_core::service;
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::state::AppState;

/// Bounds the daemon version probe so a hung binary cannot stall diagnostics.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Reports whether the resolved daemon binary launches, with its version, log path, and failure
/// detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DaemonBootCheck {
    /// The binary ran and printed a version.
    pub ok: bool,
    /// The resolved `repomond` path, shown so a user can see which copy was probed.
    pub path: String,
    /// `repomond --version` output, trimmed.
    pub version: Option<String>,
    /// The exact failure, when there is one.
    pub error: Option<String>,
    /// One actionable line for the failures nobody can diagnose from the error alone.
    pub hint: Option<String>,
    /// Where the daemon writes stdout and stderr.
    pub log_path: String,
}

/// Runs the resolved daemon’s version command and reports startup success or failure.
#[tauri::command]
pub async fn daemon_boot_check() -> DaemonBootCheck {
    tauri::async_runtime::spawn_blocking(probe_daemon_binary)
        .await
        .unwrap_or_else(|error| DaemonBootCheck {
            ok: false,
            path: service::repomond_path().display().to_string(),
            version: None,
            error: Some(format!("the version probe did not finish: {error}")),
            hint: None,
            log_path: service::log_file().display().to_string(),
        })
}

fn probe_daemon_binary() -> DaemonBootCheck {
    let program = service::repomond_path();
    let log_path = service::log_file().display().to_string();
    let path = program.display().to_string();

    if program.is_absolute() && !program.exists() {
        let error = launch::DaemonLaunchError::DaemonMissing {
            path: program.clone(),
        };
        return DaemonBootCheck {
            ok: false,
            path,
            version: None,
            hint: error.hint(),
            error: Some(error.to_string()),
            log_path,
        };
    }

    let mut command = repomon_core::process::background_command(&program);
    command.arg("--version");
    let child = match command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(source) => {
            let error = launch::DaemonLaunchError::SpawnFailed {
                path: program.clone(),
                source,
            };
            return DaemonBootCheck {
                ok: false,
                path,
                version: None,
                hint: error.hint(),
                error: Some(error.to_string()),
                log_path,
            };
        }
    };

    match wait_with_timeout(child, VERSION_PROBE_TIMEOUT) {
        Ok(output) if output.status.success() => DaemonBootCheck {
            ok: true,
            path,
            version: Some(String::from_utf8_lossy(&output.stdout).trim().to_string()),
            error: None,
            hint: None,
            log_path,
        },
        Ok(output) => {
            let code = launch::exit_code_label(&output.status);
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let detail = if stderr.is_empty() {
                code.clone()
            } else {
                format!("{code}: {stderr}")
            };
            DaemonBootCheck {
                ok: false,
                path,
                version: None,
                hint: launch::launch_hint(&detail),
                error: Some(detail),
                log_path,
            }
        }
        Err(detail) => DaemonBootCheck {
            ok: false,
            path,
            version: None,
            hint: launch::launch_hint(&detail),
            error: Some(detail),
            log_path,
        },
    }
}

/// Waits for child output with a deadline so a blocked loader cannot wedge onboarding.
pub(crate) fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|e| format!("could not read the daemon's output: {e}"));
            }
            Ok(None) => {}
            Err(e) => return Err(format!("could not observe the daemon process: {e}")),
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "the daemon did not print a version within {}s",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Opens the daemon log with the OS text-file handler, creating its directory for a fresh install.
#[tauri::command]
pub fn open_daemon_log(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let path = service::log_file();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if !path.exists() {
        std::fs::write(&path, "")
            .map_err(|e| format!("could not create {}: {e}", path.display()))?;
    }
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Builds diagnostics from native-only state, including the resolved daemon path, log tail, and IPC
/// endpoint.
pub fn diagnostics_report(
    app_version: &str,
    endpoint: &str,
    connection_phase: &str,
    connection_message: Option<&str>,
    daemon_path: &str,
    log_path: &str,
    log_tail: &str,
) -> String {
    let mut out = String::new();
    out.push_str("Repomon diagnostics\n");
    out.push_str(&format!("app: {app_version}\n"));
    out.push_str(&format!(
        "os: {} {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    out.push_str(&format!("endpoint: {endpoint}\n"));
    out.push_str(&format!("connection: {connection_phase}\n"));
    if let Some(message) = connection_message {
        out.push_str(&format!("last error: {message}\n"));
    }
    out.push_str(&format!("daemon binary: {daemon_path}\n"));
    out.push_str(&format!("daemon log: {log_path}\n"));
    out.push_str("\n-- daemon log tail --\n");
    out.push_str(if log_tail.is_empty() {
        "(the daemon log is empty)"
    } else {
        log_tail
    });
    out.push('\n');
    out
}

#[tauri::command]
pub fn daemon_diagnostics(app: AppHandle, state: State<'_, AppState>) -> String {
    let snapshot = state.connection.read().unwrap().clone();
    let log_path: PathBuf = service::log_file();
    diagnostics_report(
        app.package_info().version.to_string().as_str(),
        &snapshot.endpoint,
        &snapshot.phase,
        snapshot.message.as_deref(),
        &service::repomond_path().display().to_string(),
        &log_path.display().to_string(),
        &launch::log_tail(&log_path, launch::LOG_TAIL_LINES),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_carry_the_endpoint_binary_log_and_tail() {
        let report = diagnostics_report(
            "0.8.1",
            r"\\.\pipe\repomon-azama",
            "retrying",
            Some("the daemon started and exited immediately (exit code -1073741515 / 0xC0000135)"),
            r"C:\Users\azama\AppData\Local\Repomon\repomond.exe",
            r"C:\Users\azama\AppData\Roaming\repomon\data\logs\repomond.out.log",
            "[launch] daemon exited immediately after launch",
        );
        assert!(report.contains("app: 0.8.1"));
        assert!(report.contains(r"\\.\pipe\repomon-azama"));
        assert!(report.contains("connection: retrying"));
        assert!(report.contains("0xC0000135"));
        assert!(report.contains("repomond.exe"));
        assert!(report.contains("repomond.out.log"));
        assert!(report.contains("[launch] daemon exited immediately after launch"));
    }

    #[test]
    fn diagnostics_say_so_when_the_log_is_empty() {
        let report = diagnostics_report(
            "0.8.1",
            "/tmp/repomon-test.sock",
            "connected",
            None,
            "/Applications/Repomon.app/Contents/MacOS/repomond",
            "/tmp/logs/repomond.out.log",
            "",
        );
        assert!(report.contains("(the daemon log is empty)"));
        assert!(!report.contains("last error:"));
    }
}
