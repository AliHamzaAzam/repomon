//! Server-enforced guardrails. These caps are enforced here, in the MCP layer, not merely
//! requested in the persona prompt - so a confused or runaway orchestrator physically cannot
//! exceed them. Configured from the environment by the `repomon orchestrate` launcher.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The `Lane::role` value marking the repomind home lane. Mirrors the daemon's
/// `repomon_daemon::repomind::CONTROLLER_ROLE`; the two are joined by the wire, not by a shared
/// type, so they are asserted equal in the daemon's tests.
pub const CONTROLLER_ROLE: &str = "controller";

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
    /// Whether the orchestrator may make structural changes to a repo's lanes itself - create,
    /// merge, or delete one - vs. asking the human first.
    pub fn allows_structural(self) -> bool {
        matches!(self, Autonomy::Autonomous)
    }
}

/// How long a minted confirmation token remains redeemable before [`Policy::take_confirm`]
/// refuses it as expired.
const CONFIRM_TTL: Duration = Duration::from_secs(600);

/// Bind a single-use human confirmation to the exact lane and action flags so approval cannot
/// authorize another target or variant.
struct PendingConfirm {
    lane_id: i64,
    flags: String,
    minted: Instant,
}

/// The runtime guardrails: a fixed configuration plus mutable counters.
pub struct Policy {
    pub autonomy: Autonomy,
    /// Set for headless standing/triage runs (`REPOMON_MCP_UNATTENDED=1`): merge_lane and
    /// delete_lane are refused outright regardless of autonomy - an unattended orchestrator
    /// reports and recommends, never lands or destroys work.
    pub unattended: bool,
    pub max_concurrent_agents: usize,
    pub max_actions: u64,
    actions: Mutex<u64>,
    last_send: Mutex<HashMap<i64, (String, Instant)>>,
    pending_confirms: Mutex<HashMap<String, PendingConfirm>>,
    confirm_ttl: Duration,
}

