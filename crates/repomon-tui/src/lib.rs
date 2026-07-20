//! `repomon` TUI — library surface.
//!
//! The binary is a thin wrapper around [`run_cli`]. Exposing these modules as a library lets
//! integration tests drive the real client + app + view stack against an embedded daemon.

pub mod app;
pub mod cli;
pub mod client;
pub mod emu;
pub mod keybinds;
pub mod notify;
pub mod theme;
pub mod view;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use client::DaemonClient;
use repomon_core::{Config, config};

pub use repomon_core::launch::{connect_with_retry, ensure_daemon, spawn_daemon};

#[derive(Parser)]
#[command(
    name = "repomon",
    version,
    about = "Run a fleet of AI coding agents across all your repos, from one terminal"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<cli::Command>,
    /// Override the daemon socket path.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Run an in-process daemon (dev convenience).
    #[arg(long)]
    embedded: bool,
    /// Render one Fleet frame to stdout and exit.
    #[arg(long = "print-once")]
    print_once: bool,
}

/// Parse arguments and run the requested mode.
pub async fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load().unwrap_or_default();

    // A subcommand runs headless and exits; no subcommand launches the TUI.
    if let Some(command) = cli.command {
        return cli::handle(command, &config, cli.socket).await;
    }

    // Acquire a daemon: --embedded forces in-process; otherwise connect to a running
    // daemon, auto-start a detached `repomond` if none, and fall back to in-process if the
    // repomond binary can't be found. So plain `repomon` always just works.
    let mut _embedded = None;
    let client = if cli.embedded {
        let (socket, guard) = start_embedded(&config).await?;
        _embedded = Some(guard);
        connect_with_retry(&socket, 100).await?
    } else {
        match ensure_daemon(&config, cli.socket.clone()).await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("repomon: starting in-process daemon ({e})");
                let (socket, guard) = start_embedded(&config).await?;
                _embedded = Some(guard);
                connect_with_retry(&socket, 100).await?
            }
        }
    };

    if cli.print_once {
        print_once(&client).await?;
        return Ok(());
    }

    let theme = theme::Theme::from_accent(config.accent.as_deref());
    let cd = app::run(client, theme).await?;
    if let Some(path) = cd {
        emit_cd(&path);
    }
    Ok(())
}

/// Render a single Fleet frame to stdout via the test backend (no TTY needed).
pub async fn print_once(client: &DaemonClient) -> Result<()> {
    let mut app = app::App::new(client.clone());
    app.refresh().await;
    let s = render_to_string(&app, 100, 44)?;
    print!("{s}");
    Ok(())
}

/// Render the app to a plain string at the given size (used by `--print-once` and tests).
pub fn render_to_string(app: &app::App, width: u16, height: u16) -> Result<String> {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend)?;
    terminal.draw(|f| view::render(f, app))?;
    Ok(view::buffer_to_string(terminal.backend().buffer()))
}

/// Keep the embedded daemon's tasks (and watcher) alive for the process lifetime.
pub struct EmbeddedGuard {
    _serve: tokio::task::JoinHandle<()>,
    _watcher: Option<repomon_core::Watcher>,
}

async fn start_embedded(config: &Config) -> Result<(PathBuf, EmbeddedGuard)> {
    use repomon_core::{Store, Watcher};
    use repomon_daemon::{Ctx, serve};

    let db = config::db_path();
    let store = Store::open(&db).with_context(|| format!("opening store at {}", db.display()))?;
    let ctx = Ctx::new(store, config.clone(), Some(db));
    #[cfg(unix)]
    let socket = std::env::temp_dir().join(format!("repomon-embedded-{}.sock", std::process::id()));
    #[cfg(windows)]
    let socket = PathBuf::from(format!(r"\\.\pipe\repomon-embedded-{}", std::process::id()));

    let mut watcher = Watcher::new().ok();
    if let Some(w) = watcher.as_mut() {
        if let Ok(repos) = ctx.registry.list().await {
            for repo in repos {
                let _ = w.watch_path(&repo.path);
            }
        }
        let projects = repomon_core::agent::claude::projects_root();
        if projects.exists() {
            let _ = w.watch_path(&projects);
        }
        let mut rx = w.subscribe();
        let ctx_w = ctx.clone();
        tokio::spawn(async move {
            while let Ok(change) = rx.recv().await {
                ctx_w.broadcast(
                    "event.repo.changed",
                    serde_json::json!({ "path": change.path.to_string_lossy() }),
                );
            }
        });
    }

    tokio::spawn(repomon_daemon::stream_output(ctx.clone()));
    tokio::spawn(repomon_daemon::stream_orchestrator(ctx.clone()));

    {
        let indexer = repomon_core::Indexer::new(ctx.store.clone(), ctx.registry.clone());
        tokio::spawn(async move {
            let _ = indexer.sync_all().await;
        });
    }

    let ctx_s = ctx.clone();
    let socket_s = socket.clone();
    let serve = tokio::spawn(async move {
        let _ = serve(ctx_s, &socket_s).await;
    });

    Ok((
        socket,
        EmbeddedGuard {
            _serve: serve,
            _watcher: watcher,
        },
    ))
}

/// Write the chosen path for the shell wrapper to cd into: `$REPOMON_CD_FILE` (a temp
/// file, used by the PowerShell wrapper), else `$REPOMON_CD_FD` (an inherited fd, used
/// by the POSIX/fish wrappers), else stdout.
fn emit_cd(path: &Path) {
    if let Ok(file_path) = std::env::var("REPOMON_CD_FILE")
        && !file_path.is_empty()
        && std::fs::write(&file_path, format!("{}\n", path.display())).is_ok()
    {
        return;
    }
    #[cfg(unix)]
    if let Ok(fd_str) = std::env::var("REPOMON_CD_FD") {
        if let Ok(fd) = fd_str.parse::<i32>() {
            use std::io::Write;
            use std::os::unix::io::FromRawFd;
            // Safety: REPOMON_CD_FD names an fd the parent shell opened for us.
            let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
            let _ = writeln!(file, "{}", path.display());
            std::mem::forget(file); // don't close the borrowed fd
            return;
        }
    }
    println!("{}", path.display());
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn emit_cd_writes_repomon_cd_file() {
        let dir = std::env::temp_dir().join(format!("repomon-emit-cd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cd_file = dir.join("cd-target");
        // Safety: tests in this module are the only readers/writers of this var.
        unsafe { std::env::set_var("REPOMON_CD_FILE", &cd_file) };
        super::emit_cd(Path::new("/some/lane/worktree"));
        unsafe { std::env::remove_var("REPOMON_CD_FILE") };
        let written = std::fs::read_to_string(&cd_file).unwrap();
        assert_eq!(written.trim_end(), "/some/lane/worktree");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
