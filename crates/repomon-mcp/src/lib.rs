//! Exposes daemon operations as provider-independent MCP tools over stdio.

pub mod agent;
pub mod fleet;
pub mod mcp;
pub mod policy;
pub mod server;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};

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

/// The env value that selects the restricted worker catalog.
pub const MCP_MODE_AGENT: &str = "agent";
/// The env value controllers are launched with. Explicit rather than implied by absence, so a
/// controller window that inherits a stray `REPOMON_MCP_MODE` from its parent still gets the
/// catalog it was launched for.
pub const MCP_MODE_ORCHESTRATOR: &str = "orchestrator";

/// Which tool catalog a server process serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogMode {
    /// The restricted worker surface (`agent::agent_tool_catalog`).
    Worker,
    /// The full fleet catalog, with the policy layer in front of it.
    Full,
}

impl CatalogMode {
    /// Resolve the catalog from a raw `REPOMON_MCP_MODE` value. Only `agent` narrows the surface;
    /// everything else - including the explicit `orchestrator` and an unset variable - is full.
    pub fn from_env_value(raw: Option<&str>) -> CatalogMode {
        match raw.map(|v| v.trim().to_ascii_lowercase()) {
            Some(v) if v == MCP_MODE_AGENT => CatalogMode::Worker,
            _ => CatalogMode::Full,
        }
    }
}

/// How to run the server.
pub struct Options {
    /// The daemon socket to connect to.
    pub socket: PathBuf,
}

/// Connect to the daemon (retrying briefly, since the launcher may have just started it), bring
/// up the fleet poller, and serve MCP over stdio until the client closes stdin.
pub async fn serve_stdio(opts: Options) -> Result<()> {
    let client = repomon_core::launch::connect_with_backoff(
        &opts.socket,
        repomon_core::launch::DEFAULT_DAEMON_CONNECT_TIMEOUT,
    )
    .await
    .with_context(|| format!("connecting to repomon daemon at {}", opts.socket.display()))?;
    let fleet = fleet::Fleet::start(client.clone(), opts.socket.clone()).await;
    let mode = CatalogMode::from_env_value(std::env::var("REPOMON_MCP_MODE").ok().as_deref());
    if mode == CatalogMode::Worker {
        let token = std::env::var("REPOMON_MCP_IDENTITY_TOKEN").unwrap_or_default();
        tracing::info!("managed-agent mcp server ready");
        let server = Arc::new(agent::AgentServer::new(client, fleet, token));
        return mcp::run_stdio(server, "repomon", env!("CARGO_PKG_VERSION")).await;
    }
    let policy = policy::Policy::from_env();
    tracing::info!(
        autonomy = policy.autonomy.as_str(),
        max_agents = policy.max_concurrent_agents,
        "repomind mcp server ready"
    );
    let server = Arc::new(server::Server::new(client, fleet, policy));
    mcp::run_stdio(server, "repomon", env!("CARGO_PKG_VERSION")).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Selects the restricted worker catalog only for agent mode, otherwise using the full
    /// orchestrator catalog.
    #[test]
    fn catalog_mode_selects_the_worker_surface_only_for_agent() {
        assert_eq!(
            CatalogMode::from_env_value(Some("agent")),
            CatalogMode::Worker
        );
        assert_eq!(
            CatalogMode::from_env_value(Some("orchestrator")),
            CatalogMode::Full
        );
        assert_eq!(CatalogMode::from_env_value(None), CatalogMode::Full);
        assert_eq!(CatalogMode::from_env_value(Some("")), CatalogMode::Full);
        assert_eq!(
            CatalogMode::from_env_value(Some("Agent")),
            CatalogMode::Worker
        );
        assert_eq!(
            CatalogMode::from_env_value(Some(" agent ")),
            CatalogMode::Worker
        );
        assert_eq!(
            CatalogMode::from_env_value(Some("whatever")),
            CatalogMode::Full
        );
    }
}
