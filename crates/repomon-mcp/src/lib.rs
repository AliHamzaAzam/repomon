//! repomind — an MCP server that exposes the repomon fleet to an orchestrator agent.
//!
//! The orchestrator is an ordinary `claude` session (launched by `repomon orchestrate`) with
//! this server attached over stdio. It connects to the running daemon as a client, keeps a
//! poll-and-diff fleet snapshot, and offers orchestrator-ergonomic tools (`fleet_status`,
//! `read_agent`, `spawn_agent`, `wait_for_change`, …) that translate to the daemon's existing
//! RPC. The worker agents are the same durable tmux sessions repomon already manages.
//!
//! Entry point: [`serve_stdio`], invoked by the `repomond mcp` subcommand.

pub mod fleet;
pub mod mcp;
pub mod policy;
pub mod server;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use repomon_core::client::DaemonClient;

/// The orchestrator persona / system prompt shipped with repomind, passed to `claude` via
/// `--append-system-prompt` by the launcher.
pub const PERSONA: &str = include_str!("../assets/repomind.md");

/// Appended to [`PERSONA`] for headless standing/triage runs. The server enforces the hard
/// rules (merge_lane/delete_lane refuse in unattended mode); this addendum sets expectations
/// so the model plans within them instead of bumping into refusals.
pub const UNATTENDED_ADDENDUM: &str = "\n\n## Unattended run\n\nThis is a bounded, unattended \
standing run: no human is watching, and your final message is delivered as a notification \
(phone lock screen sized). Rules:\n\n\
- You cannot merge_lane or delete_lane here (the server refuses them). Verify merge-ready \
work with lane_diff and RECOMMEND the action instead.\n\
- Prefer observing and reporting over acting; act only when the goal explicitly asks for it \
and stay well inside the action cap.\n\
- Never wait on wait_for_change for long stretches; do the task, then stop.\n\
- End with a compact briefing (2-6 short lines): what you saw, what you did, what needs the \
human and your recommendation.";

/// How to run the server.
pub struct Options {
    /// The daemon socket to connect to.
    pub socket: PathBuf,
}

/// Connect to the daemon (retrying briefly, since the launcher may have just started it), bring
/// up the fleet poller, and serve MCP over stdio until the client closes stdin.
pub async fn serve_stdio(opts: Options) -> Result<()> {
    let client = connect_with_retry(&opts.socket, 150)
        .await
        .with_context(|| format!("connecting to repomon daemon at {}", opts.socket.display()))?;
    let fleet = fleet::Fleet::start(client.clone(), opts.socket.clone()).await;
    let policy = policy::Policy::from_env();
    tracing::info!(
        autonomy = policy.autonomy.as_str(),
        max_agents = policy.max_concurrent_agents,
        "repomind mcp server ready"
    );
    let server = Arc::new(server::Server::new(client, fleet, policy));
    mcp::run_stdio(server, "repomon", env!("CARGO_PKG_VERSION")).await
}

async fn connect_with_retry(socket: &Path, tries: usize) -> Result<DaemonClient> {
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
    Err(last.unwrap_or_else(|| anyhow!("could not connect")))
}
