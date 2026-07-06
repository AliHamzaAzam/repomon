//! Daemon-side notification engine, for clients that aren't on this machine.
//!
//! The TUI does its own edge detection for local popups; remote clients (the iOS companion)
//! can't — they may not even be connected when an agent starts waiting. So the daemon runs the
//! same shared detection (`repomon_core::notify`) over the lane list and, on each meaningful
//! transition, broadcasts an `event.notification` to subscribed clients (and, with push
//! configured, sends APNs — see `push`). Self-gating: each tick re-reads the config and does
//! nothing unless the remote bridge is enabled, so the watcher costs nothing for TUI-only use
//! and reacts live when `[remote]` is switched on.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use repomon_core::Config;
use repomon_core::agent;
use repomon_core::model::{AgentStatus, LaneId};
use repomon_core::notify::{
    NotifKind, SessKey, SessState, activity_allows_refire, compose, diff_session_transitions,
    session_by_key, session_statuses, slot_by_key,
};
use serde_json::json;

use crate::{Ctx, ORCHESTRATOR_WINDOW, push, rpc};

/// How often the watcher re-reads the fleet for remote/push notifications. Each tick recomputes
/// the overlay, but the overlay's own caches absorb most of the cost: the composite snapshot is
/// reused for `OVERLAY_TTL` (~750ms), the `lsof`/`pgrep` process probe for ~10s, and each pane
/// sniff for ~20s. So a tick that only re-reads warm caches is cheap, and a 2s cadence cuts the
/// old 8s worst-case alert latency to ~2s (the daemon owns *all* remote delivery and the local
/// desktop popup whenever the TUI is parked/closed) without pegging a core.
const TICK: Duration = Duration::from_secs(2);
/// Don't re-fire the same session's notification within this window (status flapping).
const DEBOUNCE: Duration = Duration::from_secs(30);
/// How long to keep an alert's activity latch after its session leaves the snapshot, so a
/// vanish+reappear (an `lsof` undercount, the 6h recency gate, `claude --resume` churn) can't slip
/// a repeat through the gap. Covers the longest flap window — a multi-hour usage-limit pause —
/// comfortably; a transcript gone longer than this can't re-enter under the same id anyway.
const LATCH_GRACE: Duration = Duration::from_secs(6 * 60 * 60);
/// How long since the local TUI's last request before we treat it as parked (attached) or closed
/// and let the daemon fire desktop popups itself. The TUI refreshes ~1s, so a few seconds of
/// silence means it isn't watching.
const LOCAL_TTL: Duration = Duration::from_secs(3);

