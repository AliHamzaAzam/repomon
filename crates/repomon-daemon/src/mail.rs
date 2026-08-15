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

async fn try_deliver(ctx: &Ctx, message: FleetMessage) {
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
    let Some(slot) = message.recipient.slot else {
        return;
    };
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
    let Some(session) = lanes
        .iter()
        .find(|lane| lane.id == lane_id)
        .and_then(|lane| {
            message
                .recipient
                .window
                .as_deref()
                .and_then(|window| {
                    lane.agent_sessions
                        .iter()
                        .find(|session| session.tmux_window.as_deref() == Some(window))
                })
                .or_else(|| lane.agent_sessions.get(slot.saturating_sub(1) as usize))
        })
    else {
        return;
    };
    if !injection_eligible(session) {
        return;
    }
    let Some(window) = session.tmux_window.clone() else {
        return;
    };
    if message.recipient.window.as_deref() != Some(window.as_str()) {
        return;
    }
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
    use repomon_core::model::{
        AgentAddress, AgentKind, MessageDeliveryState, MessageReadState, ResolvedAgentAddress,
    };
    use std::path::PathBuf;

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
}
