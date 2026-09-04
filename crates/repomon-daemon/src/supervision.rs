//! Supervision policy snapshot, watcher loop, and policy-driven dialog answering.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use repomon_core::agent::approval;
use repomon_core::agent::prompt::{self, PendingDialog};
use repomon_core::agent::supervision::{
    Decision, DialogClass, DialogScope, PolicyAction, SupervisionPolicy, classify_dialog, evaluate,
    resolve,
};
use repomon_core::model::{AgentSession, AgentStatus, Lane, LaneId};
use serde_json::json;

use crate::Ctx;
use crate::inject::{self, AuditSeed, Expectation, Payload};

const TICK: Duration = Duration::from_secs(2);

/// Action decided for an agent session's dialog by the supervision loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopAction {
    Answer { choice: usize },
    Deny { choice: usize },
    Hold,
    Nothing,
}

/// Pure decision router for supervision of a single dialog.
pub fn supervise_dialog(
    master: bool,
    lane_enabled: bool,
    external: bool,
    has_window: bool,
    decision: &Decision,
) -> LoopAction {
    if !master || !lane_enabled || external || !has_window {
        return LoopAction::Nothing;
    }
    match decision.action {
        PolicyAction::AutoApprove => match decision.choice {
            Some(choice) => LoopAction::Answer { choice },
            None => LoopAction::Hold,
        },
        PolicyAction::AutoDeny => match decision.choice {
            Some(choice) => LoopAction::Deny { choice },
            None => LoopAction::Hold,
        },
        PolicyAction::Hold => LoopAction::Hold,
    }
}

/// Render a dialog as compact text for audit logging (title + context + question + options, <= 800 chars).
pub fn format_dialog_excerpt(dialog: &PendingDialog) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(title) = &dialog.title {
        parts.push(title.clone());
    }
    for ctx_line in &dialog.context {
        parts.push(ctx_line.clone());
    }
    parts.push(dialog.question.clone());
    for opt in &dialog.options {
        if let Some(num) = opt.number {
            parts.push(format!("{num}. {}", opt.text));
        } else {
            parts.push(opt.text.clone());
        }
    }
    let full = parts.join("\n");
    if full.len() > 800 {
        full.chars().take(800).collect()
    } else {
        full
    }
}

/// Cached in-memory snapshot of effective supervision policies for all enabled lanes.
#[derive(Debug, Clone, Default)]
pub struct PolicySnapshot {
    /// Global supervision master switch (`config.supervision.enabled`).
    pub master: bool,
    /// Effective policies for lanes where supervision is active (both master and lane enabled).
    pub lanes: HashMap<LaneId, SupervisionPolicy>,
}

impl PolicySnapshot {
    /// Return the effective policy for `id` if supervision is active on that lane.
    pub fn lane(&self, id: LaneId) -> Option<&SupervisionPolicy> {
        self.lanes.get(&id)
    }

    /// True if the master switch is on and at least one lane is actively supervised.
    pub fn any_enabled(&self) -> bool {
        self.master && !self.lanes.is_empty()
    }
}

/// Rebuild a fresh [`PolicySnapshot`] by reading user config and DB overrides.
pub async fn rebuild_snapshot(ctx: &Ctx) -> PolicySnapshot {
    let defaults = ctx.config.read().await.supervision.clone();
    let master = defaults.enabled;
    let mut lanes = HashMap::new();
    if master {
        if let Ok(policies) = ctx.store.lane_policies().await {
            for p in policies {
                let effective = resolve(&defaults, Some(&p));
                if effective.enabled {
                    lanes.insert(p.lane_id, effective);
                }
            }
        }
    }
    PolicySnapshot { master, lanes }
}

/// Rebuild and update the cached [`PolicySnapshot`] on `ctx.supervision`.
pub async fn refresh(ctx: &Ctx) {
    let snapshot = rebuild_snapshot(ctx).await;
    *ctx.supervision.write().await = snapshot;
}

/// Get the current effective [`SupervisionPolicy`] for a lane, if actively supervised.
pub async fn supervised(ctx: &Ctx, lane: LaneId) -> Option<SupervisionPolicy> {
    ctx.supervision.read().await.lane(lane).cloned()
}

