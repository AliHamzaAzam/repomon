//! `repomond` — the repomon background daemon.
//!
//! Opens the SQLite store, starts the file watcher, and serves the JSON-RPC API over a Unix
//! socket until interrupted.

use std::path::PathBuf;

use clap::Parser;
use repomon_core::{config, Config, Store, Watcher};
use repomon_daemon::{serve, Ctx};
use serde_json::json;
use tokio::time::{interval, Duration};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "repomond", about = "The repomon background daemon")]
struct Args {
    /// Override the socket path.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Override the database path.
    #[arg(long)]
    data: Option<PathBuf>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let config = Config::load().unwrap_or_default();
    let db = args.data.unwrap_or_else(config::db_path);
    let socket = args.socket.unwrap_or_else(|| config::socket_path(&config));

    let store = match Store::open(&db) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to open store at {}: {e}", db.display());
            std::process::exit(1);
        }
    };

    let ctx = Ctx::new(store, config, Some(db));

    // Watch registered repos; rebroadcast changes so clients can refresh.
    let mut watcher = Watcher::new().ok();
    if let Some(w) = watcher.as_mut() {
        if let Ok(repos) = ctx.registry.list().await {
            for repo in repos {
                if let Err(e) = w.watch_path(&repo.path) {
                    tracing::warn!("watch {}: {e}", repo.path.display());
                }
            }
        }
        // Watch Claude Code transcripts so agent status (and "needs you") refreshes live.
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
                    json!({ "path": change.path.to_string_lossy(), "kind": format!("{:?}", change.kind) }),
                );
            }
        });
    }

    // Safety-net refresh hint, in case a filesystem event is ever missed.
    {
        let ctx_t = ctx.clone();
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(60));
            tick.tick().await; // fire immediately once, then every 60s
            loop {
                tick.tick().await;
                ctx_t.broadcast("event.repo.changed", json!({ "path": null }));
            }
        });
    }

    // Stream visible agents' output to subscribed TUIs.
    tokio::spawn(repomon_daemon::stream_output(ctx.clone()));

    // Index commit history in the background (timeline / sessions / search).
    {
        let indexer = repomon_core::Indexer::new(ctx.store.clone(), ctx.registry.clone());
        tokio::spawn(async move {
            let _ = indexer.sync_all().await;
        });
    }

    // Graceful shutdown on Ctrl-C / SIGTERM.
    {
        let ctx_s = ctx.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutdown requested");
            ctx_s.request_shutdown();
        });
    }

    if let Err(e) = serve(ctx, &socket).await {
        eprintln!("serve error: {e}");
        std::process::exit(1);
    }

    drop(watcher); // keep the watcher alive for the daemon's lifetime
}
