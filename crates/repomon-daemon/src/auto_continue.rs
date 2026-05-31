//! Auto-continue agents that pause on a usage limit.
//!
//! Claude's transcript doesn't record usage-limit pauses, so we detect them by reading each
//! managed agent's tmux pane (`repomon_core::agent::detect_usage_limit`). When an agent is
//! blocked we schedule a resume — at the parsed reset time if known, else on a periodic retry —
//! and type the configured continue message (`continue` + Enter). The decision is a pure
//! function ([`decide`]) so the state machine is unit-tested; the loop only does the IO.
//!
//! Runs in the daemon regardless of whether a TUI is attached, so agents you left running get
//! resumed even with repomon closed. Only repomon-managed lanes (with a tmux window) are
//! touched — external sessions have no window to send keys to.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use repomon_core::agent::{detect_usage_limit, UsageLimit};
use repomon_core::model::LaneId;
use repomon_core::TmuxRuntime;

use crate::{pubsub, Ctx};

const TICK: Duration = Duration::from_secs(20);
const RETRY_MIN: i64 = 5; // retry cadence when the reset time is unknown / a send didn't take
const MAX_ATTEMPTS: u32 = 6; // after this many sends, give up and surface needs-you
const SEND_COOLDOWN_SECS: i64 = 90; // suppress re-detect of the stale on-screen message
const RESET_BUFFER_SECS: i64 = 60; // resume a little after the stated reset, never before

/// The public view of a lane's rate-limit pause, read by `overlay_agents` for the TUI.
#[derive(Debug, Clone)]
pub struct RateLimit {
    pub reset_at: Option<DateTime<Utc>>,
}

/// The watcher's private scheduling state for one lane. (The reset time itself lives in the
/// public [`RateLimit`] for the TUI; here we only track *when to act*.)
#[derive(Debug, Clone)]
struct Sched {
    next_attempt: DateTime<Utc>,
    attempts: u32,
    gave_up: bool,
    cooldown_until: Option<DateTime<Utc>>,
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    /// Start tracking a newly-detected pause.
    Track {
        reset_at: Option<DateTime<Utc>>,
        next_attempt: DateTime<Utc>,
    },
    /// Type the continue message now.
    Send,
    /// Out of attempts — stop and surface needs-you.
    GiveUp,
    /// The pause is gone — the agent resumed.
    Clear,
    Nothing,
}