pub async fn notify_watch(ctx: Arc<Ctx>) {
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut prev: HashMap<(LaneId, SessKey), SessState> = HashMap::new();
    let mut seeded = false;
    let mut debounce: HashMap<(LaneId, SessKey, NotifKind), Instant> = HashMap::new();
    // Activity-anchored re-fire latch: the session's `last_activity_at` (transcript mtime) at the
    // moment each (lane, session, kind) last fired. A repeat is allowed only once that advances —
    // i.e. the agent did real work since — so status flapping can't re-alert (see
    // `activity_allows_refire`). Applies to NeedsYou/RateLimited/Resumed; Idle stays on `debounce`.
    let mut latch: HashMap<(LaneId, SessKey, NotifKind), (DateTime<Utc>, Instant)> = HashMap::new();
    // The subagent-inclusion setting the current `prev` snapshot was built with. When it flips,
    // the set of tracked keys changes wholesale (inferred sessions appear/vanish), so we re-seed
    // rather than diff — otherwise toggling it off would fire a spurious Idle for every subagent.
    let mut prev_subagents = false;
    // Orchestrator attention state, carried across ticks (see `check_orchestrator_attention`):
    // toggles the $HOME transcript scan to every other tick, caches its last result for the
    // skipped tick, and debounces the orchestrator's own "needs you" desktop popup.
    let mut orch_scan_tick = false;
    let mut orch_transcript: Option<(AgentStatus, Option<String>)> = None;
    let mut orch_popup_fired: Option<Instant> = None;

    loop {
        tick.tick().await;
        let cfg = ctx.config.read().await.clone();
        // The TUI fires its own desktop popups while it's actively watching; the daemon takes over
        // local desktop delivery only when the TUI has parked in an attach or closed — i.e. its
        // ~1s lane.list heartbeat has gone stale. Remote delivery is gated separately below.
        let tui_active =
            (*ctx.local_watcher_seen.lock().await).is_some_and(|t| t.elapsed() < LOCAL_TTL);

        // Runs unconditionally — even with notifications disabled — because the TUI's pinned row
        // and command-center header need repomind's attention live regardless; only the desktop
        // popup inside this call is gated on `cfg.notify_enabled`. Must stay ABOVE the
        // notify_enabled early-continue below.
        check_orchestrator_attention(
            &ctx,
            &cfg,
            tui_active,
            &mut orch_scan_tick,
            &mut orch_transcript,
            &mut orch_popup_fired,
        )
        .await;

        if !cfg.notify_enabled {
            // Drop state while disabled so re-enabling re-seeds instead of firing a backlog.
            prev.clear();
            seeded = false;
            debounce.clear();
            latch.clear();
            continue;
        }

        // Always recompute (bypass the lane.list cache): edge detection must never reuse a stale
        // snapshot, and in a headless setup nothing else populates the cache.
        let Ok(lanes) = rpc::lanes_with_agents_fresh(&ctx).await else {
            continue;
        };
        let subagents = cfg.notify_subagents;
        let now: HashMap<(LaneId, SessKey), SessState> = lanes
            .iter()
            .flat_map(|l| session_statuses(l.id, &l.agent_sessions, subagents))
            .collect();
        if !seeded || subagents != prev_subagents {
            prev = now;
            seeded = true;
            prev_subagents = subagents;
            continue;
        }

        let live: HashSet<LaneId> = lanes.iter().map(|l| l.id).collect();
        let managed: HashSet<LaneId> = lanes
            .iter()
            .filter(|l| l.agent_sessions.iter().any(|s| !s.external && !s.inferred))
            .map(|l| l.id)
            .collect();

        let mut fires: Vec<((LaneId, SessKey), NotifKind)> = Vec::new();
        for (key, kind) in diff_session_transitions(&prev, &now, &live, &managed) {
            if !kind_enabled(&cfg, kind) {
                continue;
            }
            let dkey = (key.0, key.1.clone(), kind);
            if debounce.get(&dkey).is_some_and(|t| t.elapsed() < DEBOUNCE) {
                continue;
            }
            // Activity latch: suppress a repeat of this alert unless the session's transcript has
            // advanced since it last fired. Defeats the status flapping (idle-decay, lsof
            // undercount, sniff wobble) that the time-debounce can't. Idle has no activity anchor
            // (it fires on disappearance), so it stays on the debounce alone.
            let activity = lanes
                .iter()
                .find(|l| l.id == key.0)
                .and_then(|l| session_by_key(l, &key.1, subagents))
                .map(|s| s.last_activity_at);
            let prev_fired = latch.get(&dkey).map(|(t, _)| *t);
            if kind != NotifKind::Idle && !activity_allows_refire(prev_fired, activity) {
                continue;
            }
            // Diagnostic for the "repeats an alert I already handled" report: a re-fire is only
            // legitimate when the transcript advanced since last time (current_activity > prev_fired).
            // If these logs show a re-fire with current_activity <= prev_fired (or prev_fired None
            // for a session that clearly fired before), the latch is being bypassed.
            if kind == NotifKind::NeedsYou {
                tracing::info!(
                    lane = key.0,
                    session = ?key.1,
                    prev_fired = ?prev_fired,
                    current_activity = ?activity,
                    "notify: NeedsYou firing"
                );
            }
            debounce.insert(dkey.clone(), Instant::now());
            if kind != NotifKind::Idle {
                if let Some(a) = activity {
                    latch.insert(dkey, (a, Instant::now()));
                }
            }
            fires.push((key, kind));
        }
        prev = now;
        let snapshot = &prev;
        debounce.retain(|(lane, sess, _), t| {
            snapshot.contains_key(&(*lane, sess.clone())) || t.elapsed() < DEBOUNCE
        });
        // Keep latch entries through a vanish+reappear (that's the repeat we're stopping); only
        // drop one once its session has been gone longer than it could plausibly return.
        latch.retain(|(lane, sess, _), (_, seen)| {
            snapshot.contains_key(&(*lane, sess.clone())) || seen.elapsed() < LATCH_GRACE
        });

        for ((lane_id, key), kind) in fires {
            let Some(lane) = lanes.iter().find(|l| l.id == lane_id) else {
                continue;
            };
            let sess = session_by_key(lane, &key, subagents);
            let (title, body) = compose(
                kind,
                lane,
                sess,
                slot_by_key(lane, &key, subagents),
                cfg.notify_show_why,
            );
            // The actual on-screen dialog, when there is one — what a push's Approve acts on.
            let dialog = sess.and_then(|s| s.pending_prompt.clone());
            // The payload's "prompt" falls back to the agent's last message for context.
            let prompt = dialog
                .clone()
                .or_else(|| sess.and_then(|s| s.last_message.clone()));
            // Stable dedup id: a genuine re-alert advances the session's activity and so gets a new
            // id, but a flapped re-send (same lane/session/kind, same activity) repeats the id — so
            // a client that briefly reconnects or APNs that double-delivers can drop the duplicate.
            let session_id = sess.and_then(|s| s.session_id.clone());
            let activity_epoch = sess.map(|s| s.last_activity_at.timestamp()).unwrap_or(0);
            let dedup_id = format!(
                "{lane_id}:{}:{}:{activity_epoch}",
                session_id.as_deref().unwrap_or("-"),
                kind.slug(),
            );
            // Finer-than-kind taxonomy for clients: permission / decision / end_of_turn / none.
            let attention = sess
                .map(|s| repomon_core::agent::attention::agent_attention(s).as_str())
                .unwrap_or("none");
            let payload = json!({
                "id": dedup_id,
                "lane_id": lane_id,
                "session_id": session_id,
                "kind": kind,
                "title": title,
                "body": body,
                "prompt": prompt,
                "attention": attention,
                "dialog": sess.and_then(|s| s.pending_dialog.clone()),
            });
            // Remote clients (the iOS companion): event.notification + APNs — only when the bridge
            // is enabled.
            if cfg.remote.enabled {
                ctx.broadcast("event.notification", payload.clone());
                // Lock-screen push: a NeedsYou with a pending question gets the actionable
                // category (Approve / Open); everything else is a plain alert. Approve-from-lock
                // only when an actual dialog is up — a plain "finished its turn" Enter would be a
                // no-op (or worse, submit an empty reply).
                let category = if kind == NotifKind::NeedsYou && dialog.is_some() {
                    push::CATEGORY_PROMPT
                } else {
                    push::CATEGORY_ALERT
                };
                push::send_all(&ctx, &title, &body, category, &payload).await;
            }

            // Local desktop popup — fired by the daemon only when the TUI isn't already covering it
            // (it's parked in an attach or closed), so we never double-notify with the TUI's own.
            if !tui_active {
                repomon_core::notify::send_native(
                    &title,
                    &body,
                    cfg.notify_sound,
                    cfg.notify_click_focus,
                );
            }
        }
    }
}

