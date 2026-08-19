//! Durable fleet-message delivery into safe managed agent windows.

use std::sync::Arc;
use std::time::Duration;

use repomon_core::model::{AgentSession, AgentStatus, FleetMessage};

use crate::Ctx;

const INJECT_BODY_CHARS: usize = 1000;

pub fn injection_line(message: &FleetMessage) -> String {
    let collapsed: String = message
        .body
        .chars()
        .filter(|value| !value.is_control() || value.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(INJECT_BODY_CHARS)
        .collect();
    let reply_to = message.reply_to.as_deref().unwrap_or("none");
    format!(
        "[REPOMON MAIL id={} from={} reply_to={reply_to}] {collapsed} [END REPOMON MAIL]",
        message.id, message.sender.address
    )
}

pub fn injection_eligible(session: &AgentSession) -> bool {
    session.tmux_window.is_some()
        && session.pending_dialog.is_none()
        && session.pending_prompt.is_none()
        && !session.stale
        && !matches!(
            session.status,
            AgentStatus::Running | AgentStatus::RateLimited
        )
        || session.tmux_window.is_some()
            && session.pending_dialog.is_none()
            && session.pending_prompt.is_none()
            && !session.stale
            && session.ended_turn
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

pub(crate) async fn try_deliver(ctx: &Ctx, message: FleetMessage) {
    let inject = {
        let config = ctx.config.read().await;
        if message.sender.lane_id.is_some() {
            config.message_inject_agents
        } else {
            config.message_inject_operator
        }
    };
    if !inject {
        return;
    }
    let Some(lane_id) = message.recipient.lane_id else {
        return;
    };
    if crate::supervision::supervised(ctx, lane_id).await.is_some() {
        return;
    }
    let lanes = match crate::rpc::lanes_with_agents(ctx).await {
        Ok(lanes) => lanes,
        Err(error) => {
            let _ = ctx
                .store
                .set_message_delivery_error(message.id, error.message)
                .await;
            return;
        }
    };
    let Some(lane) = lanes.iter().find(|l| l.id == lane_id) else {
        return;
    };
    let Some(session) = resolve_recipient_session(lane, &message) else {
        return;
    };
    if !injection_eligible(session) {
        return;
    }
    let Some(window) = session.tmux_window.clone() else {
        return;
    };
    let line = injection_line(&message);
    let backend = ctx.backend.clone();
    let result = tokio::task::spawn_blocking(move || backend.send_text_named(&window, &line)).await;
    match result {
        Ok(Ok(())) => {
            let _ = ctx.store.mark_message_delivered(message.id).await;
        }
        Ok(Err(error)) => {
            let _ = ctx
                .store
                .set_message_delivery_error(message.id, error.to_string())
                .await;
        }
        Err(error) => {
            let _ = ctx
                .store
                .set_message_delivery_error(message.id, error.to_string())
                .await;
        }
    }
}

pub async fn delivery_worker(ctx: Arc<Ctx>) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = ctx.shutdown.notified() => return,
            _ = tick.tick() => {}
        }
        let queued = match ctx.store.queued_messages(100).await {
            Ok(messages) => messages,
            Err(error) => {
                tracing::warn!("message delivery query failed: {error}");
                continue;
            }
        };
        for message in queued {
            try_deliver(&ctx, message).await;
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
    use repomon_core::agent::supervision::SupervisionOverrides;
    use repomon_core::model::{
        AgentAddress, AgentKind, MessageDeliveryState, MessageReadState, ResolvedAgentAddress,
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
            ended_turn: false,
            gate: None,
            config_dir: None,
            custom_label: None,
            generated_label: None,
        }
    }

    #[test]
    fn frame_strips_controls_and_collapses_whitespace() {
        assert_eq!(
            injection_line(&message("hello\n\t fleet\u{7}  now")),
            "[REPOMON MAIL id=mail-1 from=operator reply_to=none] hello fleet now [END REPOMON MAIL]"
        );
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

    // ---- supervised-lane handoff (T9) ----

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
            Ok(String::new())
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

    /// `try_deliver`'s ONE early return for T9: a supervised lane's mail is owned entirely by
    /// `supervision.rs`'s mail phase, so the plain delivery worker must never touch it — the
    /// message stays queued and untouched by the backend.
    #[tokio::test]
    async fn supervised_lane_is_skipped_by_delivery_worker() {
        let backend = Arc::new(ScriptedBackend::new());
        let ctx = make_mail_ctx(backend.clone());

        let policy = SupervisionOverrides {
            lane_id: 2,
            enabled: true,
            classes: std::collections::BTreeMap::new(),
            mail_mode: None,
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

        try_deliver(&ctx, queued.clone()).await;

        assert!(backend.sent_text.lock().unwrap().is_empty());
        assert!(backend.sent_keys.lock().unwrap().is_empty());

        let refreshed = ctx.store.get_message(queued.id.clone()).await.unwrap();
        assert!(
            refreshed.delivered_at.is_none(),
            "supervised lane delivery is owned by the mail phase, not the worker"
        );
    }
}