fn schedule(reset_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> DateTime<Utc> {
    match reset_at {
        Some(r) => r + chrono::Duration::seconds(RESET_BUFFER_SECS),
        None => now + chrono::Duration::minutes(RETRY_MIN),
    }
}

/// Pure state-machine step: given the current schedule, the latest detection, whether
/// auto-continue is armed for this lane, and the clock, decide what to do.
fn decide(
    current: Option<&Sched>,
    detection: Option<&UsageLimit>,
    armed: bool,
    now: DateTime<Utc>,
) -> Action {
    if !armed {
        // Disabled (globally or for this lane): never send; revert any tracked pause to its
        // natural status so it shows as needs-you.
        return if current.is_some() {
            Action::Clear
        } else {
            Action::Nothing
        };
    }
    match detection {
        Some(lim) => match current {
            None => Action::Track {
                reset_at: lim.reset_at,
                next_attempt: schedule(lim.reset_at, now),
            },
            Some(s) => {
                if s.gave_up {
                    return Action::Nothing;
                }
                if s.cooldown_until.map(|c| now < c).unwrap_or(false) {
                    return Action::Nothing;
                }
                if now >= s.next_attempt {
                    if s.attempts + 1 > MAX_ATTEMPTS {
                        Action::GiveUp
                    } else {
                        Action::Send
                    }
                } else {
                    Action::Nothing
                }
            }
        },
        // No limit message on screen — if we were tracking one, the agent resumed.
        None => {
            if current.is_some() {
                Action::Clear
            } else {
                Action::Nothing
            }
        }
    }
}

/// Lane ids that currently have a managed agent window (`lane-<id>`).
fn managed_lanes(tmux: &TmuxRuntime) -> Vec<LaneId> {
    tmux.list_windows()
        .unwrap_or_default()
        .iter()
        .filter_map(|w| {
            w.strip_prefix("lane-")
                .and_then(|s| s.parse::<LaneId>().ok())
        })
        .collect()
}

/// Background loop: scan managed agents and auto-continue any paused on a usage limit.
pub async fn auto_continue_watcher(ctx: Arc<Ctx>) {
    let mut sched: HashMap<LaneId, Sched> = HashMap::new();
    let mut tick = tokio::time::interval(TICK);
    loop {
        tick.tick().await;

        let (global_on, message) = {
            let cfg = ctx.config.read().await;
            (cfg.auto_continue, cfg.auto_continue_message.clone())
        };

        let tmux = ctx.tmux.clone();
        let lanes = match tokio::task::spawn_blocking(move || managed_lanes(&tmux)).await {
            Ok(v) => v,
            Err(_) => continue,
        };
        sched.retain(|id, _| lanes.contains(id));
        {
            let mut rl = ctx.rate_limits.lock().await;
            rl.retain(|id, _| lanes.contains(id));
        }

        let off = ctx.auto_continue_off.lock().await.clone();
        let now = Utc::now();

        for lane in lanes {
            let tmuxc = ctx.tmux.clone();
            let pane =
                match tokio::task::spawn_blocking(move || tmuxc.capture(lane, Some(120))).await {
                    Ok(Ok(p)) => p,
                    _ => continue,
                };
            let detection = detect_usage_limit(&pane);
            let armed = global_on && !off.contains(&lane);
            let action = decide(sched.get(&lane), detection.as_ref(), armed, now);
            apply(&ctx, &mut sched, lane, action, &message, now).await;
        }
    }
}

/// Perform the IO for a decided action and update both the private schedule and the public
/// rate-limit view (which the TUI reads via `overlay_agents`).
async fn apply(
    ctx: &Arc<Ctx>,
    sched: &mut HashMap<LaneId, Sched>,
    lane: LaneId,
    action: Action,
    message: &str,
    now: DateTime<Utc>,
) {
    match action {
        Action::Track {
            reset_at,
            next_attempt,
        } => {
            sched.insert(
                lane,
                Sched {
                    next_attempt,
                    attempts: 0,
                    gave_up: false,
                    cooldown_until: None,
                },
            );
            ctx.rate_limits
                .lock()
                .await
                .insert(lane, RateLimit { reset_at });
            ctx.broadcast(
                pubsub::topic::AGENT_STATUS,
                serde_json::json!({ "lane_id": lane, "status": "rate-limited" }),
            );
        }
        Action::Send => {
            let tmux = ctx.tmux.clone();
            let msg = message.to_string();
            let _ = tokio::task::spawn_blocking(move || tmux.send_text(lane, &msg)).await;
            if let Some(s) = sched.get_mut(&lane) {
                s.attempts += 1;
                s.cooldown_until = Some(now + chrono::Duration::seconds(SEND_COOLDOWN_SECS));
                s.next_attempt = now + chrono::Duration::minutes(RETRY_MIN);
            }
        }
        Action::GiveUp => {
            if let Some(s) = sched.get_mut(&lane) {
                s.gave_up = true;
            }
            // Drop the public pause so the lane shows its natural needs-you for a human.
            ctx.rate_limits.lock().await.remove(&lane);
            ctx.broadcast(
                pubsub::topic::AGENT_STATUS,
                serde_json::json!({ "lane_id": lane, "status": "waiting" }),
            );
        }
        Action::Clear => {
            sched.remove(&lane);
            ctx.rate_limits.lock().await.remove(&lane);
            ctx.broadcast(
                pubsub::topic::AGENT_STATUS,
                serde_json::json!({ "lane_id": lane, "status": "running" }),
            );
        }
        Action::Nothing => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0).unwrap()
    }

    fn sched(next_in_secs: i64, attempts: u32, gave_up: bool, cooldown_in: Option<i64>) -> Sched {
        let n = now();
        Sched {
            next_attempt: n + chrono::Duration::seconds(next_in_secs),
            attempts,
            gave_up,
            cooldown_until: cooldown_in.map(|s| n + chrono::Duration::seconds(s)),
        }
    }

    #[test]
    fn disabled_lane_never_tracks_or_sends() {
        let lim = UsageLimit { reset_at: None };
        assert_eq!(decide(None, Some(&lim), false, now()), Action::Nothing);
    }

    #[test]
    fn disabled_mid_pause_reverts() {
        let s = sched(-10, 0, false, None);
        assert_eq!(decide(Some(&s), None, false, now()), Action::Clear);
    }

    #[test]
    fn new_detection_tracks_with_reset_buffer() {
        let reset = now() + chrono::Duration::hours(2);
        let lim = UsageLimit {
            reset_at: Some(reset),
        };
        let action = decide(None, Some(&lim), true, now());
        assert_eq!(
            action,
            Action::Track {
                reset_at: Some(reset),
                next_attempt: reset + chrono::Duration::seconds(RESET_BUFFER_SECS),
            }
        );
    }

    #[test]
    fn new_detection_without_time_uses_periodic_retry() {
        let lim = UsageLimit { reset_at: None };
        let action = decide(None, Some(&lim), true, now());
        assert_eq!(
            action,
            Action::Track {
                reset_at: None,
                next_attempt: now() + chrono::Duration::minutes(RETRY_MIN),
            }
        );
    }

    #[test]
    fn waits_until_next_attempt() {
        let s = sched(120, 0, false, None); // attempt is in the future
        let lim = UsageLimit { reset_at: None };
        assert_eq!(decide(Some(&s), Some(&lim), true, now()), Action::Nothing);
    }

    #[test]
    fn sends_when_due() {
        let s = sched(-1, 0, false, None);
        let lim = UsageLimit { reset_at: None };
        assert_eq!(decide(Some(&s), Some(&lim), true, now()), Action::Send);
    }

    #[test]
    fn cooldown_suppresses_send() {
        let s = sched(-1, 1, false, Some(60)); // due, but cooling down
        let lim = UsageLimit { reset_at: None };
        assert_eq!(decide(Some(&s), Some(&lim), true, now()), Action::Nothing);
    }

    #[test]
    fn gives_up_after_max_attempts() {
        let s = sched(-1, MAX_ATTEMPTS, false, None);
        let lim = UsageLimit { reset_at: None };
        assert_eq!(decide(Some(&s), Some(&lim), true, now()), Action::GiveUp);
    }

    #[test]
    fn gave_up_stays_quiet() {
        let s = sched(-1, MAX_ATTEMPTS, true, None);
        let lim = UsageLimit { reset_at: None };
        assert_eq!(decide(Some(&s), Some(&lim), true, now()), Action::Nothing);
    }

    #[test]
    fn clears_when_message_gone() {
        let s = sched(-1, 1, false, None);
        assert_eq!(decide(Some(&s), None, true, now()), Action::Clear);
    }
}