/// Whether this notification kind is enabled (master switch checked by the caller).
fn kind_enabled(cfg: &Config, kind: NotifKind) -> bool {
    match kind {
        NotifKind::NeedsYou => cfg.notify_needs_you,
        NotifKind::RateLimited => cfg.notify_rate_limited,
        NotifKind::Resumed => cfg.notify_resumed,
        NotifKind::Idle => cfg.notify_idle,
        // A stall is a needs-you-class event (the agent is blocked and only you can unblock
        // it), so it rides that toggle rather than growing its own setting.
        NotifKind::Stalled => cfg.notify_needs_you,
    }
}

// ---- repomind orchestrator attention (B4: the human<->repomind escalation loop) ----

/// Don't re-fire the orchestrator's own "needs you" desktop popup within this window — separate
/// from the per-session `DEBOUNCE` above, since this is a single pane, not a fleet of sessions.
const ORCH_POPUP_DEBOUNCE: Duration = Duration::from_secs(30);
/// How far back to capture the orchestrator's pane for the pending-dialog sniff (mirrors the
/// managed-agent prompt sniff in `rpc::overlay_agents`).
const ORCH_CAPTURE_LINES: u32 = 45;
/// Cap on the end-of-turn headline's length (a tail of repomind's last message).
const ORCH_HEADLINE_LEN: usize = 140;

