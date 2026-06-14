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

use repomon_core::model::{AgentStatus, LaneId};
use repomon_core::notify::{
    compose, diff_session_transitions, session_by_key, session_statuses, slot_by_key, NotifKind,
    SessKey,
};
use repomon_core::Config;
use serde_json::json;

use crate::{push, rpc, Ctx};

/// How often the watcher re-reads the fleet for remote/push notifications. Each tick recomputes
/// the overlay (transcript parses, pane sniffs, process probes), so this trades notification
/// latency for idle CPU — a phone alert a few seconds later is fine, a daemon pegging a core isn't.
const TICK: Duration = Duration::from_secs(8);
/// Don't re-fire the same session's notification within this window (status flapping).
const DEBOUNCE: Duration = Duration::from_secs(30);

pub async fn notify_watch(ctx: Arc<Ctx>) {
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut prev: HashMap<(LaneId, SessKey), AgentStatus> = HashMap::new();
    let mut seeded = false;
    let mut debounce: HashMap<(LaneId, SessKey, NotifKind), Instant> = HashMap::new();

    loop {
        tick.tick().await;
        let cfg = ctx.config.read().await.clone();
        if !cfg.remote.enabled || !cfg.notify_enabled {
            // Drop state while disabled so re-enabling re-seeds instead of firing a backlog.
            prev.clear();
            seeded = false;
            debounce.clear();
            continue;
        }

        // Always recompute (bypass the lane.list cache): edge detection must never reuse a stale
        // snapshot, and in a headless setup nothing else populates the cache.
        let Ok(lanes) = rpc::lanes_with_agents_fresh(&ctx).await else {
            continue;
        };
        let now: HashMap<(LaneId, SessKey), AgentStatus> = lanes
            .iter()
            .flat_map(|l| session_statuses(l.id, &l.agent_sessions))
            .collect();
        if !seeded {
            prev = now;
            seeded = true;
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
            debounce.insert(dkey, Instant::now());
            fires.push((key, kind));
        }
        prev = now;
        let snapshot = &prev;
        debounce.retain(|(lane, sess, _), t| {
            snapshot.contains_key(&(*lane, sess.clone())) || t.elapsed() < DEBOUNCE
        });

        for ((lane_id, key), kind) in fires {
            let Some(lane) = lanes.iter().find(|l| l.id == lane_id) else {
                continue;
            };
            let sess = session_by_key(lane, &key);
            let (title, body) = compose(
                kind,
                lane,
                sess,
                slot_by_key(lane, &key),
                cfg.notify_show_why,
            );
            // The actual on-screen dialog, when there is one — what a push's Approve acts on.
            let dialog = sess.and_then(|s| s.pending_prompt.clone());
            // The payload's "prompt" falls back to the agent's last message for context.
            let prompt = dialog
                .clone()
                .or_else(|| sess.and_then(|s| s.last_message.clone()));
            let payload = json!({
                "lane_id": lane_id,
                "session_id": sess.and_then(|s| s.session_id.clone()),
                "kind": kind,
                "title": title,
                "body": body,
                "prompt": prompt,
            });
            ctx.broadcast("event.notification", payload.clone());

            // Lock-screen push: a NeedsYou with a pending question gets the actionable
            // category (Approve / Open); everything else is a plain alert.
            // Approve-from-lock-screen only when an actual dialog is up — a plain "finished
            // its turn" Enter would be a no-op (or worse, submit an empty reply).
            let category = if kind == NotifKind::NeedsYou && dialog.is_some() {
                push::CATEGORY_PROMPT
            } else {
                push::CATEGORY_ALERT
            };
            push::send_all(&ctx, &title, &body, category, &payload).await;
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
    }
}
