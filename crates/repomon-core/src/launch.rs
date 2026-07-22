//! Daemon discovery, launch, and startup retry shared by local UI clients.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

use crate::{Config, client::DaemonClient, config, service};

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
    // A first start may run SQLite migrations before binding the socket.
    connect_with_retry(&socket, 150).await
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
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }

    cmd.spawn()
        .with_context(|| format!("starting daemon `{}`", program.display()))?;
    Ok(())
}

/// Connect to the daemon, retrying while a newly started process binds its endpoint.
pub async fn connect_with_retry(socket: &Path, tries: usize) -> Result<DaemonClient> {
    let mut last = None;
    for _ in 0..tries {
        match DaemonClient::connect(socket).await {
            Ok(client) => return Ok(client),
            Err(error) => {
                last = Some(error);
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
        }
    }

    Err(last.unwrap_or_else(|| anyhow!("could not connect"))).with_context(|| {
        format!(
            "no daemon at {} - start it with `repomond` or run with --embedded",
            socket.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::net::UnixListener;

    use super::connect_with_retry;

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
}