/// Fold the repomind orchestrator's attention into this tick: a pending pane dialog (permission /
/// decision) or an end-of-turn message beats "none". Runs on every tick regardless of
/// `cfg.notify_enabled` — the TUI's pinned row and command-center header need it live even with
/// notifications off — but the desktop popup fired on the none→attention edge below IS gated on
/// `cfg.notify_enabled && cfg.notify_needs_you`, mirroring `kind_enabled`'s gating of the
/// per-session NeedsYou popup above (this is the same escalation, just for the orchestrator's own
/// pane rather than a managed agent's).
///
/// `scan_transcript`/`transcript_cache` throttle the `$HOME` transcript scan (a directory walk) to
/// every other tick a dialog isn't already covering the answer; `popup_fired` debounces the popup.
async fn check_orchestrator_attention(
    ctx: &Ctx,
    cfg: &Config,
    tui_active: bool,
    scan_transcript: &mut bool,
    transcript_cache: &mut Option<(AgentStatus, Option<String>)>,
    popup_fired: &mut Option<Instant>,
) {
    let alive = rpc::reconcile_orchestrator(ctx).await;
    let (word, headline) = if !alive {
        *transcript_cache = None; // no session: drop any stale cached transcript status
        ("none", None)
    } else {
        // Pin the transcript scan to the orchestrator's own session id (captured at spawn via
        // `--session-id`) — the `ctx.orchestrator` state `reconcile_orchestrator` just confirmed
        // is alive — so it never picks up some other active Claude session's transcript. See
        // `rpc::pick_orchestrator_transcript`. `has_transcript` gates the scan entirely: a
        // backend with no parseable transcript (codex) must NOT reach the picker at all — with
        // its always-`None` session id the picker would fall back to the "newest `~/.claude`
        // transcript with content" heuristic and misattribute another live Claude session.
        let (session_id, has_transcript) = {
            let orch = ctx.orchestrator.lock().await;
            let o = orch.as_ref();
            (
                o.and_then(|o| o.session_id.clone()),
                // `None` (stopped between the reconcile above and here) also means "don't scan".
                o.is_some_and(|o| o.backend.has_transcript()),
            )
        };
        if !has_transcript {
            // Also drop any cached status a prior Claude-backed session left behind, so it can't
            // leak an end_of_turn into this one.
            *transcript_cache = None;
        }
        let tmux = ctx.tmux.clone();
        let pane = tokio::task::spawn_blocking(move || {
            tmux.capture_named(ORCHESTRATOR_WINDOW, Some(ORCH_CAPTURE_LINES))
        })
        .await
        .ok()
        .and_then(|r| r.ok());
        let dialog = pane
            .as_deref()
            .and_then(agent::prompt::detect_pending_prompt);

        *scan_transcript = !*scan_transcript;
        if has_transcript && dialog.is_none() && *scan_transcript {
            *transcript_cache = tokio::task::spawn_blocking(move || {
                rpc::pick_orchestrator_transcript(session_id.as_deref())
            })
            .await
            .ok()
            .flatten()
            .map(|s| (s.status, s.last_message));
        }
        derive_attention(dialog.as_deref(), transcript_cache.clone())
    };

    let mut slot = ctx.orchestrator_attention.lock().await;
    if slot.0 == word && slot.1 == headline {
        return;
    }
    let edge_to_attention = slot.0 == "none" && word != "none";
    *slot = (word.to_string(), headline.clone());
    drop(slot);

    let orch = ctx.orchestrator.lock().await;
    let status = rpc::orchestrator_status_value(orch.as_ref(), word, headline.as_deref());
    drop(orch);
    ctx.broadcast(crate::pubsub::topic::ORCHESTRATOR_STATUS, status);

    if edge_to_attention && !tui_active && cfg.notify_enabled && cfg.notify_needs_you {
        let due = popup_fired
            .map(|t| t.elapsed() >= ORCH_POPUP_DEBOUNCE)
            .unwrap_or(true);
        if due {
            *popup_fired = Some(Instant::now());
            repomon_core::notify::send_native(
                "repomind needs you",
                headline.as_deref().unwrap_or(""),
                cfg.notify_sound,
                cfg.notify_click_focus,
            );
        }
    }
}