impl Policy {
    /// Read configuration from the environment (set by the launcher), with safe defaults.
    pub fn from_env() -> Policy {
        let autonomy = std::env::var("REPOMON_MCP_AUTONOMY")
            .map(|s| Autonomy::parse(&s))
            .unwrap_or(Autonomy::Autonomous);
        let max_concurrent_agents = env_usize("REPOMON_MCP_MAX_AGENTS", 4);
        let max_actions = env_usize("REPOMON_MCP_MAX_ACTIONS", 100) as u64;
        let unattended = std::env::var("REPOMON_MCP_UNATTENDED")
            .map(|v| matches!(v.trim(), "1" | "true" | "TRUE"))
            .unwrap_or(false);
        Policy {
            autonomy,
            unattended,
            max_concurrent_agents,
            max_actions,
            actions: Mutex::new(0),
            last_send: Mutex::new(HashMap::new()),
            pending_confirms: Mutex::new(HashMap::new()),
            confirm_ttl: CONFIRM_TTL,
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

    /// Rejects destructive controller-lane operations at every autonomy level before confirmation
    /// because that lane holds fleet memory.
    pub fn refuse_controller_lane(&self, role: Option<&str>, verb: &str) -> Result<(), String> {
        if role == Some(CONTROLLER_ROLE) {
            return Err(format!(
                "that is the controller lane (repomind's own home repo): {verb} it is never \
                 allowed. It holds the fleet's memory and the lane you are running in. If the \
                 human wants it gone, they remove it themselves outside the fleet tools."
            ));
        }
        Ok(())
    }

    /// Suppress an identical `send_to_agent` to the same lane within a short window - the
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

    /// Mints a single-use confirmation bound to the exact lane and action flags.
    pub fn mint_confirm(&self, lane_id: i64, flags: &str) -> String {
        let token = random_token();
        let mut m = self
            .pending_confirms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Prune expired confirmations even when callers only mint tokens, bounding pending state.
        m.retain(|_, p| p.minted.elapsed() < self.confirm_ttl);
        m.insert(
            token.clone(),
            PendingConfirm {
                lane_id,
                flags: flags.to_string(),
                minted: Instant::now(),
            },
        );
        token
    }

    /// Redeems an unexpired matching confirmation once, retaining it after mismatched attempts so a
    /// correct retry remains possible.
    pub fn take_confirm(&self, token: &str, lane_id: i64, flags: &str) -> Result<(), String> {
        let mut m = self
            .pending_confirms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = match m.get(token) {
            None => Err(
                "confirmation token not recognized (it may already have been used, \
                 never existed, or the server restarted) — re-run without confirm to get a \
                 fresh impact summary."
                    .to_string(),
            ),
            Some(p) if p.minted.elapsed() >= self.confirm_ttl => Err(
                "confirmation token expired — re-run without confirm to get a fresh impact \
                 summary."
                    .to_string(),
            ),
            Some(p) if p.lane_id != lane_id || p.flags != flags => Err(
                "confirmation token does not match this lane or action — tokens are single-use \
                 and bound to the exact request that minted them. Re-run without confirm to get \
                 a fresh impact summary."
                    .to_string(),
            ),
            Some(_) => Ok(()),
        };
        if result.is_ok() {
            m.remove(token);
        }
        // Opportunistic cleanup: sweep any other tokens that have expired while we hold the lock,
        // so a long-lived session's pending-confirm map doesn't grow unbounded.
        m.retain(|_, p| p.minted.elapsed() < self.confirm_ttl);
        result
    }
}

/// This token prevents accidental confirmation bypass, not adversarial guessing; per-process
/// entropy and a counter provide distinct inputs.
fn random_token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(nanos ^ counter);
    format!("{:08x}", hasher.finish() as u32)
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
            unattended: false,
            max_concurrent_agents: 4,
            max_actions,
            actions: Mutex::new(0),
            last_send: Mutex::new(HashMap::new()),
            pending_confirms: Mutex::new(HashMap::new()),
            confirm_ttl: CONFIRM_TTL,
        }
    }

    #[test]
    fn autonomy_parsing() {
        assert_eq!(Autonomy::parse("read-only"), Autonomy::ReadOnly);
        assert_eq!(Autonomy::parse("supervised"), Autonomy::Supervised);
        assert_eq!(Autonomy::parse("autonomous"), Autonomy::Autonomous);
        assert_eq!(Autonomy::parse("anything-else"), Autonomy::Autonomous);
        assert!(Autonomy::Autonomous.allows_structural());
        assert!(!Autonomy::Supervised.allows_structural());
        assert!(!Autonomy::ReadOnly.allows_mutation());
    }

    #[test]
    fn unattended_flag_defaults_off_in_test_policies() {
        assert!(!policy(Autonomy::Autonomous, 100).unattended);
    }

    #[test]
    fn read_only_refuses_mutations() {
        assert!(policy(Autonomy::ReadOnly, 100).record_mutation().is_err());
    }

    /// All structural operations must share the same autonomy gate.
    #[test]
    fn structural_gate_covers_create_merge_and_delete_lane() {
        assert!(!Autonomy::ReadOnly.allows_structural());
        assert!(!Autonomy::Supervised.allows_structural());
        assert!(Autonomy::Autonomous.allows_structural());
    }

    /// The controller lane is repomind's own home. Deleting or merging it would destroy the
    /// fleet's memory, so both are refused outright - before the two-phase confirm, and at every
    /// autonomy level.
    #[test]
    fn controller_lane_refuses_destructive_actions_outright() {
        let p = policy(Autonomy::Autonomous, 100);
        let err = p
            .refuse_controller_lane(Some("controller"), "deleting")
            .unwrap_err();
        assert!(err.contains("controller lane"), "unexpected message: {err}");
        assert!(err.contains("deleting"), "unexpected message: {err}");
        assert!(
            p.refuse_controller_lane(Some("controller"), "merging")
                .is_err()
        );
    }

    #[test]
    fn an_ordinary_lane_is_not_refused() {
        let p = policy(Autonomy::Autonomous, 100);
        assert!(p.refuse_controller_lane(None, "deleting").is_ok());
        assert!(p.refuse_controller_lane(Some("worker"), "merging").is_ok());
    }

    #[test]
    fn action_cap_is_a_backstop() {
        let p = policy(Autonomy::Autonomous, 2);
        assert!(p.record_mutation().is_ok());
        assert!(p.record_mutation().is_ok());
        assert!(p.record_mutation().is_err());
    }

    #[test]
    fn duplicate_sends_are_suppressed() {
        let p = policy(Autonomy::Autonomous, 100);
        assert!(p.check_send_dedupe(1, "go").is_ok());
        assert!(p.check_send_dedupe(1, "go").is_err());
        assert!(p.check_send_dedupe(1, "different").is_ok());
        assert!(p.check_send_dedupe(2, "go").is_ok());
    }

    fn policy_with_ttl(ttl: Duration) -> Policy {
        Policy {
            autonomy: Autonomy::Autonomous,
            unattended: false,
            max_concurrent_agents: 4,
            max_actions: 100,
            actions: Mutex::new(0),
            last_send: Mutex::new(HashMap::new()),
            pending_confirms: Mutex::new(HashMap::new()),
            confirm_ttl: ttl,
        }
    }

    #[test]
    fn confirm_mint_then_take_succeeds_exactly_once() {
        let p = policy(Autonomy::Autonomous, 100);
        let token = p.mint_confirm(7, "delete_branch=true");
        assert_eq!(token.len(), 8);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(p.take_confirm(&token, 7, "delete_branch=true").is_ok());
        // single-use: redeeming the same token again must fail (no replay).
        assert!(p.take_confirm(&token, 7, "delete_branch=true").is_err());
    }

    #[test]
    fn confirm_rejects_wrong_lane_or_flags_without_consuming_it() {
        let p = policy(Autonomy::Autonomous, 100);
        let token = p.mint_confirm(7, "delete_branch=true");
        // Wrong lane_id: a token minted for lane 7 must not confirm an action on lane 8.
        assert!(p.take_confirm(&token, 8, "delete_branch=true").is_err());
        // Wrong flags: a token minted for delete_branch=true must not confirm delete_branch=false.
        assert!(p.take_confirm(&token, 7, "delete_branch=false").is_err());
        // A mismatched attempt must not burn the token - the correct binding still works after.
        assert!(p.take_confirm(&token, 7, "delete_branch=true").is_ok());
    }

    #[test]
    fn confirm_rejects_unknown_and_malformed_tokens() {
        let p = policy(Autonomy::Autonomous, 100);
        assert!(p.take_confirm("deadbeef", 1, "").is_err());
        // An empty-string confirm (e.g. a malformed/omitted arg coerced to "") must never match.
        assert!(p.take_confirm("", 1, "").is_err());
    }

    #[test]
    fn confirm_tokens_do_not_cross_lanes() {
        // A token minted for one lane must never confirm a same-shaped action on another lane,
        // even with identical flags - this is the cross-lane-reuse bypass the caller must not have.
        let p = policy(Autonomy::Autonomous, 100);
        let token_a = p.mint_confirm(1, "delete_branch=false");
        let token_b = p.mint_confirm(2, "delete_branch=false");
        assert!(p.take_confirm(&token_a, 2, "delete_branch=false").is_err());
        assert!(p.take_confirm(&token_b, 1, "delete_branch=false").is_err());

        assert!(p.take_confirm(&token_a, 1, "delete_branch=false").is_ok());
        assert!(p.take_confirm(&token_b, 2, "delete_branch=false").is_ok());
    }

    #[test]
    fn confirm_expires_after_its_ttl() {
        let p = policy_with_ttl(Duration::from_millis(20));
        let token = p.mint_confirm(1, "x");
        std::thread::sleep(Duration::from_millis(60));
        let err = p.take_confirm(&token, 1, "x").unwrap_err();
        assert!(
            err.contains("expired"),
            "expected an expiry-specific message, got: {err}"
        );
    }

    #[test]
    fn confirm_does_not_expire_before_its_ttl() {
        let p = policy_with_ttl(Duration::from_secs(600));
        let token = p.mint_confirm(1, "x");
        assert!(p.take_confirm(&token, 1, "x").is_ok());
    }

    #[test]
    fn mint_confirm_sweeps_expired_entries() {
        // Mint-only callers must not accumulate expired confirmations indefinitely.
        let p = policy_with_ttl(Duration::from_millis(20));
        let _stale = p.mint_confirm(1, "delete_branch=false");
        std::thread::sleep(Duration::from_millis(60));
        // This mint should sweep the now-expired entry above rather than let it linger forever.
        let _fresh = p.mint_confirm(2, "delete_branch=false");
        let m = p
            .pending_confirms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(m.len(), 1, "expired entry should have been swept on mint");
    }
}
