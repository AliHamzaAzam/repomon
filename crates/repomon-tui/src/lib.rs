//! `repomon` TUI — library surface.
//!
//! The binary is a thin wrapper around [`run_cli`]. Exposing these modules as a library lets
//! integration tests drive the real client + app + view stack against an embedded daemon.

pub mod app;
pub mod cli;
pub mod client;
pub mod keybinds;
pub mod theme;
pub mod view;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use client::DaemonClient;
use repomon_core::{config, Config};

#[derive(Parser)]
#[command(
    name = "repomon",
    about = "Terminal mission control for parallel coding agents"
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

    let (socket, _embedded) = if cli.embedded {
        let (socket, guard) = start_embedded(&config).await?;
        (socket, Some(guard))
    } else {
        (
            cli.socket.unwrap_or_else(|| config::socket_path(&config)),
            None,
        )
    };

    let client = connect_with_retry(&socket, if cli.embedded { 100 } else { 20 }).await?;

    if cli.print_once {
        print_once(&client).await?;
        return Ok(());
    }

    let cd = app::run(client).await?;
    if let Some(path) = cd {
        emit_cd(&path);
    }
    Ok(())
}

/// Connect to the daemon, retrying briefly (the embedded daemon needs a moment to bind).
pub async fn connect_with_retry(socket: &Path, tries: usize) -> Result<DaemonClient> {
    let mut last = None;
    for _ in 0..tries {
        match DaemonClient::connect(socket).await {
            Ok(c) => return Ok(c),
            Err(e) => {
                last = Some(e);
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("could not connect"))).with_context(|| {
        format!(
            "no daemon at {} — start it with `repomond` or run with --embedded",
            socket.display()
        )
    })
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
    use repomon_daemon::{serve, Ctx};

    let db = config::db_path();
    let store = Store::open(&db).with_context(|| format!("opening store at {}", db.display()))?;
    let ctx = Ctx::new(store, config.clone(), Some(db));
    let socket = std::env::temp_dir().join(format!("repomon-embedded-{}.sock", std::process::id()));

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

/// Write the chosen path to `$REPOMON_CD_FD` (or stdout) for the shell wrapper to cd into.
fn emit_cd(path: &Path) {
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
