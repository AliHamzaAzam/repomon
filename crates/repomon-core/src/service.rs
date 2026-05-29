//! Daemon service management (macOS launchd).
//!
//! Lives in `repomon-core` because the `repomon daemon …` subcommands run from the TUI
//! binary and must drive install/start/stop without depending on the daemon crate. The
//! systemd-user equivalent can land later; non-macOS targets get a clear error.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The launchd label / service identifier.
pub const LABEL: &str = "com.repomon.daemon";

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

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use std::process::Command;

    pub fn plist_path() -> PathBuf {
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
        let plist = plist_path();
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
        let plist = plist_path();
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

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;

    fn unsupported() -> Error {
        Error::Other("service management is currently macOS-only (launchd)".into())
    }

    pub fn plist_path() -> PathBuf {
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

pub use platform::{install, plist_path, start, status, stop, uninstall};

/// Path to the daemon log file (stdout).
pub fn log_file() -> PathBuf {
    log_dir().join("repomond.out.log")
}

/// Best-effort guess at the `repomond` binary path (sibling of the current exe).
pub fn repomond_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join("repomond");
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
}
