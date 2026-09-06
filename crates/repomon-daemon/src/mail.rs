//! Durable fleet-message delivery into safe managed agent windows.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use repomon_core::model::{AgentSession, AgentStatus, FleetMessage, Lane};
use serde_json::json;

use crate::Ctx;
use crate::inject::{self, AuditSeed, Expectation, Payload, SendOutcome};

const DELIVERY_SWEEP: Duration = Duration::from_secs(1);
const FAILURE_NOTIFY_AFTER: u32 = 2;

pub fn injection_line(message: &FleetMessage) -> String {
    let collapsed: String = message
        .body
        .chars()
        .filter(|value| !value.is_control() || value.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let reply_to = message.reply_to.as_deref().unwrap_or("none");
    format!(
        "[REPOMAIL id={} from={} reply_to={reply_to}] {collapsed} [END REPOMAIL]",
        message.id, message.sender.address
    )
}

pub fn injection_eligible(session: &AgentSession) -> bool {
    !session.external
        && session.tmux_window.is_some()
        && session.pending_dialog.is_none()
        && session.pending_prompt.is_none()
        && !session.stale
        && session.status != AgentStatus::RateLimited
        && (session.status != AgentStatus::Running || session.ended_turn)
}

/// Pure routing decision for one queued message. Policy-blocked messages stay durable for inbox
/// pickup, unresolved/busy recipients wait, and only a safe managed pane is injected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryAction {
    PolicyBlocked,
    Wait,
    Inject,
}

fn decide_delivery(
    policy_allows: bool,
    recipient_resolved: bool,
    recipient_eligible: bool,
) -> DeliveryAction {
    if !policy_allows {
        DeliveryAction::PolicyBlocked
    } else if !recipient_resolved || !recipient_eligible {
        DeliveryAction::Wait
    } else {
        DeliveryAction::Inject
    }
}

fn injection_allowed(message: &FleetMessage, inject_agents: bool, inject_operator: bool) -> bool {
    if message.sender.lane_id.is_some() {
        inject_agents
    } else {
        inject_operator
    }
}

