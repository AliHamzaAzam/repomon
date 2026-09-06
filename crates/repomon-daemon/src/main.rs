//! Serves the JSON-RPC API over platform IPC with a persistent store and file watchers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use repomon_core::transport::{self, Endpoint, IpcListener};
use repomon_core::{Config, Store, Watcher, config};
use repomon_daemon::{Ctx, socket::serve_listener};
use serde_json::json;
use tokio::time::{Duration, interval};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "repomond", version, about = "The repomon background daemon")]
struct Args {
    /// Override the socket path.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Override the database path.
    #[arg(long)]
    data: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run as an MCP server over stdio for the repomind orchestrator. Connects to the running
    /// daemon as a client and exposes the fleet as MCP tools; logs go to stderr so stdout stays
    /// a clean protocol channel. Normally launched by `repomon orchestrate`, not by hand.
    Mcp,
}

fn main() {
    // Before any thread exists: a Finder/Dock launch (and a launchd service) hands the daemon a
    // stripped PATH, which would hide tmux and every agent binary from the whole process tree.
    // `set_var` is only sound while single-threaded, so this must precede the runtime.
    unsafe { repomon_daemon::path_env::repair_path_before_threads() };
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build the tokio runtime")
        .block_on(run());
}

async fn run() {
    let args = Args::parse();

    // The MCP subcommand is a stdio protocol server: keep all logging on stderr and never run
    // the daemon setup below (it connects to the *already-running* daemon as a client).
    if let Some(Command::Mcp) = args.command {
        run_mcp(args.socket).await;
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let mut config = Config::load().unwrap_or_default();
    // Keep derived MCP connection settings aligned with the endpoint this daemon actually binds.
    if let Some(sock) = &args.socket {
        config.socket_path = Some(sock.clone());
    }
    let db = args.data.unwrap_or_else(config::db_path);
    let socket = config::socket_path(&config);

    let store = match Store::open(&db) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to open store at {}: {e}", db.display());
            std::process::exit(1);
        }
    };

    let ctx = Ctx::new(store, config, Some(db));

    let listener = match bind_before_startup(&socket, start_background_tasks(ctx.clone())).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("serve error: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = serve_listener(ctx, &socket, listener).await {
        eprintln!("serve error: {e}");
        std::process::exit(1);
    }
}

/// Binds before startup can spawn children, preventing listener inheritance before cloexec is set.
async fn bind_before_startup(
    socket: &Path,
    startup: impl std::future::Future<Output = ()>,
) -> std::io::Result<IpcListener> {
    let listener = transport::listen(&Endpoint::from_path(socket)).await?;
    startup.await;
    Ok(listener)
}

