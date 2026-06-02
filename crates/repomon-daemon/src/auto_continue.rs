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
const RETRY_MIN: i64 = 5; // retry cadence after a known reset time, or when a send didn't take
const UNKNOWN_RETRY_MIN: i64 = 20; // coarse retry when no reset time is known (don't spam)
const GIVE_UP_AFTER_HOURS: i64 = 6; // stop this long after first detecting the pause (>5h window)
const SEND_COOLDOWN_SECS: i64 = 90; // suppress re-detect of the stale on-screen message
const RESET_BUFFER_SECS: i64 = 60; // resume a little after the stated reset, never before

/// The public view of a lane's rate-limit pause, read by `overlay_agents` for the TUI.
#[derive(Debug, Clone)]
pub struct RateLimit {
    pub reset_at: Option<DateTime<Utc>>,
}

/// The watcher's private scheduling state for one lane.
#[derive(Debug, Clone)]
struct Sched {
    /// When the pause was first detected — gives the wall-clock give-up horizon.
    started: DateTime<Utc>,
    /// The parsed reset time, if any (drives the retry cadence: precise vs coarse).
    reset_at: Option<DateTime<Utc>>,
    next_attempt: DateTime<Utc>,
    gave_up: bool,
    cooldown_until: Option<DateTime<Utc>>,
    /// We've already pressed Enter on Claude's "Stop and wait for limit to reset" menu for this
    /// pause, so don't press it again (reset only when the lane clears).
    menu_confirmed: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    /// Start tracking a newly-detected pause.
    Track {
        reset_at: Option<DateTime<Utc>>,
        next_attempt: DateTime<Utc>,
    },
    /// Press Enter to pick option 1 ("Stop and wait for limit to reset") on the interactive menu.
    ChooseWait,
    /// Type the continue message now.
    Send,
    /// Waited too long without resuming — stop and surface needs-you.
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
                // Give up on a wall-clock horizon, not an attempt count: when no reset time is
                // shown we must keep waiting through the multi-hour window without quitting early.
                if now - s.started > chrono::Duration::hours(GIVE_UP_AFTER_HOURS) {
                    return Action::GiveUp;
                }
                if s.cooldown_until.map(|c| now < c).unwrap_or(false) {
                    return Action::Nothing;
                }
                // Pick "Stop and wait for limit to reset" once, before ever typing `continue` —
                // otherwise the continue text would land in the menu.
                if lim.menu && !s.menu_confirmed {
                    return Action::ChooseWait;
                }
                if now >= s.next_attempt {
                    Action::Send
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
                    started: now,
                    reset_at,
                    next_attempt,
                    gave_up: false,
                    cooldown_until: None,
                    menu_confirmed: false,
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
        Action::ChooseWait => {
            // Press Enter to confirm the menu's default option 1 ("Stop and wait …").
            let tmux = ctx.tmux.clone();
            let _ = tokio::task::spawn_blocking(move || tmux.send_key(lane, "Enter")).await;
            if let Some(s) = sched.get_mut(&lane) {
                s.menu_confirmed = true;
                s.cooldown_until = Some(now + chrono::Duration::seconds(SEND_COOLDOWN_SECS));
            }
        }
        Action::Send => {
            let tmux = ctx.tmux.clone();
            let msg = message.to_string();
            let _ = tokio::task::spawn_blocking(move || tmux.send_text(lane, &msg)).await;
            if let Some(s) = sched.get_mut(&lane) {
                // Retry sooner when we know the reset time; coarsely when we're guessing.
                let cadence = if s.reset_at.is_some() {
                    RETRY_MIN
                } else {
                    UNKNOWN_RETRY_MIN
                };
                s.cooldown_until = Some(now + chrono::Duration::seconds(SEND_COOLDOWN_SECS));
                s.next_attempt = now + chrono::Duration::minutes(cadence);
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

    fn sched(next_in_secs: i64, gave_up: bool, cooldown_in: Option<i64>) -> Sched {
        let n = now();
        Sched {
            started: n,
            reset_at: None,
            next_attempt: n + chrono::Duration::seconds(next_in_secs),
            gave_up,
            cooldown_until: cooldown_in.map(|s| n + chrono::Duration::seconds(s)),
            menu_confirmed: true, // default: menu already handled, so tests reach the Send path
        }
    }

    fn lim(reset_at: Option<DateTime<Utc>>, menu: bool) -> UsageLimit {
        UsageLimit { reset_at, menu }
    }

    #[test]
    fn disabled_lane_never_tracks_or_sends() {
        assert_eq!(
            decide(None, Some(&lim(None, false)), false, now()),
            Action::Nothing
        );
    }

    #[test]
    fn disabled_mid_pause_reverts() {
        let s = sched(-10, false, None);
        assert_eq!(decide(Some(&s), None, false, now()), Action::Clear);
    }

    #[test]
    fn new_detection_tracks_with_reset_buffer() {
        let reset = now() + chrono::Duration::hours(2);
        let action = decide(None, Some(&lim(Some(reset), false)), true, now());
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
        let action = decide(None, Some(&lim(None, false)), true, now());
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
        let s = sched(120, false, None); // attempt is in the future
        assert_eq!(
            decide(Some(&s), Some(&lim(None, false)), true, now()),
            Action::Nothing
        );
    }

    #[test]
    fn sends_when_due() {
        let s = sched(-1, false, None);
        assert_eq!(
            decide(Some(&s), Some(&lim(None, false)), true, now()),
            Action::Send
        );
    }

    #[test]
    fn cooldown_suppresses_send() {
        let s = sched(-1, false, Some(60)); // due, but cooling down
        assert_eq!(
            decide(Some(&s), Some(&lim(None, false)), true, now()),
            Action::Nothing
        );
    }

    #[test]
    fn gives_up_after_long_wait() {
        let mut s = sched(-1, false, None);
        s.started = now() - chrono::Duration::hours(GIVE_UP_AFTER_HOURS + 1);
        assert_eq!(
            decide(Some(&s), Some(&lim(None, false)), true, now()),
            Action::GiveUp
        );
    }

    #[test]
    fn gave_up_stays_quiet() {
        let s = sched(-1, true, None);
        assert_eq!(
            decide(Some(&s), Some(&lim(None, false)), true, now()),
            Action::Nothing
        );
    }

    #[test]
    fn clears_when_message_gone() {
        let s = sched(-1, false, None);
        assert_eq!(decide(Some(&s), None, true, now()), Action::Clear);
    }

    #[test]
    fn confirms_menu_before_continue() {
        // The interactive menu is up and we haven't chosen yet: pick option 1, don't type
        // `continue` — even though a send is otherwise due.
        let mut s = sched(-1, false, None);
        s.menu_confirmed = false;
        assert_eq!(
            decide(Some(&s), Some(&lim(None, true)), true, now()),
            Action::ChooseWait
        );
    }

    #[test]
    fn does_not_reconfirm_menu_once_chosen() {
        // Menu text still on screen but already confirmed → proceed to send `continue`.
        let s = sched(-1, false, None); // menu_confirmed: true by default
        assert_eq!(
            decide(Some(&s), Some(&lim(None, true)), true, now()),
            Action::Send
        );
    }
}