pub(crate) fn resolve_recipient_session<'a>(
    lane: &'a repomon_core::model::Lane,
    message: &FleetMessage,
) -> Option<&'a AgentSession> {
    let slot = message.recipient.slot?;
    let session = message
        .recipient
        .window
        .as_deref()
        .and_then(|window| {
            lane.agent_sessions
                .iter()
                .find(|session| session.tmux_window.as_deref() == Some(window))
        })
        .or_else(|| lane.agent_sessions.get(slot.saturating_sub(1) as usize))?;

    let window = session.tmux_window.as_deref()?;
    if message.recipient.window.as_deref() != Some(window) {
        return None;
    }
    Some(session)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AttemptOutcome {
    Delivered,
    Deferred,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryMode {
    Automatic,
    Force,
}

async fn try_deliver(
    ctx: &Ctx,
    lanes: &[Lane],
    message: &FleetMessage,
    mode: DeliveryMode,
) -> AttemptOutcome {
    let policy_allows = if mode == DeliveryMode::Force {
        true
    } else {
        let config = ctx.config.read().await;
        injection_allowed(
            message,
            config.message_inject_agents,
            config.message_inject_operator,
        )
    };
    let Some(lane_id) = message.recipient.lane_id else {
        return AttemptOutcome::Deferred;
    };
    let Some(lane) = lanes.iter().find(|l| l.id == lane_id) else {
        return AttemptOutcome::Deferred;
    };
    let session = resolve_recipient_session(lane, message);
    let action = decide_delivery(
        policy_allows,
        session.is_some(),
        session.is_some_and(injection_eligible),
    );
    if action != DeliveryAction::Inject {
        return AttemptOutcome::Deferred;
    }
    let session = session.expect("inject decision requires a resolved recipient");
    let window = session
        .tmux_window
        .clone()
        .expect("inject decision requires a managed window");
    let seed = AuditSeed {
        lane_id,
        window,
        session_id: session.session_id.clone(),
        agent_kind: Some(session.agent.as_str().to_string()),
        trigger: "mail".to_string(),
        dialog_class: None,
        repo_scoped: None,
        decision: "full_body".to_string(),
        policy_source: None,
        reason: Some("durable push delivery".to_string()),
        subject: None,
        pane_excerpt: None,
    };
    match inject::verified_send(
        ctx,
        Expectation::IdleNoDialog,
        Payload::VerifiedLine {
            text: injection_line(message),
            marker: "[END REPOMAIL]".to_string(),
        },
        seed,
    )
    .await
    {
        SendOutcome::Sent { .. } => {
            match ctx
                .store
                .mark_message_push_delivered(message.id.clone())
                .await
            {
                Ok(_) => AttemptOutcome::Delivered,
                Err(error) => AttemptOutcome::Failed(error.to_string()),
            }
        }
        SendOutcome::Skipped { .. } => AttemptOutcome::Deferred,
        SendOutcome::Failed { error, .. } => AttemptOutcome::Failed(error),
    }
}

/// Attempt one operator-requested delivery immediately. This overrides only the configured
/// sender-class policy for this message; recipient resolution, idle/dialog safety, and the
/// self-verifying composer submission path remain identical to automatic delivery.
pub(crate) async fn force_deliver(
    ctx: &Ctx,
    lanes: &[Lane],
    message: &FleetMessage,
) -> Result<FleetMessage, String> {
    if message.delivered_at.is_some() {
        return Err("message is already delivered".to_string());
    }
    match try_deliver(ctx, lanes, message, DeliveryMode::Force).await {
        AttemptOutcome::Delivered => ctx
            .store
            .get_message(message.id.clone())
            .await
            .map_err(|error| error.to_string()),
        AttemptOutcome::Deferred => {
            Err("recipient is not currently safe for message injection".to_string())
        }
        AttemptOutcome::Failed(error) => {
            let _ = ctx
                .store
                .set_message_delivery_error(message.id.clone(), error.clone())
                .await;
            Err(error)
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct FailureState {
    attempts: u32,
    notified: bool,
}

fn record_failure(state: &mut FailureState) -> bool {
    state.attempts = state.attempts.saturating_add(1);
    if state.attempts < FAILURE_NOTIFY_AFTER || state.notified {
        return false;
    }
    state.notified = true;
    true
}

fn delivery_failure_payload(lane: &Lane, message: &FleetMessage) -> serde_json::Value {
    json!({
        "kind": "needs_you",
        "title": format!("{} needs you", lane.repo.name),
        "body": format!("queued mail could not be delivered to {}", message.recipient.address),
        "lane_id": lane.id,
    })
}

async fn delivery_pass(ctx: &Ctx, failures: &mut HashMap<String, FailureState>) {
    let (inject_agents, inject_operator) = {
        let config = ctx.config.read().await;
        (config.message_inject_agents, config.message_inject_operator)
    };
    let queued = match ctx
        .store
        .queued_messages_for_injection(inject_agents, inject_operator, 200)
        .await
    {
        Ok(messages) => messages,
        Err(error) => {
            tracing::warn!("message delivery query failed: {error}");
            return;
        }
    };
    let queued_ids: HashSet<&str> = queued.iter().map(|message| message.id.as_str()).collect();
    failures.retain(|id, _| queued_ids.contains(id.as_str()));
    if queued.is_empty() {
        return;
    }

    let lanes = match crate::rpc::lanes_with_agents(ctx).await {
        Ok(lanes) => lanes,
        Err(error) => {
            tracing::warn!("message delivery failed to inspect lanes: {error:?}");
            return;
        }
    };

    // Never inject two queued messages into one pane from the same overlay snapshot: the first
    // send may start generation before transcript state catches up. Later messages remain queued
    // for an idle transition or the next fallback sweep.
    let mut attempted_windows = HashSet::new();
    for message in &queued {
        let resolved_window = message
            .recipient
            .lane_id
            .and_then(|lane_id| lanes.iter().find(|lane| lane.id == lane_id))
            .and_then(|lane| resolve_recipient_session(lane, message))
            .filter(|session| injection_eligible(session))
            .and_then(|session| session.tmux_window.as_deref());
        let Some(window) = resolved_window else {
            continue;
        };
        if !attempted_windows.insert(window) {
            continue;
        }
        let lane = message
            .recipient
            .lane_id
            .and_then(|lane_id| lanes.iter().find(|lane| lane.id == lane_id));
        match try_deliver(ctx, &lanes, message, DeliveryMode::Automatic).await {
            AttemptOutcome::Delivered => {
                failures.remove(&message.id);
            }
            AttemptOutcome::Deferred => {}
            AttemptOutcome::Failed(error) => {
                let _ = ctx
                    .store
                    .set_message_delivery_error(message.id.clone(), error)
                    .await;
                let state = failures.entry(message.id.clone()).or_default();
                if record_failure(state) {
                    if let Some(lane) = lane {
                        ctx.broadcast(
                            "event.notification",
                            delivery_failure_payload(lane, message),
                        );
                    }
                }
            }
        }
    }
}

/// Update eligibility for every window present in this overlay and wake delivery on transitions
/// into an injection-safe state. This is deliberately transition-based: unconditional wakes from
/// inside `lanes_with_agents` would make a busy durable queue spin in a tight self-triggered loop.
pub(crate) async fn note_eligible_windows(ctx: &Ctx, lanes: &[Lane]) {
    let mut previous = ctx.mail_eligible_windows.lock().await;
    let mut became_eligible = false;
    for session in lanes.iter().flat_map(|lane| lane.agent_sessions.iter()) {
        let Some(window) = session.tmux_window.as_ref() else {
            continue;
        };
        if injection_eligible(session) {
            became_eligible |= previous.insert(window.clone());
        } else {
            previous.remove(window);
        }
    }
    drop(previous);
    if became_eligible {
        ctx.wake_mail_delivery();
    }
}

pub async fn delivery_worker(ctx: Arc<Ctx>) {
    let mut tick = tokio::time::interval(DELIVERY_SWEEP);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut failures = HashMap::new();
    loop {
        tokio::select! {
            _ = ctx.shutdown.notified() => return,
            _ = tick.tick() => {},
            _ = ctx.mail_delivery.notified() => {},
        }
        delivery_pass(&ctx, &mut failures).await;
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
    use repomon_core::agent::supervision::SupervisionOverrides;
    use repomon_core::model::{
        AgentAddress, AgentKind, MessageDeliveryState, MessageReadState, Repo,
        ResolvedAgentAddress, Worktree, WorktreeState,
    };
    use repomon_core::{Config, Store};
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;

    fn message(body: &str) -> FleetMessage {
        FleetMessage {
            id: "mail-1".into(),
            requested_to: AgentAddress::new("lane-2/1"),
            sender: ResolvedAgentAddress {
                address: AgentAddress::new("operator"),
                lane_id: None,
                slot: None,
                window: None,
                session_id: None,
                agent_kind: None,
            },
            recipient: ResolvedAgentAddress {
                address: AgentAddress::new("lane-2/1"),
                lane_id: Some(2),
                slot: Some(1),
                window: Some("lane-2".into()),
                session_id: Some("session-2".into()),
                agent_kind: Some("claude-code".into()),
            },
            body: body.into(),
            thread_id: "mail-1".into(),
            reply_to: None,
            remaining_hops: 6,
            created_at: Utc::now(),
            delivered_at: None,
            read_at: None,
            delivery_error: None,
            delivery_state: MessageDeliveryState::Queued,
            read_state: MessageReadState::Unread,
        }
    }

    fn session(status: AgentStatus) -> AgentSession {
        AgentSession {
            id: 1,
            agent: AgentKind::ClaudeCode,
            repo_id: 1,
            worktree_id: Some(1),
            started_at: Utc::now(),
            last_activity_at: Utc::now(),
            ended_at: None,
            manifest_path: PathBuf::from("/tmp/session.jsonl"),
            tool_call_count: 0,
            title: None,
            status,
            external: false,
            session_id: Some("session-2".into()),
            resume_at: None,
            inferred: false,
            tmux_window: Some("lane-2".into()),
            last_message: None,
            pending_prompt: None,
            pending_dialog: None,
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

    fn lane_with_session(session: AgentSession) -> Lane {
        let head = "0000000000000000000000000000000000000000".parse().unwrap();
        Lane {
            id: 2,
            repo: Repo {
                id: 2,
                name: "repo-2".into(),
                path: PathBuf::from("/repo-2"),
                added_at: Utc::now(),
                worktree_root_template: None,
                hidden: false,
                position: None,
                label: None,
            },
            worktree: Worktree {
                id: 2,
                repo_id: 2,
                path: PathBuf::from("/repo-2"),
                branch: Some("feat/mail".into()),
                head,
                is_main: false,
                name: "feat-mail".into(),
            },
            state: WorktreeState {
                worktree_id: 2,
                head,
                branch: Some("feat/mail".into()),
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

    #[test]
    fn frame_strips_controls_and_collapses_whitespace() {
        assert_eq!(
            injection_line(&message("hello\n\t fleet\u{7}  now")),
            "[REPOMAIL id=mail-1 from=operator reply_to=none] hello fleet now [END REPOMAIL]"
        );
    }

    #[test]
    fn frame_keeps_the_complete_long_body_and_closing_marker() {
        let body = "x".repeat(8 * 1024);
        let line = injection_line(&message(&body));
        assert!(line.contains(&body));
        assert!(line.ends_with("[END REPOMAIL]"));
    }

    #[test]
    fn delivery_decision_table_respects_policy_resolution_and_safety() {
        assert_eq!(
            decide_delivery(false, true, true),
            DeliveryAction::PolicyBlocked
        );
        assert_eq!(decide_delivery(true, false, true), DeliveryAction::Wait);
        assert_eq!(decide_delivery(true, true, false), DeliveryAction::Wait);
        assert_eq!(decide_delivery(true, true, true), DeliveryAction::Inject);

        let operator = message("operator mail");
        assert!(injection_allowed(&operator, false, true));
        let mut agent = operator;
        agent.sender.lane_id = Some(9);
        assert!(!injection_allowed(&agent, false, true));
        assert!(injection_allowed(&agent, true, false));
    }

    #[test]
    fn repeated_delivery_failure_raises_attention_exactly_once() {
        let mut state = FailureState::default();
        assert!(!record_failure(&mut state));
        assert!(record_failure(&mut state));
        assert!(!record_failure(&mut state));
        assert_eq!(state.attempts, 3);
        assert!(state.notified);
    }

    #[test]
    fn eligibility_rejects_busy_dialog_rate_limit_and_stall() {
        assert!(injection_eligible(&session(AgentStatus::Waiting)));
        assert!(injection_eligible(&session(AgentStatus::Idle)));
        assert!(!injection_eligible(&session(AgentStatus::Running)));
        assert!(!injection_eligible(&session(AgentStatus::RateLimited)));
        let mut dialog = session(AgentStatus::Waiting);
        dialog.pending_prompt = Some("Allow?".into());
        assert!(!injection_eligible(&dialog));
        let mut stalled = session(AgentStatus::Idle);
        stalled.stale = true;
        assert!(!injection_eligible(&stalled));
        let mut ended = session(AgentStatus::Running);
        ended.ended_turn = true;
        assert!(injection_eligible(&ended));
    }

    #[tokio::test]
    async fn pane_became_idle_wakes_delivery_once() {
        let backend = Arc::new(ScriptedBackend::new());
        let ctx = make_mail_ctx(backend);
        let mut busy = session(AgentStatus::Running);
        busy.ended_turn = false;
        note_eligible_windows(&ctx, &[lane_with_session(busy)]).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(10), ctx.mail_delivery.notified())
                .await
                .is_err()
        );

        note_eligible_windows(&ctx, &[lane_with_session(session(AgentStatus::Waiting))]).await;
        tokio::time::timeout(Duration::from_millis(100), ctx.mail_delivery.notified())
            .await
            .expect("busy-to-idle transition should wake delivery");

        note_eligible_windows(&ctx, &[lane_with_session(session(AgentStatus::Waiting))]).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(10), ctx.mail_delivery.notified())
                .await
                .is_err(),
            "unchanged eligibility must not self-trigger a delivery loop"
        );
    }

    struct ScriptedBackend {
        sent_keys: StdMutex<Vec<(String, String)>>,
        sent_text: StdMutex<Vec<(String, String)>>,
    }

    impl ScriptedBackend {
        fn new() -> Self {
            Self {
                sent_keys: StdMutex::new(Vec::new()),
                sent_text: StdMutex::new(Vec::new()),
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
        fn spawn(
            &self,
            _lane: repomon_core::model::LaneId,
            _spec: &SpawnSpec,
        ) -> repomon_core::Result<String> {
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
            Ok("› Ask Codex to do anything".to_string())
        }
        fn cursor_named(&self, _window: &str) -> Option<repomon_core::agent::Cursor> {
            Some(repomon_core::agent::Cursor { col: 2, row: 0 })
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

    fn make_mail_ctx(backend: Arc<dyn SessionBackend>) -> Arc<Ctx> {
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

    #[tokio::test]
    async fn supervised_lane_receives_the_actual_body() {
        let backend = Arc::new(ScriptedBackend::new());
        let ctx = make_mail_ctx(backend.clone());

        let policy = SupervisionOverrides {
            lane_id: 2,
            enabled: true,
            classes: std::collections::BTreeMap::new(),
            nudge_text: None,
            stall_mins: None,
            nudge_retries: None,
            expect_work: true,
            updated_at: Utc::now(),
        };
        ctx.store.set_lane_policy(policy).await.unwrap();
        crate::supervision::refresh(&ctx).await;
        assert!(crate::supervision::supervised(&ctx, 2).await.is_some());

        let queued = ctx
            .store
            .send_message(
                AgentAddress::new("lane-2/1"),
                ResolvedAgentAddress {
                    address: AgentAddress::new("operator"),
                    lane_id: None,
                    slot: None,
                    window: None,
                    session_id: None,
                    agent_kind: None,
                },
                ResolvedAgentAddress {
                    address: AgentAddress::new("lane-2/1"),
                    lane_id: Some(2),
                    slot: Some(1),
                    window: Some("lane-2".into()),
                    session_id: Some("session-2".into()),
                    agent_kind: Some("claude-code".into()),
                },
                "please look at this".into(),
                None,
            )
            .await
            .unwrap();

        let lane = lane_with_session(session(AgentStatus::Waiting));
        let outcome = try_deliver(&ctx, &[lane], &queued, DeliveryMode::Automatic).await;

        assert_eq!(outcome, AttemptOutcome::Delivered);
        let sent = backend.sent_text.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].1.contains("please look at this"));
        assert!(sent[0].1.ends_with("[END REPOMAIL]"));
        assert!(backend.sent_keys.lock().unwrap().is_empty());

        let refreshed = ctx.store.get_message(queued.id.clone()).await.unwrap();
        assert!(
            refreshed.delivered_at.is_some(),
            "push delivery marks supervised mail delivered"
        );
        assert_eq!(refreshed.read_state, MessageReadState::Read);
        assert_eq!(refreshed.delivered_at, refreshed.read_at);
    }

    #[tokio::test]
    async fn force_delivery_overrides_policy_but_keeps_verified_send() {
        let backend = Arc::new(ScriptedBackend::new());
        let ctx = make_mail_ctx(backend.clone());
        {
            let mut config = ctx.config.write().await;
            config.message_inject_operator = false;
        }
        let queued = ctx
            .store
            .send_message(
                AgentAddress::new("lane-2/1"),
                ResolvedAgentAddress {
                    address: AgentAddress::new("operator"),
                    lane_id: None,
                    slot: None,
                    window: None,
                    session_id: None,
                    agent_kind: None,
                },
                ResolvedAgentAddress {
                    address: AgentAddress::new("lane-2/1"),
                    lane_id: Some(2),
                    slot: Some(1),
                    window: Some("lane-2".into()),
                    session_id: Some("session-2".into()),
                    agent_kind: Some("claude-code".into()),
                },
                "force this one message".into(),
                None,
            )
            .await
            .unwrap();
        let lane = lane_with_session(session(AgentStatus::Waiting));

        assert_eq!(
            try_deliver(
                &ctx,
                std::slice::from_ref(&lane),
                &queued,
                DeliveryMode::Automatic,
            )
            .await,
            AttemptOutcome::Deferred,
        );
        let delivered = force_deliver(&ctx, &[lane], &queued).await.unwrap();

        assert!(delivered.delivered_at.is_some());
        assert_eq!(delivered.read_state, MessageReadState::Read);
        assert_eq!(delivered.delivered_at, delivered.read_at);
        assert_eq!(backend.sent_text.lock().unwrap().len(), 1);
        assert!(
            backend.sent_text.lock().unwrap()[0]
                .1
                .ends_with("[END REPOMAIL]")
        );
        assert!(backend.sent_keys.lock().unwrap().is_empty());
    }
}
