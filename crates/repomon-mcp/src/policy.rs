//! Server-enforced guardrails. These caps are enforced here, in the MCP layer, not merely
//! requested in the persona prompt — so a confused or runaway orchestrator physically cannot
//! exceed them. Configured from the environment by the `repomon orchestrate` launcher.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How much the orchestrator may do without a human in the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Autonomy {
    /// Observe only: no mutating tools.
    ReadOnly,
    /// May steer existing agents, but proposes lane creation for human confirmation.
    Supervised,
    /// Default: may also create lanes and run goals end-to-end, within the hard caps.
    Autonomous,
}

impl Autonomy {
    pub fn parse(s: &str) -> Autonomy {
        match s.trim().to_lowercase().as_str() {
            "read-only" | "readonly" | "read_only" | "observe" => Autonomy::ReadOnly,
            "supervised" | "suggest" => Autonomy::Supervised,
            _ => Autonomy::Autonomous,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Autonomy::ReadOnly => "read-only",
            Autonomy::Supervised => "supervised",
            Autonomy::Autonomous => "autonomous",
        }
    }
    /// Whether any state-changing tool is permitted at all.
    pub fn allows_mutation(self) -> bool {
        !matches!(self, Autonomy::ReadOnly)
    }
    /// Whether the orchestrator may create lanes itself (vs. asking the human first).
    pub fn allows_create_lane(self) -> bool {
        matches!(self, Autonomy::Autonomous)
    }
}

/// The runtime guardrails: a fixed configuration plus mutable counters.
pub struct Policy {
    pub autonomy: Autonomy,
    pub max_concurrent_agents: usize,
    pub max_actions: u64,
    actions: Mutex<u64>,
    last_send: Mutex<HashMap<i64, (String, Instant)>>,
}

impl Policy {
    /// Read configuration from the environment (set by the launcher), with safe defaults.
    pub fn from_env() -> Policy {
        let autonomy = std::env::var("REPOMON_MCP_AUTONOMY")
            .map(|s| Autonomy::parse(&s))
            .unwrap_or(Autonomy::Autonomous);
        let max_concurrent_agents = env_usize("REPOMON_MCP_MAX_AGENTS", 4);
        let max_actions = env_usize("REPOMON_MCP_MAX_ACTIONS", 100) as u64;
        Policy {
            autonomy,
            max_concurrent_agents,
            max_actions,
            actions: Mutex::new(0),
            last_send: Mutex::new(HashMap::new()),
        }
    }

    /// Gate a mutating action: refuse in read-only mode, otherwise count it against the
    /// per-session action cap (a runaway backstop).
    pub fn record_mutation(&self) -> Result<u64, String> {
        if !self.autonomy.allows_mutation() {
            return Err("autonomy is read-only — this tool only observes. \
                 Report what you see and let the human decide."
                .into());
        }
        let mut a = self
            .actions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *a >= self.max_actions {
            return Err(format!(
                "action cap reached ({} actions this session). Pausing for safety — \
                 summarize progress and check in with the human before continuing.",
                self.max_actions
            ));
        }
        *a += 1;
        Ok(*a)
    }

    /// Suppress an identical `send_to_agent` to the same lane within a short window — the
    /// cheapest defense against an infinite re-prompt / handoff loop.
    pub fn check_send_dedupe(&self, lane: i64, text: &str) -> Result<(), String> {
        let mut m = self
            .last_send
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((prev, when)) = m.get(&lane) {
            if prev == text && when.elapsed() < Duration::from_secs(15) {
                return Err("duplicate message suppressed (identical text to this lane within 15s). \
                     Don't resend — use wait_for_change to let it work, or read_agent to see where it's stuck."
                    .into());
            }
        }
        m.insert(lane, (text.to_string(), Instant::now()));
        Ok(())
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(autonomy: Autonomy, max_actions: u64) -> Policy {
        Policy {
            autonomy,
            max_concurrent_agents: 4,
            max_actions,
            actions: Mutex::new(0),
            last_send: Mutex::new(HashMap::new()),
        }
    }

    #[test]
    fn autonomy_parsing() {
        assert_eq!(Autonomy::parse("read-only"), Autonomy::ReadOnly);
        assert_eq!(Autonomy::parse("supervised"), Autonomy::Supervised);
        assert_eq!(Autonomy::parse("autonomous"), Autonomy::Autonomous);
        assert_eq!(Autonomy::parse("anything-else"), Autonomy::Autonomous);
        assert!(Autonomy::Autonomous.allows_create_lane());
        assert!(!Autonomy::Supervised.allows_create_lane());
        assert!(!Autonomy::ReadOnly.allows_mutation());
    }

    #[test]
    fn read_only_refuses_mutations() {
        assert!(policy(Autonomy::ReadOnly, 100).record_mutation().is_err());
    }

    #[test]
    fn action_cap_is_a_backstop() {
        let p = policy(Autonomy::Autonomous, 2);
        assert!(p.record_mutation().is_ok());
        assert!(p.record_mutation().is_ok());
        assert!(p.record_mutation().is_err()); // third exceeds the cap of 2
    }

    #[test]
    fn duplicate_sends_are_suppressed() {
        let p = policy(Autonomy::Autonomous, 100);
        assert!(p.check_send_dedupe(1, "go").is_ok());
        assert!(p.check_send_dedupe(1, "go").is_err()); // identical, same lane, within window
        assert!(p.check_send_dedupe(1, "different").is_ok()); // different text is fine
        assert!(p.check_send_dedupe(2, "go").is_ok()); // different lane is fine
    }
}
