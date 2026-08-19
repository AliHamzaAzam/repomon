//! Supervision policy snapshot, watcher loop, and policy-driven dialog answering.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use repomon_core::agent::approval;
use repomon_core::agent::prompt::{self, PendingDialog};
use repomon_core::agent::supervision::{
    Decision, DialogClass, DialogScope, PolicyAction, SupervisionPolicy, classify_dialog, evaluate,
    resolve,
};
use repomon_core::model::{AgentSession, LaneId};

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

    loop {
        tokio::select! {
            _ = ctx.shutdown.notified() => break,
            _ = tick.tick() => {
                supervision_step(&ctx, &mut held_cache).await;
            }
        }
    }
}

async fn supervision_step(ctx: &Ctx, held_cache: &mut HashMap<String, String>) {
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

    for lane in lanes {
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
        DialogClass, MailDeliveryMode, PolicyAction, PolicySource, SupervisionOverrides,
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
    }

    impl ScriptedBackend {
        fn new(captures: Vec<String>) -> Self {
            Self {
                captures: StdMutex::new(captures),
                sent_keys: StdMutex::new(Vec::new()),
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
            mail_mode: None,
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
            mail_mode: None,
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
            mail_mode: Some(MailDeliveryMode::Nudge),
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
            mail_mode: None,
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
            mail_mode: None,
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
    async fn supervised_reads_refreshed_state() {
        let ctx = test_ctx().await;

        assert_eq!(supervised(&ctx, 10).await, None);

        let p = SupervisionOverrides {
            lane_id: 10,
            enabled: true,
            classes: [(DialogClass::Deletion, PolicyAction::AutoDeny)]
                .into_iter()
                .collect(),
            mail_mode: None,
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
}