/// Map the orchestrator's pane dialog (if any — already detected/classified by
/// `repomon_core::agent::prompt`, which is fixture-tested there) and its transcript status to an
/// attention word + headline. Pure, so *this* mapping — dialog → permission/decision, `Waiting` →
/// end_of_turn, else none — is unit-testable without tmux or a real transcript.
fn derive_attention(
    dialog: Option<&str>,
    transcript: Option<(AgentStatus, Option<String>)>,
) -> (&'static str, Option<String>) {
    if let Some(summary) = dialog {
        let word = match agent::prompt::classify_prompt(summary) {
            agent::prompt::PromptClass::Permission => "permission",
            agent::prompt::PromptClass::Decision => "decision",
        };
        return (word, Some(summary.to_string()));
    }
    match transcript {
        Some((AgentStatus::Waiting, last_message)) => (
            "end_of_turn",
            last_message.map(|m| tail(&m, ORCH_HEADLINE_LEN)),
        ),
        _ => ("none", None),
    }
}

/// The tail of a message, trimmed and capped at `max` chars — likelier than the opening line to
/// hold repomind's actual question when a turn ends on a long response.
fn tail(s: &str, max: usize) -> String {
    let s = s.trim();
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let start = count - max;
    let clipped: String = s.chars().skip(start).collect();
    format!("…{}", clipped.trim_start())
}

#[cfg(test)]
mod attention_tests {
    use super::*;

    #[test]
    fn permission_dialog_maps_to_permission() {
        let (word, headline) = derive_attention(Some("Do you want to proceed?"), None);
        assert_eq!(word, "permission");
        assert_eq!(headline.as_deref(), Some("Do you want to proceed?"));
    }

    #[test]
    fn open_question_dialog_maps_to_decision() {
        let (word, headline) = derive_attention(Some("Which auth method should we use?"), None);
        assert_eq!(word, "decision");
        assert_eq!(
            headline.as_deref(),
            Some("Which auth method should we use?")
        );
    }

    #[test]
    fn a_dialog_wins_even_over_a_waiting_transcript() {
        // The pane dialog is the more precise signal — it beats a stale/lagging transcript scan.
        let (word, _) = derive_attention(
            Some("Do you trust the files in this folder?"),
            Some((AgentStatus::Waiting, Some("some prior message".into()))),
        );
        assert_eq!(word, "permission");
    }

    #[test]
    fn waiting_transcript_with_no_dialog_maps_to_end_of_turn() {
        let (word, headline) =
            derive_attention(None, Some((AgentStatus::Waiting, Some("all done!".into()))));
        assert_eq!(word, "end_of_turn");
        assert_eq!(headline.as_deref(), Some("all done!"));
    }

    #[test]
    fn waiting_transcript_with_no_message_has_no_headline() {
        let (word, headline) = derive_attention(None, Some((AgentStatus::Waiting, None)));
        assert_eq!(word, "end_of_turn");
        assert_eq!(headline, None);
    }

    #[test]
    fn running_or_idle_transcript_and_no_dialog_is_none() {
        assert_eq!(
            derive_attention(None, Some((AgentStatus::Running, Some("mid-turn".into())))).0,
            "none"
        );
        assert_eq!(derive_attention(None, None).0, "none");
    }

    #[test]
    fn long_headline_truncates_to_a_tail() {
        let msg = format!("{}the important bit at the end", "x".repeat(200));
        let (word, headline) = derive_attention(None, Some((AgentStatus::Waiting, Some(msg))));
        assert_eq!(word, "end_of_turn");
        let h = headline.unwrap();
        assert!(h.ends_with("the important bit at the end"));
        assert!(h.starts_with('…'));
        assert!(h.chars().count() <= ORCH_HEADLINE_LEN + 1);
    }

    #[test]
    fn short_message_tail_is_unchanged() {
        assert_eq!(tail("hello", 140), "hello");
        assert_eq!(tail("  padded  ", 140), "padded");
    }
}
