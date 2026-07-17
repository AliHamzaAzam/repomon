//! `repomond` — the repomon background daemon.
//!
//! Opens the SQLite store, starts the file watcher, and serves the JSON-RPC API over a Unix
//! socket until interrupted.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use repomon_core::{Config, Store, Watcher, config};
use repomon_daemon::{Ctx, serve};
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

#[tokio::main]
async fn main() {
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
    // Reflect a `--socket` override into the in-memory config: everything that later derives the
    // socket from config — most importantly the orchestrator spawn path, which points the fleet
    // MCP server (`repomond mcp`) back at the daemon via `REPOMON_MCP_SOCKET` — must name the
    // socket this process actually binds, or repomind's MCP server connects to a socket nobody
    // is listening on and dies during its initialize handshake.
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

    // Watch registered repos; rebroadcast changes so clients can refresh. Done in a background
    // task (and the watcher is owned by it) so the socket binds immediately — registering a
    // recursive watch on a large tree like ~/.claude/projects can take a few seconds, and we
    // don't want clients to see a "not running" gap while it sets up.
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
            // a tree at runtime — otherwise the watch set only reflects startup, and a removed repo
            // keeps churning fsevents until the next restart.
            *ctx_w.watcher.lock().await = Some(watcher);
            while let Ok(change) = rx.recv().await {
                // Drop this worktree's cached git state so it re-walks (rate-limited) on the next
                // list — the only thing that should trigger a fresh gix status walk.
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
            tick.tick().await; // fire immediately once, then every 60s
            loop {
                tick.tick().await;
                ctx_t.broadcast("event.repo.changed", json!({ "path": null }));
            }
        });
    }

    // Stream visible agents' output to subscribed TUIs.
    tokio::spawn(repomon_daemon::stream_output(ctx.clone()));

    // Sweep stale pipe-panes from a previous daemon: a watch left on with no reader would make
    // tmux buffer that pane's output in memory without bound.
    tokio::spawn(repomon_daemon::bytes_stream::sweep(ctx.tmux.clone()));

    // Stream the repomind orchestrator's pane to a watching command-center view (self-gates on a
    // running session + a watcher, so it's free until the orchestrator is opened).
    tokio::spawn(repomon_daemon::stream_orchestrator(ctx.clone()));

    // Remote-access bridge (companion apps over Tailscale) — only when explicitly enabled
    // and a token exists; without both, no network listener is ever opened.
    {
        let remote = ctx.config.read().await.remote.clone();
        if remote.enabled {
            match remote.bind {
                Some(bind) => {
                    // Seed the auth cache (paired device tokens + the legacy config token) before
                    // the listener accepts, so the first handshake matches against a current set.
                    // Under the mutate lock for consistency with pair/revoke (nothing races here yet,
                    // but the choke point stays uniform).
                    {
                        let _guard = ctx.remote_mutate_lock.lock().await;
                        if let Err(e) = repomon_daemon::rpc::refresh_remote_tokens(&ctx).await {
                            tracing::error!("failed to seed remote tokens: {e:?}");
                        }
                    }
                    let ctx_r = ctx.clone();
                    tokio::spawn(async move {
                        // Keep retrying, not just at startup: the bind is typically a Tailscale
                        // IP, which isn't assignable until the tailnet interface is up — a
                        // daemon started at login (or a Mac waking from sleep) raced it and the
                        // bridge stayed dead until the next manual restart. Ok(()) means a
                        // clean shutdown; any Err waits out a short delay and binds again.
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

    // Daemon-side notification engine for remote clients (event.notification + push). Spawned
    // unconditionally — it self-gates per tick on `[remote] enabled`, so flipping the config
    // live (config.set) starts/stops it without a restart.
    tokio::spawn(repomon_daemon::notify_watch::notify_watch(ctx.clone()));

    // Probe Claude's `/usage` for the TUI's account-usage corner. Self-gates per tick on
    // `[usage_probe]` and a TUI being attached, so it costs nothing until enabled and watched.
    tokio::spawn(repomon_daemon::usage_watch::usage_watcher(ctx.clone()));

    // Reap orphaned `lane-<id>` windows whose id no longer maps to the worktree they claim —
    // leftovers from a re-registered worktree or a store reset the long-lived tmux server
    // outlived. Sweeps immediately on startup, then slowly, so phantom "exited" sessions
    // (idle `claude` processes that never exit on their own) clean themselves up.
    tokio::spawn(repomon_daemon::reap::reap_watcher(ctx.clone()));

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
}

/// `repomond mcp` — serve the MCP protocol over stdio for the repomind orchestrator.
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