/// Handle supervision for a single agent session in a supervised lane.
#[allow(clippy::too_many_arguments)]
pub async fn handle_session(
    ctx: &Ctx,
    lane_id: LaneId,
    repo_name: &str,
    repo_root: &Path,
    worktree: &Path,
    policy: &SupervisionPolicy,
    session: &AgentSession,
    held_cache: &mut HashMap<String, String>,
) {
    if session.external {
        return;
    }
    let Some(window) = &session.tmux_window else {
        return;
    };
    let Some(dialog) = &session.pending_dialog else {
        held_cache.remove(window);
        return;
    };

    let scope = DialogScope {
        worktree: worktree.to_path_buf(),
        repo_root: repo_root.to_path_buf(),
    };
    let classification = classify_dialog(dialog, session.agent.clone(), &scope);

    let extra_allow = if classification.class == DialogClass::CommandExec {
        if let Some(cmd) = &classification.subject {
            !approval::is_always_escalate(cmd)
                && ctx
                    .store
                    .has_approval_rule(repo_name.to_string(), approval::command_pattern(cmd))
                    .await
                    .unwrap_or(false)
        } else {
            false
        }
    } else {
        false
    };

    let decision = evaluate(
        policy,
        &classification,
        dialog,
        session.agent.clone(),
        extra_allow,
    );
    let master = ctx.config.read().await.supervision.enabled;
    let action = supervise_dialog(
        master,
        policy.enabled,
        session.external,
        session.tmux_window.is_some(),
        &decision,
    );

    match action {
        LoopAction::Answer { choice } => {
            held_cache.remove(window);
            let seed = AuditSeed {
                lane_id,
                window: window.clone(),
                session_id: session.session_id.clone(),
                agent_kind: Some(session.agent.as_str().to_string()),
                trigger: "dialog".to_string(),
                dialog_class: Some(classification.class),
                repo_scoped: Some(classification.repo_scoped),
                decision: "approve".to_string(),
                policy_source: Some(decision.source),
                reason: Some(decision.reason),
                subject: classification.subject,
                pane_excerpt: None,
            };
            let expect = Expectation::DialogSummary(dialog.summary());
            let payload = Payload::Keys(prompt::dialog_select_keys(dialog, choice));
            inject::verified_send(ctx, expect, payload, seed).await;
        }
        LoopAction::Deny { choice } => {
            held_cache.remove(window);
            let seed = AuditSeed {
                lane_id,
                window: window.clone(),
                session_id: session.session_id.clone(),
                agent_kind: Some(session.agent.as_str().to_string()),
                trigger: "dialog".to_string(),
                dialog_class: Some(classification.class),
                repo_scoped: Some(classification.repo_scoped),
                decision: "deny".to_string(),
                policy_source: Some(decision.source),
                reason: Some(decision.reason),
                subject: classification.subject,
                pane_excerpt: None,
            };
            let expect = Expectation::DialogSummary(dialog.summary());
            let payload = Payload::Keys(prompt::dialog_select_keys(dialog, choice));
            inject::verified_send(ctx, expect, payload, seed).await;
        }
        LoopAction::Hold => {
            let summary = dialog.summary();
            let already_held = held_cache.get(window).is_some_and(|last| last == &summary);
            if !already_held {
                held_cache.insert(window.clone(), summary);
                let excerpt = format_dialog_excerpt(dialog);
                let seed = AuditSeed {
                    lane_id,
                    window: window.clone(),
                    session_id: session.session_id.clone(),
                    agent_kind: Some(session.agent.as_str().to_string()),
                    trigger: "dialog".to_string(),
                    dialog_class: Some(classification.class),
                    repo_scoped: Some(classification.repo_scoped),
                    decision: "hold".to_string(),
                    policy_source: Some(decision.source),
                    reason: Some(decision.reason),
                    subject: classification.subject,
                    pane_excerpt: Some(excerpt),
                };
                inject::record_hold(ctx, seed).await;
            }
        }
        LoopAction::Nothing => {}
    }
}

/// Background supervision loop that answers pending dialogs per policy.
pub async fn supervision_watch(ctx: Arc<Ctx>) {
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut held_cache: HashMap<String, String> = HashMap::new();
    let mut stall_scheds: HashMap<String, StallSched> = HashMap::new();

    loop {
        tokio::select! {
            _ = ctx.shutdown.notified() => break,
            _ = tick.tick() => {
                supervision_step(&ctx, &mut held_cache, &mut stall_scheds).await;
            }
        }
    }
}

async fn supervision_step(
    ctx: &Ctx,
    held_cache: &mut HashMap<String, String>,
    stall_scheds: &mut HashMap<String, StallSched>,
) {
    refresh(ctx).await;
    let snapshot = ctx.supervision.read().await.clone();
    if !snapshot.master || snapshot.lanes.is_empty() {
        return;
    }

    let lanes = match crate::rpc::lanes_with_agents(ctx).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("supervision failed to get lanes: {e:?}");
            return;
        }
    };

    for lane in &lanes {
        if let Some(policy) = snapshot.lane(lane.id) {
            for session in &lane.agent_sessions {
                handle_session(
                    ctx,
                    lane.id,
                    &lane.repo.name,
                    &lane.repo.path,
                    &lane.worktree.path,
                    policy,
                    session,
                    held_cache,
                )
                .await;
            }
        }
    }

    // Durable mail has its own push-based worker. The stall phase only handles explicit
    // `expect_work`; queued mail is delivered to its exact recipient rather than converted into
    // a lane-wide "check your inbox" nudge.
    stall_phase(ctx, &lanes, &snapshot, stall_scheds, Utc::now()).await;
}

// ---- supervised stall nudge & escalation (T10) ----------------------------------------------

/// How long a supervised agent's own pane must sit unchanged (evidence-of-freeze) before a
/// stalled session's idle time is trusted — mirrors how `rpc.rs::stall_since` trusts
/// `ctx.pane_seen` for the unsupervised stall watchdog.
const NUDGE_SPACING: chrono::Duration = chrono::Duration::minutes(5);

/// Loop-local retry state for one supervised window's stall episode.
#[derive(Debug, Clone, Copy)]
pub struct StallSched {
    nudges_sent: u32,
    last_nudge_at: Option<DateTime<Utc>>,
    escalated: bool,
}

/// Action decided for one supervised window by [`decide_stall`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallAction {
    Nudge,
    Escalate,
    Nothing,
}

/// Pure decision router for the supervised stall nudge/escalate state machine: no outstanding
/// work, or the agent hasn't been idle long enough, is always Nothing; an `escalated` sched
/// latches Nothing forever; a fresh episode (no sched, or a sched whose first nudge hasn't
/// happened yet) nudges immediately; after the first nudge, [`NUDGE_SPACING`] must elapse before
/// the next nudge or the escalation decision, which fires once `nudge_retries` nudges have gone
/// out.
pub fn decide_stall(
    sched: Option<&StallSched>,
    idle_mins: i64,
    outstanding: bool,
    policy_stall_mins: u32,
    policy_retries: u32,
    now: DateTime<Utc>,
) -> StallAction {
    if !outstanding || idle_mins < policy_stall_mins as i64 {
        return StallAction::Nothing;
    }
    let Some(sched) = sched else {
        return StallAction::Nudge;
    };
    if sched.escalated {
        return StallAction::Nothing;
    }
    if sched.nudges_sent == 0 {
        return StallAction::Nudge;
    }
    let spacing_elapsed = sched
        .last_nudge_at
        .is_none_or(|last| now - last >= NUDGE_SPACING);
    if !spacing_elapsed {
        return StallAction::Nothing;
    }
    if sched.nudges_sent < policy_retries {
        StallAction::Nudge
    } else {
        StallAction::Escalate
    }
}