async fn start_background_tasks(ctx: Arc<Ctx>) {
    // Make the repomind home exist, be registered, and carry its controller lane. In a background
    // task for the same reason as the watcher below: a first run creates directories and runs
    // `git init`, and the socket should bind before any of that.
    {
        let ctx_r = ctx.clone();
        tokio::spawn(async move { repomon_daemon::repomind::start(&ctx_r).await });
    }

    // Bind the socket while recursive watcher setup runs in the background.
    {
        let ctx_w = ctx.clone();
        tokio::spawn(async move {
            let mut watcher = match Watcher::new() {
                Ok(w) => w,
                Err(e) => {
                    tracing::warn!("watcher init failed: {e}");
                    return;
                }
            };
            if let Ok(repos) = ctx_w.registry.list().await {
                for repo in repos {
                    if let Err(e) = watcher.watch_path(&repo.path) {
                        tracing::warn!("watch {}: {e}", repo.path.display());
                    }
                }
            }
            // Watch Claude Code transcripts so agent status (and "needs you") refreshes live.
            let projects = repomon_core::agent::claude::projects_root();
            if projects.exists() {
                let _ = watcher.watch_path(&projects);
            }
            let mut rx = watcher.subscribe();
            // Hand the watcher to the shared context so repo.add / repo.remove can watch / unwatch
            // a tree at runtime - otherwise the watch set only reflects startup, and a removed repo
            // keeps churning fsevents until the next restart.
            *ctx_w.watcher.lock().await = Some(watcher);
            while let Ok(change) = rx.recv().await {
                // Drop this worktree's cached git state so it re-walks (rate-limited) on the next
                // list - the only thing that should trigger a fresh gix status walk.
                ctx_w.lanes.invalidate_state(&change.path);
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
            tick.tick().await;
            loop {
                tick.tick().await;
                ctx_t.broadcast("event.repo.changed", json!({ "path": null }));
            }
        });
    }

    tokio::spawn(repomon_daemon::stream_output(ctx.clone()));

    // Clear unread pipe-panes so tmux cannot buffer their output indefinitely.
    tokio::spawn(repomon_daemon::bytes_stream::sweep(ctx.backend.clone()));

    // Stream the repomind orchestrator's pane to a watching command-center view (self-gates on a
    // running session + a watcher, so it's free until the orchestrator is opened).
    tokio::spawn(repomon_daemon::stream_orchestrator(ctx.clone()));

    // Remote-access bridge (companion apps over Tailscale) - only when explicitly enabled
    // and a token exists; without both, no network listener is ever opened.
    {
        let remote = ctx.config.read().await.remote.clone();
        if remote.enabled {
            match remote.bind {
                Some(bind) => {
                    // Seed authentication before accepting clients, under the mutation lock to
                    // avoid publishing stale state.
                    {
                        let _guard = ctx.remote_mutate_lock.lock().await;
                        if let Err(e) = repomon_daemon::rpc::refresh_remote_tokens(&ctx).await {
                            tracing::error!("failed to seed remote tokens: {e:?}");
                        }
                    }
                    let ctx_r = ctx.clone();
                    tokio::spawn(async move {
                        // Retry binding because the tailnet interface may become available after
                        // daemon startup or wake.
                        loop {
                            match repomon_daemon::remote::serve_remote(ctx_r.clone(), &bind).await {
                                Ok(()) => break,
                                Err(e) => {
                                    tracing::warn!("remote bridge failed (retrying in 15s): {e}");
                                }
                            }
                            tokio::select! {
                                _ = ctx_r.shutdown.notified() => break,
                                _ = tokio::time::sleep(std::time::Duration::from_secs(15)) => {}
                            }
                        }
                    });
                }
                None => tracing::warn!(
                    "[remote] enabled but bind missing — run `repomon remote enable`"
                ),
            }
        }
    }

    // Auto-continue agents paused on a usage limit (resume at the reset time).
    tokio::spawn(repomon_daemon::auto_continue::auto_continue_watcher(
        ctx.clone(),
    ));

    // Daemon-side notification engine for subscribed clients (event.notification + optional push). Spawned
    // unconditionally - it self-gates per tick on `[remote] enabled`, so flipping the config
    // live (config.set) starts/stops it without a restart.
    tokio::spawn(repomon_daemon::notify_watch::notify_watch(ctx.clone()));

    // Supervision watcher: policy-driven dialog answering and hold recording for supervised lanes.
    tokio::spawn(repomon_daemon::supervision::supervision_watch(ctx.clone()));

    // Deliver queued fleet mail only when the resolved managed recipient is safe to interrupt.
    tokio::spawn(repomon_daemon::mail::delivery_worker(ctx.clone()));

    // Standing orchestrations: fire due schedules as bounded headless repomind runs. Costs
    // nothing until a schedule exists.
    tokio::spawn(repomon_daemon::standing::standing_watch(ctx.clone()));

    // Debounced one-way export of the journal, schedules, and approval rules into the repomind
    // home, plus the commit that records each batch there.
    tokio::spawn(repomon_daemon::repomind::export::export_watch(ctx.clone()));

    // Once a day, roll journal day files past the 90-day horizon into their month's archive.
    // The start-of-day pass is part of `repomind::start`; this only covers a daemon that stays
    // up across midnights.
    tokio::spawn(repomon_daemon::repomind::export::archive_watch(ctx.clone()));

    // Probe Claude's `/usage` for local UIs' account-usage display. Self-gates per tick on
    // `[usage_probe]` and a local UI being active, so it costs nothing until enabled and watched.
    tokio::spawn(repomon_daemon::usage_watch::usage_watcher(ctx.clone()));

    // Ingest agent transcripts into the usage ledger. Self-gates per pass on `[usage] enabled`,
    // reads only files that already exist, and bounds its work per tick.
    tokio::spawn(repomon_daemon::usage_ingest::ingest_watch(ctx.clone()));

    // Daily LiteLLM price refresh. Self-gates on `[usage] refresh_prices` (on by default); the
    // cache starts stale, so this also covers "fetch at daemon start when older than 24h".
    repomon_daemon::usage_rates::spawn_daily_task(ctx.clone());

    tokio::spawn(repomon_daemon::reap::reap_watcher(ctx.clone()));

    // Index commit history in the background (timeline / sessions / search).
    {
        let indexer = repomon_core::Indexer::new(ctx.store.clone(), ctx.registry.clone());
        tokio::spawn(async move {
            let _ = indexer.sync_all().await;
        });
    }

    {
        let ctx_s = ctx.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutdown requested");
            ctx_s.request_shutdown();
        });
    }
}

/// `repomond mcp` - serve the MCP protocol over stdio for the repomind orchestrator.
async fn run_mcp(socket_override: Option<PathBuf>) {
    // Logs to stderr only: stdout carries the newline-delimited MCP JSON-RPC stream.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let config = Config::load().unwrap_or_default();
    let socket = socket_override
        .or_else(|| std::env::var("REPOMON_MCP_SOCKET").ok().map(PathBuf::from))
        .unwrap_or_else(|| config::socket_path(&config));

    if let Err(e) = repomon_mcp::serve_stdio(repomon_mcp::Options { socket }).await {
        eprintln!("repomond mcp: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ipc_is_bound_before_startup_can_spawn_children() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("startup.sock");
        let endpoint = Endpoint::from_path(&socket);
        let mut startup_ran = false;

        let _listener = bind_before_startup(&socket, async {
            transport::connect(&endpoint)
                .await
                .expect("IPC must be bound before the startup future is polled");
            startup_ran = true;
        })
        .await
        .unwrap();

        assert!(startup_ran);
    }

    #[tokio::test]
    async fn bind_failure_does_not_start_background_work() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("occupied.sock");
        let endpoint = Endpoint::from_path(&socket);
        let _owner = transport::listen(&endpoint).await.unwrap();
        let mut startup_ran = false;

        let result = bind_before_startup(&socket, async { startup_ran = true }).await;

        assert!(result.is_err());
        assert!(!startup_ran);
    }
}
