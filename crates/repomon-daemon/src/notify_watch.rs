//! Daemon-side notification engine for every subscribed client.
//!
//! The TUI does its own edge detection for local popups, while desktop and remote clients consume
//! `event.notification`. The daemon runs the shared detection (`repomon_core::notify`) over the
//! lane list and broadcasts every meaningful transition. APNs remains optional and remote-gated.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use repomon_core::Config;
use repomon_core::agent;
use repomon_core::agent::backend::CaptureOpts;
use repomon_core::agent::supervision::{DialogClass, PolicySource};
use repomon_core::model::{AgentSession, AgentStatus, Lane, LaneId};
use repomon_core::notify::{
    NotifKind, SessKey, SessState, activity_allows_refire, compose, diff_session_transitions,
    session_by_key, session_statuses, slot_by_key,
};
use serde_json::json;

use crate::inject::{self, AuditSeed, Expectation, Payload, SendOutcome};
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
    // Needs-you edges awaiting a triage orchestration (config-gated on `triage_after_mins`).
    // An entry is consumed when its triage fires or when the session stops needing attention;
    // the notify latch above prevents re-adds until the agent does real work again.
    let mut pending_triage: HashMap<(LaneId, SessKey), Instant> = HashMap::new();

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
            if kind == NotifKind::NeedsYou && cfg.triage_after_mins.is_some() {
                pending_triage.insert((key.0, key.1.clone()), Instant::now());
            }
            fires.push((key, kind));
        }
        // Needs-you triage: after `triage_after_mins` with the agent still stuck and still no
        // UI attached (no TUI heartbeat, no live connections), fire one bounded triage
        // orchestration for the lane. The entry is consumed either way.
        if let Some(after_mins) = cfg.triage_after_mins {
            let ui_attached = tui_active || !ctx.sessions.lock().await.is_empty();
            let mut due: Vec<(LaneId, SessKey)> = Vec::new();
            pending_triage.retain(|(lane_id, sess), fired| {
                if !now.contains_key(&(*lane_id, sess.clone())) {
                    return false; // agent moved on; triage moot
                }
                if crate::standing::triage_due(fired.elapsed(), after_mins, ui_attached) {
                    due.push((*lane_id, sess.clone()));
                    return false;
                }
                true
            });
            for (lane_id, _sess) in due {
                let repo = lanes
                    .iter()
                    .find(|l| l.id == lane_id)
                    .map(|l| l.repo.name.clone())
                    .unwrap_or_default();
                let prompt = format!(
                    "Triage lane {lane_id} (repo {repo}): an agent there has needed attention \
                     for over {after_mins} minutes with nobody watching. Use read_agent to see \
                     its state, classify the situation, and recommend exactly ONE next action \
                     for the human. Do not approve, merge, or delete anything. End with a 2-3 \
                     sentence briefing."
                );
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    tracing::info!(lane = lane_id, "needs-you triage firing");
                    crate::standing::run_standing(
                        &ctx,
                        "triage_run",
                        &format!("triage-{lane_id}"),
                        &prompt,
                        5,
                        json!({ "lane_id": lane_id, "repo": repo }),
                        Some(lane_id),
                    )
                    .await;
                });
            }
        } else {
            pending_triage.clear();
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
            // Legacy approval-policy auto-approve: a routine Bash permission matching a
            // confirmed per-repo rule is answered by the daemon itself and the alert is
            // suppressed — the acceptance is precisely "the fourth cargo test never reaches
            // your phone". Routed through `inject::verified_send` (T11) so it shares the one
            // audited, re-verified send path with supervision; supervised lanes opt out here
            // entirely, since their own loop already carries this same learned rule.
            if kind == NotifKind::NeedsYou
                && legacy_rule_auto_approve(&ctx, lane_id, lane, sess).await
            {
                continue;
            }
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
            // Every subscribed local or remote client receives the feed event. APNs remains gated
            // behind the remote bridge because it carries remote credentials and device state.
            ctx.broadcast("event.notification", payload.clone());
            if cfg.remote.enabled {
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

            // Local desktop popup, fired by the daemon only when no local UI is already covering
            // it (the TUI is parked in an attach, or nothing is open), so we never double-notify
            // with the TUI's own. `notify_desktop_fallback` turns it off entirely: on macOS this
            // goes out via `osascript`, which delivers as Script Editor and wears its icon.
            if repomon_core::notify::daemon_popup_allowed(tui_active, cfg.notify_desktop_fallback) {
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

/// Legacy learned-rule auto-approve (pre-dates supervision): a routine Bash permission dialog
/// whose `(repo, command_pattern)` has a confirmed `ApprovalRule` is answered by the daemon
/// itself — the acceptance is precisely "the fourth cargo test never reaches your phone". The
/// hardcoded always-escalate sniffer wins over any learned rule. Returns `true` when the alert
/// should be suppressed (this block fully handled the dialog); `false` to fall through to
/// normal notification handling — including when the send was skipped or failed, so the human
/// still gets notified.
///
/// A lane under active supervision opts out here entirely: the supervision loop owns answering
/// there and carries this same learned rule via its own `extra_allow` input
/// (`supervision::handle_session`), so this legacy path must never race it.
async fn legacy_rule_auto_approve(
    ctx: &Ctx,
    lane_id: LaneId,
    lane: &Lane,
    sess: Option<&AgentSession>,
) -> bool {
    if crate::supervision::supervised(ctx, lane_id).await.is_some() {
        return false;
    }

    use repomon_core::agent::approval;
    let auto = match sess {
        Some(s) => match (s.pending_dialog.as_ref(), s.tmux_window.clone()) {
            (Some(dialog), Some(window)) => approval::dialog_command(dialog)
                .map(|cmd| (approval::command_pattern(&cmd), cmd, window, dialog.clone())),
            _ => None,
        },
        None => None,
    };
    let Some((pattern, cmd, window, dialog)) = auto else {
        return false;
    };
    let allowed = !approval::is_always_escalate(&cmd)
        && ctx
            .store
            .has_approval_rule(lane.repo.name.clone(), pattern.clone())
            .await
            .unwrap_or(false);
    if !allowed {
        return false;
    }

    tracing::info!(
        lane = lane_id,
        pattern = %pattern,
        "auto-approving allowlisted permission"
    );

    let seed = AuditSeed {
        lane_id,
        window: window.clone(),
        session_id: sess.and_then(|s| s.session_id.clone()),
        agent_kind: sess.map(|s| s.agent.as_str().to_string()),
        trigger: "legacy_rule".to_string(),
        dialog_class: Some(DialogClass::CommandExec),
        repo_scoped: None,
        decision: "approve".to_string(),
        policy_source: Some(PolicySource::ApprovalRule),
        reason: Some(format!("learned rule matched pattern '{pattern}'")),
        subject: Some(cmd.clone()),
        pane_excerpt: None,
    };
    let outcome = inject::verified_send(
        ctx,
        Expectation::DialogSummary(dialog.summary()),
        Payload::Keys(vec!["Enter".into()]),
        seed,
    )
    .await;

    match outcome {
        SendOutcome::Sent { .. } => {
            let _ = ctx
                .store
                .append_journal(repomon_core::model::JournalEntry {
                    id: 0,
                    at: chrono::Utc::now(),
                    session: format!("auto-approve-{lane_id}"),
                    action: "auto_approve".into(),
                    lane_id: Some(lane_id),
                    repo: Some(lane.repo.name.clone()),
                    params: Some(json!({ "pattern": pattern, "command": cmd }).to_string()),
                    outcome: "ok".into(),
                    detail: None,
                })
                .await;
            true
        }
        SendOutcome::Skipped { .. } | SendOutcome::Failed { .. } => false,
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
        let tmux = ctx.backend.clone();
        let pane = tokio::task::spawn_blocking(move || {
            tmux.capture_named(ORCHESTRATOR_WINDOW, CaptureOpts::last(ORCH_CAPTURE_LINES))
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

    if edge_to_attention && cfg.notify_enabled && cfg.notify_needs_you {
        let due = popup_fired
            .map(|t| t.elapsed() >= ORCH_POPUP_DEBOUNCE)
            .unwrap_or(true);
        if due {
            *popup_fired = Some(Instant::now());
            // Phone loop: repomind's own escalations reach remote clients like a managed
            // agent's would — event.notification for the in-app feed plus APNs — regardless of
            // whether a TUI is open locally (mirrors the lane path's remote gating).
            if cfg.remote.enabled {
                let (title, body, payload) =
                    orchestrator_attention_payload(word, headline.as_deref());
                ctx.broadcast("event.notification", payload.clone());
                push::send_all(ctx, &title, &body, push::CATEGORY_ALERT, &payload).await;
            }
            // Local desktop popup only when the TUI isn't already covering it, and only if the
            // user left the daemon's own popup switched on.
            if repomon_core::notify::daemon_popup_allowed(tui_active, cfg.notify_desktop_fallback) {
                repomon_core::notify::send_native(
                    "repomind needs you",
                    headline.as_deref().unwrap_or(""),
                    cfg.notify_sound,
                    cfg.notify_click_focus,
                );
            }
        }
    }
}

/// Build the remote notification for a repomind needs-you edge: title, push body, and the
/// event payload. Pure so the shape is testable; `attention` is the derived word
/// (permission / decision / end_of_turn) and `headline` the dialog summary or last message.
fn orchestrator_attention_payload(
    attention: &str,
    headline: Option<&str>,
) -> (String, String, serde_json::Value) {
    let title = "repomind needs you".to_string();
    let body = match headline {
        Some(h) if !h.trim().is_empty() => h.trim().to_string(),
        _ => format!("repomind is waiting on you ({attention})"),
    };
    let payload = json!({
        // One live orchestrator pane exists, so attention-kind granularity is enough for
        // client-side dedup (a re-fire only happens after the attention word changed).
        "id": format!("orchestrator:{attention}"),
        "kind": "orchestrator_needs_you",
        "attention": attention,
        "title": title,
        "body": body,
    });
    (title, body, payload)
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
mod orch_payload_tests {
    use super::*;

    #[test]
    fn orchestrator_payload_carries_attention_and_headline() {
        let (title, body, payload) =
            orchestrator_attention_payload("decision", Some("Which auth method should we use?"));
        assert_eq!(title, "repomind needs you");
        assert!(body.contains("auth method"));
        assert_eq!(payload["kind"], serde_json::json!("orchestrator_needs_you"));
        assert_eq!(payload["attention"], serde_json::json!("decision"));
        assert!(payload["id"].as_str().unwrap().contains("orchestrator"));
    }

    #[test]
    fn orchestrator_payload_survives_a_missing_headline() {
        let (_, body, payload) = orchestrator_attention_payload("end_of_turn", None);
        assert!(
            !body.is_empty(),
            "an empty push body reads as a bug on the phone"
        );
        assert_eq!(payload["attention"], serde_json::json!("end_of_turn"));
    }
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

#[cfg(test)]
mod legacy_auto_approve_tests {
    use super::*;
    use repomon_core::agent::backend::{
        AttachCommand, ByteStream, OwnerState, ScrollEvent, SessionBackend, SpawnSpec,
        WindowActivity,
    };
    use repomon_core::agent::prompt::detect_dialog;
    use repomon_core::agent::supervision::{PolicyAction, SupervisionOverrides};
    use repomon_core::model::{AgentKind, Repo, Worktree, WorktreeState};
    use repomon_core::{Config, Store};
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;

    struct ScriptedBackend {
        captures: StdMutex<Vec<String>>,
        sent_keys: StdMutex<Vec<(String, String)>>,
        capture_calls: std::sync::atomic::AtomicU32,
    }

    impl ScriptedBackend {
        fn new(captures: Vec<String>) -> Self {
            Self {
                captures: StdMutex::new(captures),
                sent_keys: StdMutex::new(Vec::new()),
                capture_calls: std::sync::atomic::AtomicU32::new(0),
            }
        }
    }

    impl SessionBackend for ScriptedBackend {
        fn available(&self) -> bool {
            true
        }
        fn label(&self) -> String {
            "scripted".to_string()
        }
        fn session_exists(&self) -> bool {
            true
        }
        fn claim_or_verify_owner(&self, _me: &str) -> OwnerState {
            OwnerState::Owned
        }
        fn list_windows(&self) -> repomon_core::Result<Vec<String>> {
            Ok(vec![])
        }
        fn list_windows_with_activity(&self) -> repomon_core::Result<Vec<WindowActivity>> {
            Ok(vec![])
        }
        fn spawn(&self, _lane: LaneId, _spec: &SpawnSpec) -> repomon_core::Result<String> {
            Ok("target".into())
        }
        fn spawn_named(&self, _window: &str, _spec: &SpawnSpec) -> repomon_core::Result<String> {
            Ok("target".into())
        }
        fn open_named(
            &self,
            _window: &str,
            _cwd: &std::path::Path,
        ) -> repomon_core::Result<String> {
            Ok("target".into())
        }
        fn capture_named(&self, _window: &str, _opts: CaptureOpts) -> repomon_core::Result<String> {
            self.capture_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut c = self.captures.lock().unwrap();
            if c.is_empty() {
                Ok(String::new())
            } else {
                Ok(c.remove(0))
            }
        }
        fn cursor_named(&self, _window: &str) -> Option<repomon_core::agent::Cursor> {
            None
        }
        fn size_named(&self, _window: &str) -> Option<(u16, u16)> {
            Some((80, 24))
        }
        fn resize_named(&self, _window: &str, _cols: u16, _rows: u16) -> repomon_core::Result<()> {
            Ok(())
        }
        fn follow_client_named(&self, _window: &str) -> repomon_core::Result<()> {
            Ok(())
        }
        fn alternate_on_named(&self, _window: &str) -> bool {
            false
        }
        fn scroll_wheel_named(
            &self,
            _window: &str,
            _event: ScrollEvent,
        ) -> repomon_core::Result<()> {
            Ok(())
        }
        fn send_literal_named(&self, _window: &str, _text: &str) -> repomon_core::Result<()> {
            Ok(())
        }
        fn send_text_named(&self, _window: &str, _text: &str) -> repomon_core::Result<()> {
            Ok(())
        }
        fn send_key_named(&self, window: &str, key: &str) -> repomon_core::Result<()> {
            self.sent_keys
                .lock()
                .unwrap()
                .push((window.to_string(), key.to_string()));
            Ok(())
        }
        fn kill_named(&self, _window: &str) -> repomon_core::Result<()> {
            Ok(())
        }
        fn configure(&self) {}
        fn target_named(&self, window: &str) -> String {
            window.to_string()
        }
        fn exact_target_named(&self, window: &str) -> String {
            window.to_string()
        }
        fn attach_command(&self, target: &str) -> AttachCommand {
            AttachCommand {
                program: "tmux".into(),
                args: vec!["attach".into(), "-t".into(), target.into()],
            }
        }
        fn open_byte_stream(&self, _window: &str) -> repomon_core::Result<ByteStream> {
            let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
            Ok(ByteStream { rx })
        }
        fn close_byte_stream(&self, _window: &str) -> repomon_core::Result<()> {
            Ok(())
        }
    }

    fn make_ctx(backend: Arc<dyn SessionBackend>) -> Arc<Ctx> {
        let store = Store::open_in_memory().unwrap();
        Ctx::new_with_backend(
            store,
            Config::default(),
            None,
            PathBuf::from("/tmp/config.toml"),
            PathBuf::from("/tmp/repo-notes"),
            backend,
        )
    }

    const DIALOG_A: &str = "● Running cargo install…\n\
        ╭──────────────────────────────────────────────╮\n\
        │ Bash command                                 │\n\
        │                                              │\n\
        │   cargo install --path crates/repomon-tui    │\n\
        │   Install the repomon TUI                    │\n\
        │                                              │\n\
        │ Do you want to proceed?                      │\n\
        │ ❯ 1. Yes                                     │\n\
        │   2. Yes, and don't ask again for cargo      │\n\
        │   3. No, and tell Claude what to do          │\n\
        ╰──────────────────────────────────────────────╯";

    const IDLE_PANE: &str = "azaleas@macbook repomon % cargo check\n    \
        Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.04s\n\
        azaleas@macbook repomon % ";

    fn sample_lane(session: AgentSession) -> Lane {
        let head = "0000000000000000000000000000000000000000".parse().unwrap();
        Lane {
            id: 1,
            repo: Repo {
                id: 1,
                path: PathBuf::from("/code/alpha"),
                name: "test-repo".into(),
                added_at: Utc::now(),
                worktree_root_template: None,
                hidden: false,
            },
            worktree: Worktree {
                id: 1,
                repo_id: 1,
                path: PathBuf::from("/code/alpha"),
                branch: Some("main".into()),
                head,
                is_main: true,
                name: "main".into(),
            },
            state: WorktreeState {
                worktree_id: 1,
                head,
                branch: Some("feat/x".into()),
                upstream: None,
                ahead: 0,
                behind: 0,
                dirty: Default::default(),
                last_commit_at: None,
                locked: false,
                prunable: false,
                last_change_at: None,
            },
            agent_sessions: vec![session],
            last_activity_at: Utc::now(),
            pinned: false,
        }
    }

    fn sample_session(dialog: repomon_core::agent::prompt::PendingDialog) -> AgentSession {
        AgentSession {
            id: 1,
            agent: AgentKind::ClaudeCode,
            repo_id: 1,
            worktree_id: Some(1),
            started_at: Utc::now(),
            last_activity_at: Utc::now(),
            ended_at: None,
            manifest_path: PathBuf::from("/repo/.manifest"),
            tool_call_count: 5,
            title: Some("test session".into()),
            status: AgentStatus::Waiting,
            external: false,
            session_id: Some("sess-legacy".into()),
            resume_at: None,
            inferred: false,
            tmux_window: Some("win-lane-1".into()),
            last_message: None,
            pending_prompt: Some(dialog.summary()),
            pending_dialog: Some(dialog),
            stale: false,
            stalled_since: None,
            subagent_running: None,
            ended_turn: false,
            gate: None,
            config_dir: None,
            custom_label: None,
            generated_label: None,
        }
    }

    #[tokio::test]
    async fn legacy_rule_still_approves_when_unsupervised() {
        let backend = Arc::new(ScriptedBackend::new(vec![DIALOG_A.to_string()]));
        let ctx = make_ctx(backend.clone());

        ctx.store
            .add_approval_rule("test-repo".into(), "cargo install".into())
            .await
            .unwrap();

        let dialog = detect_dialog(DIALOG_A).expect("dialog detected");
        let session = sample_session(dialog);
        let lane = sample_lane(session.clone());

        let suppressed = legacy_rule_auto_approve(&ctx, lane.id, &lane, Some(&session)).await;
        assert!(
            suppressed,
            "unsupervised matching rule must suppress the alert"
        );

        let sent = backend.sent_keys.lock().unwrap().clone();
        assert_eq!(sent, vec![("win-lane-1".to_string(), "Enter".to_string())]);

        let journal = ctx.store.recent_journal(10).await.unwrap();
        assert_eq!(journal.len(), 1);
        assert_eq!(journal[0].action, "auto_approve");
        assert_eq!(journal[0].lane_id, Some(1));

        let log = ctx.store.supervision_log(None, 10, None).await.unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].outcome, "sent");
        assert_eq!(log[0].trigger, "legacy_rule");
    }

    #[tokio::test]
    async fn legacy_rule_skips_when_dialog_changed() {
        // The dialog is gone by send time (idle pane instead): verified_send must skip rather
        // than type a stray Enter, and the alert must NOT be suppressed — the one documented
        // deviation from the old raw-send behavior.
        let backend = Arc::new(ScriptedBackend::new(vec![IDLE_PANE.to_string()]));
        let ctx = make_ctx(backend.clone());

        ctx.store
            .add_approval_rule("test-repo".into(), "cargo install".into())
            .await
            .unwrap();

        let dialog = detect_dialog(DIALOG_A).expect("dialog detected");
        let session = sample_session(dialog);
        let lane = sample_lane(session.clone());

        let suppressed = legacy_rule_auto_approve(&ctx, lane.id, &lane, Some(&session)).await;
        assert!(!suppressed, "a vanished dialog must not suppress the alert");

        assert!(backend.sent_keys.lock().unwrap().is_empty());

        let journal = ctx.store.recent_journal(10).await.unwrap();
        assert!(journal.is_empty(), "no auto_approve journal row on a skip");

        let log = ctx.store.supervision_log(None, 10, None).await.unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].outcome, "skipped");
    }

    #[tokio::test]
    async fn legacy_block_inert_for_supervised_lane() {
        let backend = Arc::new(ScriptedBackend::new(vec![DIALOG_A.to_string()]));
        let ctx = make_ctx(backend.clone());
        ctx.config.write().await.supervision.enabled = true;

        ctx.store
            .add_approval_rule("test-repo".into(), "cargo install".into())
            .await
            .unwrap();

        let p = SupervisionOverrides {
            lane_id: 1,
            enabled: true,
            classes: [(DialogClass::CommandExec, PolicyAction::AutoApprove)]
                .into_iter()
                .collect(),
            mail_mode: None,
            nudge_text: None,
            stall_mins: None,
            nudge_retries: None,
            expect_work: true,
            updated_at: Utc::now(),
        };
        ctx.store.set_lane_policy(p).await.unwrap();
        crate::supervision::refresh(&ctx).await;
        assert!(crate::supervision::supervised(&ctx, 1).await.is_some());

        let dialog = detect_dialog(DIALOG_A).expect("dialog detected");
        let session = sample_session(dialog);
        let lane = sample_lane(session.clone());

        let suppressed = legacy_rule_auto_approve(&ctx, lane.id, &lane, Some(&session)).await;
        assert!(
            !suppressed,
            "a supervised lane's legacy block must be a no-op"
        );

        assert_eq!(
            backend
                .capture_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            0,
            "supervised opt-out must happen before any pane capture"
        );
        assert!(backend.sent_keys.lock().unwrap().is_empty());

        let journal = ctx.store.recent_journal(10).await.unwrap();
        assert!(journal.is_empty());

        let log = ctx.store.supervision_log(None, 10, None).await.unwrap();
        assert!(
            log.is_empty(),
            "zero backend sends attributable to the legacy block"
        );
    }
}