/// Record that a stall nudge attempt for `window` was made (sent, skipped, or failed — any
/// `verified_send` outcome consumes the attempt, same as the mail phase's `bump_sched`).
fn bump_stall_sched(scheds: &mut HashMap<String, StallSched>, window: &str, now: DateTime<Utc>) {
    let entry = scheds.entry(window.to_string()).or_insert(StallSched {
        nudges_sent: 0,
        last_nudge_at: None,
        escalated: false,
    });
    entry.nudges_sent += 1;
    entry.last_nudge_at = Some(now);
}

/// Only a non-external, windowed session with no dialog on screen, that is not mid-generation,
/// can be considered stalled here. `AgentStatus::Running` counts only once its turn has ended
/// (mirrors `mail.rs::injection_eligible`) — an agent still generating is the existing
/// `stall_since` watchdog's job, not this feature's.
fn stall_eligible(session: &AgentSession) -> bool {
    if session.external || session.tmux_window.is_none() || session.pending_dialog.is_some() {
        return false;
    }
    matches!(session.status, AgentStatus::Waiting | AgentStatus::Idle)
        || (session.status == AgentStatus::Running && session.ended_turn)
}

/// Whether `window`'s pane has sat unchanged for at least `stall_mins` — no recorded change at
/// all means no evidence of a freeze, so (mirroring `rpc.rs::stall_since`'s `None` case) it does
/// NOT count as quiet.
async fn pane_quiet_for(ctx: &Ctx, window: &str, stall_mins: u32, now: DateTime<Utc>) -> bool {
    let seen = ctx.pane_seen.lock().await;
    match seen.get(window) {
        Some(&(_, changed_at)) => now - changed_at >= chrono::Duration::minutes(stall_mins as i64),
        None => false,
    }
}

/// The `event.notification` payload for a stall escalation, mirroring the field set
/// `mail_give_up_payload` broadcasts for the analogous mail give-up.
fn stall_escalate_payload(lane: &Lane, idle_mins: i64) -> serde_json::Value {
    json!({
        "kind": "needs_you",
        "title": format!("{} needs you", lane.repo.name),
        "body": format!("agent has been idle {idle_mins}m with outstanding work despite nudges"),
        "lane_id": lane.id,
    })
}

