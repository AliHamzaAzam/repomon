//! Server-enforced guardrails. These caps are enforced here, in the MCP layer, not merely
//! requested in the persona prompt — so a confused or runaway orchestrator physically cannot
//! exceed them. Configured from the environment by the `repomon orchestrate` launcher.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// Whether the orchestrator may make structural changes to a repo's lanes itself — create,
    /// merge, or delete one — vs. asking the human first.
    pub fn allows_structural(self) -> bool {
        matches!(self, Autonomy::Autonomous)
    }
}

/// How long a minted confirmation token remains redeemable before [`Policy::take_confirm`]
/// refuses it as expired.
const CONFIRM_TTL: Duration = Duration::from_secs(600);

/// A minted two-phase confirmation awaiting the human-approved second call. Single-use and bound
/// to the exact `(lane_id, flags)` it was minted for, so it can't confirm a different lane or a
/// different variant of the same action (e.g. a plain delete vs. one that also deletes the
/// branch).
struct PendingConfirm {
    lane_id: i64,
    flags: String,
    minted: Instant,
}

/// The runtime guardrails: a fixed configuration plus mutable counters.
pub struct Policy {
    pub autonomy: Autonomy,
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
        Policy {
            autonomy,
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

    /// Mint a single-use confirmation token for a destructive tool's two-phase confirm flow,
    /// bound to `(lane_id, flags)`. `flags` is a caller-chosen discriminator string (e.g.
    /// "delete_branch=true") distinguishing variants of the same action so a token minted for
    /// one variant can't confirm another.
    pub fn mint_confirm(&self, lane_id: i64, flags: &str) -> String {
        let token = random_token();
        let mut m = self
            .pending_confirms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Opportunistic cleanup: sweep any expired tokens while we hold the lock, same as
        // take_confirm does — otherwise a caller that only ever runs phase 1 (mints an impact
        // summary but never confirms) can grow pending_confirms unbounded over a long session.
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

    /// Redeem a confirmation token minted by [`Self::mint_confirm`]. Single-use: removed from the
    /// pending set only on success, so a mismatched or expired attempt doesn't burn a token the
    /// caller could otherwise still retry correctly. Must match the exact `lane_id` and `flags`
    /// it was minted for, and must be redeemed within `confirm_ttl`. Error messages are written
    /// to tell the calling LLM exactly what to do next.
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

/// An 8-hex-char token for `mint_confirm`/`take_confirm`. This is not cryptographically secure —
/// the threat model is a well-behaved-but-confused orchestrator LLM that must not be able to
/// fabricate a token to skip straight to a destructive call, not an adversary brute-forcing the
/// token space — so std's per-process hasher entropy plus a monotonic counter (guaranteeing every
/// call sees a distinct input) is sufficient. No `rand` dependency is in this crate's tree, so we
/// deliberately don't add one for this.
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
    fn read_only_refuses_mutations() {
        assert!(policy(Autonomy::ReadOnly, 100).record_mutation().is_err());
    }

    /// create_lane, merge_lane, and delete_lane all gate on `allows_structural()` — the single
    /// source of truth their handlers share in server.rs. Handler-level tests for that gate
    /// aren't feasible without a running daemon, so this exercises the shared predicate directly
    /// for every autonomy level, standing in for all three call sites (including delete_lane's,
    /// which was added as defense-in-depth on top of its two-phase confirm).
    #[test]
    fn structural_gate_covers_create_merge_and_delete_lane() {
        assert!(!Autonomy::ReadOnly.allows_structural());
        assert!(!Autonomy::Supervised.allows_structural());
        assert!(Autonomy::Autonomous.allows_structural());
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

    fn policy_with_ttl(ttl: Duration) -> Policy {
        Policy {
            autonomy: Autonomy::Autonomous,
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
        // A mismatched attempt must not burn the token — the correct binding still works after.
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
        // even with identical flags — this is the cross-lane-reuse bypass the caller must not have.
        let p = policy(Autonomy::Autonomous, 100);
        let token_a = p.mint_confirm(1, "delete_branch=false");
        let token_b = p.mint_confirm(2, "delete_branch=false");
        assert!(p.take_confirm(&token_a, 2, "delete_branch=false").is_err());
        assert!(p.take_confirm(&token_b, 1, "delete_branch=false").is_err());
        // Each token still works for its own lane.
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
        // A caller that only ever runs delete_lane's phase 1 (mints an impact summary, never
        // confirms — e.g. a confused orchestrator looping) must not grow pending_confirms
        // unbounded. mint_confirm sweeps expired entries opportunistically, same as take_confirm.
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
