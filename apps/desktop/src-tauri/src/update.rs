use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use repomon_core::{Config, config, service};

const UPDATE_MARKER: &str = "daemon-update.pending";

fn marker_path() -> PathBuf {
    config::data_dir().join(UPDATE_MARKER)
}

fn managed_daemon_path() -> PathBuf {
    config::data_dir()
        .join("bin")
        .join(format!("repomond{}", std::env::consts::EXE_SUFFIX))
}

#[cfg(windows)]
fn bundled_agent_host_path() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|directory| {
        directory.join(format!(
            "repomon-agent-host{}",
            std::env::consts::EXE_SUFFIX
        ))
    })
}

#[cfg(not(windows))]
fn bundled_tmux_path() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(directory) = exe.parent() {
            let cand = directory.join(format!("tmux{}", std::env::consts::EXE_SUFFIX));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    let bundled = service::repomond_path();
    if let Some(directory) = bundled.parent() {
        let cand = directory.join(format!("tmux{}", std::env::consts::EXE_SUFFIX));
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

fn service_is_installed(status: &str) -> bool {
    !status.trim().eq_ignore_ascii_case("not installed")
}

fn copy_daemon(source: &Path, destination: &Path) -> Result<()> {
    if !source.is_file() {
        bail!("bundled daemon not found at {}", source.display());
    }
    let parent = destination
        .parent()
        .context("managed daemon path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let temporary = destination.with_extension(format!("update-{}", std::process::id()));
    std::fs::copy(source, &temporary).with_context(|| {
        format!(
            "copying bundled daemon from {} to {}",
            source.display(),
            temporary.display()
        )
    })?;

    #[cfg(windows)]
    if destination.exists() {
        std::fs::remove_file(destination)?;
    }
    std::fs::rename(&temporary, destination)?;
    Ok(())
}

#[tauri::command]
pub fn mark_daemon_update() -> Result<(), String> {
    let marker = marker_path();
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(marker, b"pending\n").map_err(|error| error.to_string())
}

#[tauri::command]
pub fn clear_daemon_update() -> Result<(), String> {
    let marker = marker_path();
    match std::fs::remove_file(marker) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub async fn apply_pending_daemon_update(
    config_value: &Config,
    socket_override: Option<PathBuf>,
) -> Result<()> {
    let marker = marker_path();
    if !marker.exists() {
        return Ok(());
    }

    let socket = socket_override
        .clone()
        .unwrap_or_else(|| config::socket_path(config_value));
    let bundled = service::repomond_path();
    let managed = managed_daemon_path();
    let service_managed = socket_override.is_none()
        && service::status()
            .map(|status| service_is_installed(&status))
            .unwrap_or(false);

    if let Ok(client) = repomon_core::client::DaemonClient::connect(&socket).await {
        let _ = client.call("daemon.shutdown", None).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    if service_managed {
        let _ = service::stop();
    }

    copy_daemon(&bundled, &managed)?;
    #[cfg(windows)]
    if let Some(host) = bundled_agent_host_path().filter(|path| path.is_file()) {
        let destination = managed
            .parent()
            .context("managed daemon path has no parent")?
            .join(format!(
                "repomon-agent-host{}",
                std::env::consts::EXE_SUFFIX
            ));
        copy_daemon(&host, &destination)?;
    }
    #[cfg(not(windows))]
    if let Some(tmux) = bundled_tmux_path().filter(|path| path.is_file()) {
        let destination = managed
            .parent()
            .context("managed daemon path has no parent")?
            .join(format!("tmux{}", std::env::consts::EXE_SUFFIX));
        copy_daemon(&tmux, &destination)?;
    }
    if service_managed {
        service::install(&managed, &socket)?;
    }
    std::fs::remove_file(marker)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_platform_absence_sentinel_means_no_service() {
        assert!(!service_is_installed("not installed"));
        assert!(!service_is_installed("  NOT INSTALLED\n"));
        assert!(service_is_installed("active"));
        assert!(service_is_installed("Running"));
    }

    #[test]
    fn copies_the_bundled_daemon_to_the_managed_path() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("repomond-source");
        let destination = dir.path().join("bin").join("repomond");
        std::fs::write(&source, b"daemon-v2").unwrap();

        copy_daemon(&source, &destination).unwrap();

        assert_eq!(std::fs::read(destination).unwrap(), b"daemon-v2");
    }

    #[test]
    fn copies_sibling_tmux_to_managed_path() {
        let dir = tempfile::tempdir().unwrap();
        let tmux_source = dir.path().join("tmux");
        let tmux_destination = dir.path().join("bin").join("tmux");
        std::fs::write(&tmux_source, b"tmux-binary").unwrap();

        copy_daemon(&tmux_source, &tmux_destination).unwrap();

        assert_eq!(std::fs::read(tmux_destination).unwrap(), b"tmux-binary");
    }
}