/// Supervised stall handling: per eligible session in a supervised lane, when it has been idle
/// past the policy's `stall_mins` with explicit outstanding assigned work (`policy.expect_work`)
/// AND its pane has independently sat quiet that long, send one nudge; if nudges keep failing to
/// unstick it, raise attention once and hold until the agent shows activity again. Durable mail is
/// intentionally excluded: its push worker targets the exact recipient with the actual body.
async fn stall_phase(
    ctx: &Ctx,
    lanes: &[Lane],
    snapshot: &PolicySnapshot,
    scheds: &mut HashMap<String, StallSched>,
    now: DateTime<Utc>,
) {
    for lane in lanes {
        let Some(policy) = snapshot.lane(lane.id) else {
            continue;
        };
        for session in &lane.agent_sessions {
            if !stall_eligible(session) {
                continue;
            }
            let Some(window) = session.tmux_window.clone() else {
                continue;
            };
            let outstanding = policy.expect_work;

            // Activity reset: drop a stale episode's bookkeeping once the agent has moved
            // (activity past the last nudge) or there's no longer any outstanding work, so a
            // future stall episode for this window starts fresh.
            if let Some(sched) = scheds.get(&window) {
                let activity_moved = sched
                    .last_nudge_at
                    .is_some_and(|last| session.last_activity_at > last);
                if activity_moved || !outstanding {
                    scheds.remove(&window);
                }
            }

            if !outstanding {
                continue;
            }
            if !pane_quiet_for(ctx, &window, policy.stall_mins, now).await {
                continue;
            }

            let idle_mins = (now - session.last_activity_at).num_minutes();
            let action = decide_stall(
                scheds.get(&window),
                idle_mins,
                outstanding,
                policy.stall_mins,
                policy.nudge_retries,
                now,
            );

            match action {
                StallAction::Nothing => {}
                StallAction::Nudge => {
                    let nudge_text = policy.nudge_text.clone();
                    let seed = AuditSeed {
                        lane_id: lane.id,
                        window: window.clone(),
                        session_id: session.session_id.clone(),
                        agent_kind: Some(session.agent.as_str().to_string()),
                        trigger: "stall".to_string(),
                        dialog_class: None,
                        repo_scoped: None,
                        decision: "nudge".to_string(),
                        policy_source: None,
                        reason: None,
                        subject: None,
                        pane_excerpt: None,
                    };
                    let _ = inject::verified_send(
                        ctx,
                        Expectation::IdleNoDialog,
                        Payload::VerifiedLine {
                            marker: nudge_text.clone(),
                            text: nudge_text,
                        },
                        seed,
                    )
                    .await;
                    bump_stall_sched(scheds, &window, now);
                }
                StallAction::Escalate => {
                    let nudges_sent = scheds.get(&window).map(|s| s.nudges_sent).unwrap_or(0);
                    if let Some(sched) = scheds.get_mut(&window) {
                        sched.escalated = true;
                    }
                    let seed = AuditSeed {
                        lane_id: lane.id,
                        window: window.clone(),
                        session_id: session.session_id.clone(),
                        agent_kind: Some(session.agent.as_str().to_string()),
                        trigger: "stall".to_string(),
                        dialog_class: None,
                        repo_scoped: None,
                        decision: "hold".to_string(),
                        policy_source: None,
                        reason: Some(format!("escalated after {nudges_sent} nudges")),
                        subject: None,
                        pane_excerpt: None,
                    };
                    inject::record_hold(ctx, seed).await;
                    ctx.broadcast(
                        "event.notification",
                        stall_escalate_payload(lane, idle_mins),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use repomon_core::agent::backend::{
        AttachCommand, ByteStream, CaptureOpts, OwnerState, ScrollEvent, SessionBackend, SpawnSpec,
        WindowActivity,
    };
    use repomon_core::agent::prompt::detect_dialog;
    use repomon_core::agent::supervision::{
        DialogClass, PolicyAction, PolicySource, SupervisionOverrides,
    };
    use repomon_core::model::{AgentKind, AgentStatus};
    use repomon_core::{Config, Store};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex as StdMutex};

    async fn test_ctx() -> Arc<Ctx> {
        let store = Store::open_in_memory().unwrap();
        let mut config = Config::default();
        config.supervision.enabled = true;
        Ctx::new(store, config, None)
    }

    // ---- Pure-function tests for supervise_dialog ----

    #[test]
    fn master_off_is_nothing() {
        let decision = Decision {
            action: PolicyAction::AutoApprove,
            choice: Some(0),
            source: PolicySource::LaneClass,
            reason: "approved".to_string(),
        };
        let action = supervise_dialog(false, true, false, true, &decision);
        assert_eq!(action, LoopAction::Nothing);
    }

    #[test]
    fn lane_disabled_is_nothing() {
        let decision = Decision {
            action: PolicyAction::AutoApprove,
            choice: Some(0),
            source: PolicySource::LaneClass,
            reason: "approved".to_string(),
        };
        let action = supervise_dialog(true, false, false, true, &decision);
        assert_eq!(action, LoopAction::Nothing);
    }

    #[test]
    fn external_session_is_nothing() {
        let decision = Decision {
            action: PolicyAction::AutoApprove,
            choice: Some(0),
            source: PolicySource::LaneClass,
            reason: "approved".to_string(),
        };
        let action = supervise_dialog(true, true, true, true, &decision);
        assert_eq!(action, LoopAction::Nothing);
    }

    #[test]
    fn windowless_session_is_nothing() {
        let decision = Decision {
            action: PolicyAction::AutoApprove,
            choice: Some(0),
            source: PolicySource::LaneClass,
            reason: "approved".to_string(),
        };
        let action = supervise_dialog(true, true, false, false, &decision);
        assert_eq!(action, LoopAction::Nothing);
    }

    #[test]
    fn approve_decision_routes_answer() {
        let decision = Decision {
            action: PolicyAction::AutoApprove,
            choice: Some(1),
            source: PolicySource::LaneClass,
            reason: "approved".to_string(),
        };
        let action = supervise_dialog(true, true, false, true, &decision);
        assert_eq!(action, LoopAction::Answer { choice: 1 });
    }

    #[test]
    fn deny_decision_routes_deny() {
        let decision = Decision {
            action: PolicyAction::AutoDeny,
            choice: Some(2),
            source: PolicySource::LaneClass,
            reason: "denied".to_string(),
        };
        let action = supervise_dialog(true, true, false, true, &decision);
        assert_eq!(action, LoopAction::Deny { choice: 2 });
    }

    #[test]
    fn hold_decision_routes_hold() {
        let decision = Decision {
            action: PolicyAction::Hold,
            choice: None,
            source: PolicySource::GlobalClass,
            reason: "held".to_string(),
        };
        let action = supervise_dialog(true, true, false, true, &decision);
        assert_eq!(action, LoopAction::Hold);
    }

    // ---- ScriptedBackend for integration tests ----

    struct ScriptedBackend {
        captures: StdMutex<Vec<String>>,
        sent_keys: StdMutex<Vec<(String, String)>>,
        sent_text: StdMutex<Vec<(String, String)>>,
        capture_calls: std::sync::atomic::AtomicU32,
    }

    impl ScriptedBackend {
        fn new(captures: Vec<String>) -> Self {
            Self {
                captures: StdMutex::new(captures),
                sent_keys: StdMutex::new(Vec::new()),
                sent_text: StdMutex::new(Vec::new()),
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
            Some(repomon_core::agent::Cursor { col: 0, row: 0 })
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
        fn send_text_named(&self, window: &str, text: &str) -> repomon_core::Result<()> {
            self.sent_text
                .lock()
                .unwrap()
                .push((window.to_string(), text.to_string()));
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
        let mut config = Config::default();
        config.supervision.enabled = true;
        Ctx::new_with_backend(
            store,
            config,
            None,
            PathBuf::from("/tmp/config.toml"),
            PathBuf::from("/tmp/repo-notes"),
            backend,
        )
    }

    fn sample_session(dialog: Option<PendingDialog>) -> AgentSession {
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
            session_id: Some("sess-123".into()),
            resume_at: None,
            inferred: false,
            tmux_window: Some("win-lane-1".into()),
            last_message: None,
            pending_prompt: dialog.as_ref().map(|d| d.summary()),
            pending_dialog: dialog,
            stale: false,
            stalled_since: None,
            subagent_running: None,
            status_reason: None,
            attention_kind: None,
            ended_turn: false,
            gate: None,
            config_dir: None,
            custom_label: None,
            generated_label: None,
        }
    }

    const BOXED_DIALOG_PANE: &str = "● Running cargo build…\n\
        ╭──────────────────────────────────────────────╮\n\
        │ Bash command                                 │\n\
        │                                              │\n\
        │   cargo build                                │\n\
        │   Build workspace                            │\n\
        │                                              │\n\
        │ Do you want to proceed?                      │\n\
        │ ❯ 1. Yes                                     │\n\
        │   2. No                                      │\n\
        ╰──────────────────────────────────────────────╯";

    #[tokio::test]
    async fn supervised_dialog_is_answered_end_to_end() {
        let backend = Arc::new(ScriptedBackend::new(vec![BOXED_DIALOG_PANE.to_string()]));
        let ctx = make_ctx(backend.clone());

        // Configure lane policy with CommandExec = AutoApprove
        let p = SupervisionOverrides {
            lane_id: 1,
            enabled: true,
            classes: [(DialogClass::CommandExec, PolicyAction::AutoApprove)]
                .into_iter()
                .collect(),
            nudge_text: None,
            stall_mins: None,
            nudge_retries: None,
            expect_work: true,
            updated_at: Utc::now(),
        };
        ctx.store.set_lane_policy(p).await.unwrap();
        refresh(&ctx).await;
        let policy = supervised(&ctx, 1).await.expect("supervised policy");

        let dialog = detect_dialog(BOXED_DIALOG_PANE).expect("dialog detected");
        let session = sample_session(Some(dialog));

        let mut held_cache = HashMap::new();
        handle_session(
            &ctx,
            1,
            "test-repo",
            Path::new("/repo"),
            Path::new("/repo/wt"),
            &policy,
            &session,
            &mut held_cache,
        )
        .await;

        // Keys were sent (Enter)
        let keys = backend.sent_keys.lock().unwrap().clone();
        assert_eq!(keys, vec![("win-lane-1".to_string(), "Enter".to_string())]);

        // Audit row written
        let log = ctx.store.supervision_log(Some(1), 10, None).await.unwrap();
        assert_eq!(log.len(), 1);
        let entry = &log[0];
        assert_eq!(entry.trigger, "dialog");
        assert_eq!(entry.decision, "approve");
        assert_eq!(entry.outcome, "sent");
        assert_eq!(entry.lane_id, 1);
        assert_eq!(entry.window, "win-lane-1");
    }

    #[tokio::test]
    async fn held_dialog_recorded_once() {
        let backend = Arc::new(ScriptedBackend::new(vec![]));
        let ctx = make_ctx(backend.clone());

        // Configure lane policy with CommandExec = Hold
        let p = SupervisionOverrides {
            lane_id: 1,
            enabled: true,
            classes: [(DialogClass::CommandExec, PolicyAction::Hold)]
                .into_iter()
                .collect(),
            nudge_text: None,
            stall_mins: None,
            nudge_retries: None,
            expect_work: true,
            updated_at: Utc::now(),
        };
        ctx.store.set_lane_policy(p).await.unwrap();
        refresh(&ctx).await;
        let policy = supervised(&ctx, 1).await.expect("supervised policy");

        let dialog = detect_dialog(BOXED_DIALOG_PANE).expect("dialog detected");
        let session = sample_session(Some(dialog));

        let mut held_cache = HashMap::new();

        // First handle_session records hold
        handle_session(
            &ctx,
            1,
            "test-repo",
            Path::new("/repo"),
            Path::new("/repo/wt"),
            &policy,
            &session,
            &mut held_cache,
        )
        .await;

        let log = ctx.store.supervision_log(Some(1), 10, None).await.unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].trigger, "dialog");
        assert_eq!(log[0].decision, "hold");
        assert_eq!(log[0].outcome, "held");

        // Second handle_session with the same held dialog produces NO duplicate log
        handle_session(
            &ctx,
            1,
            "test-repo",
            Path::new("/repo"),
            Path::new("/repo/wt"),
            &policy,
            &session,
            &mut held_cache,
        )
        .await;

        let log2 = ctx.store.supervision_log(Some(1), 10, None).await.unwrap();
        assert_eq!(log2.len(), 1);
    }

    #[tokio::test]
    async fn snapshot_only_contains_enabled_lanes() {
        let ctx = test_ctx().await;

        // Lane 1: enabled
        let p1 = SupervisionOverrides {
            lane_id: 1,
            enabled: true,
            classes: [(DialogClass::CommandExec, PolicyAction::AutoApprove)]
                .into_iter()
                .collect(),
            nudge_text: None,
            stall_mins: None,
            nudge_retries: None,
            expect_work: true,
            updated_at: Utc::now(),
        };
        ctx.store.set_lane_policy(p1).await.unwrap();

        // Lane 2: disabled
        let p2 = SupervisionOverrides {
            lane_id: 2,
            enabled: false,
            classes: std::collections::BTreeMap::new(),
            nudge_text: None,
            stall_mins: None,
            nudge_retries: None,
            expect_work: false,
            updated_at: Utc::now(),
        };
        ctx.store.set_lane_policy(p2).await.unwrap();

        let snapshot = rebuild_snapshot(&ctx).await;
        assert!(snapshot.master);
        assert_eq!(snapshot.lanes.len(), 1);
        assert!(snapshot.lane(1).is_some());
        assert!(snapshot.lane(2).is_none());
        assert!(snapshot.any_enabled());
    }

    #[tokio::test]
    async fn snapshot_empty_when_master_off() {
        let store = Store::open_in_memory().unwrap();
        let mut config = Config::default();
        config.supervision.enabled = false; // master OFF
        let ctx = Ctx::new(store, config, None);

        // Lane 1: explicitly enabled in DB, but master is OFF
        let p1 = SupervisionOverrides {
            lane_id: 1,
            enabled: true,
            classes: std::collections::BTreeMap::new(),
            nudge_text: None,
            stall_mins: None,
            nudge_retries: None,
            expect_work: false,
            updated_at: Utc::now(),
        };
        ctx.store.set_lane_policy(p1).await.unwrap();

        let snapshot = rebuild_snapshot(&ctx).await;
        assert!(!snapshot.master);
        assert!(snapshot.lanes.is_empty());
        assert!(!snapshot.any_enabled());
        assert!(snapshot.lane(1).is_none());
    }

    #[tokio::test]
    async fn supervision_step_refreshes_a_stale_snapshot_before_master_gate() {
        let backend = Arc::new(ScriptedBackend::new(vec![]));
        let ctx = make_ctx(backend.clone());
        ctx.store
            .set_lane_policy(SupervisionOverrides {
                lane_id: 1,
                enabled: true,
                classes: std::collections::BTreeMap::new(),
                nudge_text: Some("must not send".into()),
                stall_mins: Some(5),
                nudge_retries: Some(1),
                expect_work: true,
                updated_at: Utc::now(),
            })
            .await
            .unwrap();
        refresh(&ctx).await;
        assert!(ctx.supervision.read().await.lane(1).is_some());

        ctx.config.write().await.supervision.enabled = false;
        let mut held = HashMap::new();
        let mut stalls = HashMap::new();
        supervision_step(&ctx, &mut held, &mut stalls).await;

        let snapshot = ctx.supervision.read().await;
        assert!(!snapshot.master);
        assert!(snapshot.lanes.is_empty());
        drop(snapshot);
        assert!(backend.sent_text.lock().unwrap().is_empty());
        assert!(backend.sent_keys.lock().unwrap().is_empty());
        assert!(
            ctx.store
                .supervision_log(Some(1), 10, None)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn supervised_reads_refreshed_state() {
        let ctx = test_ctx().await;

        assert_eq!(supervised(&ctx, 10).await, None);

        let p = SupervisionOverrides {
            lane_id: 10,
            enabled: true,
            classes: [(DialogClass::Deletion, PolicyAction::AutoDeny)]
                .into_iter()
                .collect(),
            nudge_text: Some("nudge 10".into()),
            stall_mins: Some(20),
            nudge_retries: Some(2),
            expect_work: true,
            updated_at: Utc::now(),
        };
        ctx.store.set_lane_policy(p).await.unwrap();

        // Before refresh, cache doesn't have it
        assert_eq!(supervised(&ctx, 10).await, None);

        // After refresh, cache is updated
        refresh(&ctx).await;
        let pol = supervised(&ctx, 10).await.expect("supervised");
        assert!(pol.enabled);
        assert_eq!(pol.nudge_text, "nudge 10");
        assert_eq!(pol.stall_mins, 20);
        assert_eq!(pol.nudge_retries, 2);
    }

    // ---- Lane/session fixtures -------------------------------------------------------------

    use repomon_core::model::{AgentAddress, Repo, ResolvedAgentAddress, Worktree, WorktreeState};

    fn lane_with_session(lane_id: LaneId, session: AgentSession) -> Lane {
        let head = "0000000000000000000000000000000000000000".parse().unwrap();
        Lane {
            id: lane_id,
            repo: Repo {
                id: 1,
                path: PathBuf::from("/code/alpha"),
                name: "alpha".into(),
                added_at: Utc::now(),
                worktree_root_template: None,
                hidden: false,
                position: None,
                label: None,
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
                merged: false,
                last_change_at: None,
            },
            agent_sessions: vec![session],
            last_activity_at: Utc::now(),
            pinned: false,
            role: None,
        }
    }

    // ---- Supervised stall nudge & escalation (T10) ----

    fn stall_policy_overrides(
        lane_id: LaneId,
        stall_mins: u32,
        nudge_retries: u32,
        expect_work: bool,
    ) -> SupervisionOverrides {
        SupervisionOverrides {
            lane_id,
            enabled: true,
            classes: std::collections::BTreeMap::new(),
            nudge_text: Some("please continue".into()),
            stall_mins: Some(stall_mins),
            nudge_retries: Some(nudge_retries),
            expect_work,
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn decide_stall_table() {
        let now = Utc::now();

        // No outstanding work: Nothing regardless of idle time.
        assert_eq!(
            decide_stall(None, 1000, false, 5, 2, now),
            StallAction::Nothing
        );

        // Outstanding, but under the idle threshold: Nothing.
        assert_eq!(decide_stall(None, 2, true, 5, 2, now), StallAction::Nothing);

        // Outstanding, over threshold, no prior sched: first nudge.
        assert_eq!(decide_stall(None, 10, true, 5, 2, now), StallAction::Nudge);

        // One nudge already sent, still inside NUDGE_SPACING: Nothing.
        let within_spacing = StallSched {
            nudges_sent: 1,
            last_nudge_at: Some(now - chrono::Duration::minutes(2)),
            escalated: false,
        };
        assert_eq!(
            decide_stall(Some(&within_spacing), 10, true, 5, 2, now),
            StallAction::Nothing
        );

        // Spacing elapsed, retries remain: nudge again.
        let spacing_elapsed = StallSched {
            nudges_sent: 1,
            last_nudge_at: Some(now - chrono::Duration::minutes(6)),
            escalated: false,
        };
        assert_eq!(
            decide_stall(Some(&spacing_elapsed), 10, true, 5, 2, now),
            StallAction::Nudge
        );

        // Retries exhausted: escalate.
        let retries_exhausted = StallSched {
            nudges_sent: 2,
            last_nudge_at: Some(now - chrono::Duration::minutes(6)),
            escalated: false,
        };
        assert_eq!(
            decide_stall(Some(&retries_exhausted), 10, true, 5, 2, now),
            StallAction::Escalate
        );

        // Escalated latch: Nothing, forever.
        let escalated = StallSched {
            nudges_sent: 2,
            last_nudge_at: Some(now - chrono::Duration::minutes(100)),
            escalated: true,
        };
        assert_eq!(
            decide_stall(Some(&escalated), 10, true, 5, 2, now),
            StallAction::Nothing
        );
    }

    #[tokio::test]
    async fn queued_mail_never_becomes_a_wrong_window_stall_nudge() {
        let backend = Arc::new(ScriptedBackend::new(vec![]));
        let ctx = make_ctx(backend.clone());
        ctx.store
            .set_lane_policy(stall_policy_overrides(1, 5, 2, false))
            .await
            .unwrap();
        refresh(&ctx).await;
        let snapshot = ctx.supervision.read().await.clone();

        ctx.store
            .send_message(
                AgentAddress::new("lane-1/2"),
                ResolvedAgentAddress {
                    address: AgentAddress::new("lane-9/1"),
                    lane_id: Some(9),
                    slot: Some(1),
                    window: Some("lane-9".into()),
                    session_id: Some("sender".into()),
                    agent_kind: Some("claude-code".into()),
                },
                ResolvedAgentAddress {
                    address: AgentAddress::new("lane-1/2"),
                    lane_id: Some(1),
                    slot: Some(2),
                    window: Some("win-lane-1-2".into()),
                    session_id: Some("other-recipient".into()),
                    agent_kind: Some("claude-code".into()),
                },
                "mail for another pane".into(),
                None,
            )
            .await
            .unwrap();

        let mut session = sample_session(None);
        session.status = AgentStatus::Waiting;
        let now = Utc::now();
        session.last_activity_at = now - chrono::Duration::minutes(30);
        ctx.pane_seen.lock().await.insert(
            "win-lane-1".to_string(),
            (1, now - chrono::Duration::minutes(30)),
        );
        let mut scheds = HashMap::new();

        stall_phase(
            &ctx,
            &[lane_with_session(1, session)],
            &snapshot,
            &mut scheds,
            now,
        )
        .await;

        assert!(backend.sent_text.lock().unwrap().is_empty());
        assert!(scheds.is_empty());
        assert!(
            ctx.store
                .supervision_log(Some(1), 10, None)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn stall_nudge_sends_and_journals() {
        let backend = Arc::new(ScriptedBackend::new(vec![
            String::new(),
            "› Ask Codex to do anything".to_string(),
            "› please continue".to_string(),
            "• Working\n\n› Ask Codex to do anything".to_string(),
        ]));
        let ctx = make_ctx(backend.clone());

        ctx.store
            .set_lane_policy(stall_policy_overrides(1, 5, 2, true))
            .await
            .unwrap();
        refresh(&ctx).await;
        let snapshot = ctx.supervision.read().await.clone();

        let mut session = sample_session(None);
        session.status = AgentStatus::Waiting;
        let now = Utc::now();
        session.last_activity_at = now - chrono::Duration::minutes(30);

        ctx.pane_seen.lock().await.insert(
            "win-lane-1".to_string(),
            (1u64, now - chrono::Duration::minutes(10)),
        );

        let lane = lane_with_session(1, session);
        let mut scheds = HashMap::new();

        stall_phase(&ctx, &[lane], &snapshot, &mut scheds, now).await;

        let sent = backend.sent_text.lock().unwrap().clone();
        assert_eq!(
            sent,
            vec![("win-lane-1".to_string(), "please continue".to_string())]
        );

        let sched = scheds.get("win-lane-1").expect("sched recorded");
        assert_eq!(sched.nudges_sent, 1);
        assert!(!sched.escalated);

        let log = ctx.store.supervision_log(Some(1), 10, None).await.unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].trigger, "stall");
        assert_eq!(log[0].decision, "nudge");
        assert_eq!(log[0].outcome, "sent");
        assert_eq!(
            backend.sent_keys.lock().unwrap().as_slice(),
            &[("win-lane-1".to_string(), "Enter".to_string())]
        );
    }

    #[tokio::test]
    async fn failed_stall_nudge_is_journaled_and_consumes_one_watchdog_attempt() {
        let backend = Arc::new(ScriptedBackend::new(vec![
            String::new(),
            "› Ask Codex to do anything".to_string(),
            "› please continue".to_string(),
            "› please continue".to_string(),
            "› please continue".to_string(),
        ]));
        let ctx = make_ctx(backend.clone());

        ctx.store
            .set_lane_policy(stall_policy_overrides(1, 5, 2, true))
            .await
            .unwrap();
        refresh(&ctx).await;
        let snapshot = ctx.supervision.read().await.clone();

        let mut session = sample_session(None);
        session.status = AgentStatus::Waiting;
        let now = Utc::now();
        session.last_activity_at = now - chrono::Duration::minutes(30);
        ctx.pane_seen.lock().await.insert(
            "win-lane-1".to_string(),
            (1u64, now - chrono::Duration::minutes(10)),
        );

        let mut scheds = HashMap::new();
        stall_phase(
            &ctx,
            &[lane_with_session(1, session)],
            &snapshot,
            &mut scheds,
            now,
        )
        .await;

        let sched = scheds.get("win-lane-1").expect("attempt recorded");
        assert_eq!(sched.nudges_sent, 1);
        assert!(!sched.escalated);
        assert_eq!(backend.sent_keys.lock().unwrap().len(), 2);

        let log = ctx.store.supervision_log(Some(1), 10, None).await.unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].trigger, "stall");
        assert_eq!(log[0].outcome, "failed");
        assert!(
            log[0]
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("composer still contains please continue"))
        );
    }

    #[tokio::test]
    async fn escalation_notifies_once_and_latches() {
        let backend = Arc::new(ScriptedBackend::new(vec![]));
        let ctx = make_ctx(backend.clone());
        let mut events = ctx.events.subscribe();

        // nudge_retries = 1: the second nudge attempt (after spacing) finds retries exhausted.
        ctx.store
            .set_lane_policy(stall_policy_overrides(1, 5, 1, true))
            .await
            .unwrap();
        refresh(&ctx).await;
        let snapshot = ctx.supervision.read().await.clone();

        let mut session = sample_session(None);
        session.status = AgentStatus::Waiting;
        let t0 = Utc::now();
        session.last_activity_at = t0 - chrono::Duration::minutes(30);

        ctx.pane_seen.lock().await.insert(
            "win-lane-1".to_string(),
            (1u64, t0 - chrono::Duration::minutes(1000)),
        );

        let lane = lane_with_session(1, session);
        let mut scheds = HashMap::new();

        // Tick 1: fresh episode -> nudge.
        stall_phase(
            &ctx,
            std::slice::from_ref(&lane),
            &snapshot,
            &mut scheds,
            t0,
        )
        .await;
        assert_eq!(scheds.get("win-lane-1").unwrap().nudges_sent, 1);
        assert!(!scheds.get("win-lane-1").unwrap().escalated);

        // Tick 2: spacing elapsed, retries (1) exhausted -> escalate.
        let t1 = t0 + chrono::Duration::minutes(6);
        stall_phase(
            &ctx,
            std::slice::from_ref(&lane),
            &snapshot,
            &mut scheds,
            t1,
        )
        .await;
        assert!(scheds.get("win-lane-1").unwrap().escalated);

        // Tick 3 (extra tick, well past spacing again): the escalation latch must not repeat.
        let t2 = t1 + chrono::Duration::minutes(6);
        stall_phase(&ctx, &[lane], &snapshot, &mut scheds, t2).await;

        let mut notifications = 0;
        while let Ok(value) = events.try_recv() {
            if value["method"] == "event.notification" {
                notifications += 1;
                assert_eq!(value["params"]["kind"], "needs_you");
            }
        }
        assert_eq!(notifications, 1, "escalation must notify exactly once");

        let log = ctx.store.supervision_log(Some(1), 10, None).await.unwrap();
        let holds: Vec<_> = log
            .iter()
            .filter(|e| e.trigger == "stall" && e.decision == "hold")
            .collect();
        assert_eq!(holds.len(), 1, "escalation must journal exactly once");
        assert_eq!(holds[0].outcome, "held");
    }

    #[tokio::test]
    async fn activity_clears_stall_sched() {
        let backend = Arc::new(ScriptedBackend::new(vec![]));
        let ctx = make_ctx(backend.clone());

        ctx.store
            .set_lane_policy(stall_policy_overrides(1, 5, 2, true))
            .await
            .unwrap();
        refresh(&ctx).await;
        let snapshot = ctx.supervision.read().await.clone();

        let mut session = sample_session(None);
        session.status = AgentStatus::Waiting;
        let t0 = Utc::now();
        session.last_activity_at = t0 - chrono::Duration::minutes(30);

        // Pane frozen well past any threshold used in this test — quiet throughout.
        ctx.pane_seen.lock().await.insert(
            "win-lane-1".to_string(),
            (1u64, t0 - chrono::Duration::minutes(1000)),
        );

        let mut scheds = HashMap::new();

        // Episode 1: idle past threshold -> nudge sent, sched recorded.
        let lane1 = lane_with_session(1, session.clone());
        stall_phase(
            &ctx,
            std::slice::from_ref(&lane1),
            &snapshot,
            &mut scheds,
            t0,
        )
        .await;
        assert_eq!(scheds.get("win-lane-1").unwrap().nudges_sent, 1);

        // Activity: the session's last_activity_at moves past the last nudge time.
        session.last_activity_at = t0 + chrono::Duration::minutes(1);
        let t1 = t0 + chrono::Duration::minutes(2);
        let lane2 = lane_with_session(1, session.clone());
        stall_phase(
            &ctx,
            std::slice::from_ref(&lane2),
            &snapshot,
            &mut scheds,
            t1,
        )
        .await;
        assert!(
            !scheds.contains_key("win-lane-1"),
            "activity past the last nudge must drop the sched"
        );

        // Episode 2: idle again -> a fresh episode nudges again (nudges_sent restarts at 1).
        session.last_activity_at = t1 - chrono::Duration::minutes(30);
        let t2 = t1 + chrono::Duration::minutes(1);
        let lane3 = lane_with_session(1, session);
        stall_phase(&ctx, &[lane3], &snapshot, &mut scheds, t2).await;
        assert_eq!(
            scheds.get("win-lane-1").unwrap().nudges_sent,
            1,
            "fresh episode restarts the nudge count"
        );

        // Two distinct stall episodes each made one attempt (the second may be latch-skipped by
        // `inject.rs`'s own anti-thrash cooldown, since it's the same window/text within the same
        // wall-clock second — that's a separate, correct safety net, not this feature's concern).
        let log = ctx.store.supervision_log(Some(1), 10, None).await.unwrap();
        let stall_rows: Vec<_> = log.iter().filter(|e| e.trigger == "stall").collect();
        assert_eq!(
            stall_rows.len(),
            2,
            "each episode makes exactly one attempt"
        );
    }
}
