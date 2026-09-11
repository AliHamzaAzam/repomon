//! Volatile delivery state for successful daemon submissions. Transcript consumption is authoritative.
use super::*;
use std::collections::HashMap;
use std::sync::Mutex;

pub(super) const INPUT_SENT: &str = "event.agent.input_sent";
#[derive(Default)]
pub struct Inputs {
    serial: AtomicU64,
    windows: Mutex<HashMap<String, WindowInputs>>,
}
#[derive(Default)]
struct WindowInputs {
    pending: Vec<Ticket>,
    aliases: HashMap<String, String>,
}
pub struct Ticket {
    source: Option<Source>,
    floor: u64,
    item: TranscriptItem,
    prior_prompts: Option<Vec<String>>,
    consumed: bool,
    submitted: String,
}

pub async fn prepare_input(ctx: &Ctx, lane: LaneId, window: &str, text: &str) -> Option<Ticket> {
    if repomon_core::usage_ledger::scan::strip_injected_blocks(text)
        .trim()
        .is_empty()
    {
        return None;
    }
    let backend = ctx.backend.clone();
    let target = window.to_string();
    let pane =
        tokio::task::spawn_blocking(move || backend.capture_named(&target, CaptureOpts::last(100)))
            .await
            .ok()
            .and_then(Result::ok);
    prepare_input_from_pane(ctx, lane, window, text, pane.as_deref()).await
}

/// Verified injection already captured the pane; do not add a second observation to its sequence.
pub async fn prepare_input_from_pane(
    ctx: &Ctx,
    lane: LaneId,
    window: &str,
    text: &str,
    pane: Option<&str>,
) -> Option<Ticket> {
    let cleaned = repomon_core::usage_ledger::scan::strip_injected_blocks(text);
    if cleaned.trim().is_empty() || repomon_core::agent::conversation::is_slash_command(&cleaned) {
        return None;
    }
    let p = Params {
        lane_id: lane,
        session_id: None,
        window: Some(window.into()),
        kind: None,
        before: None,
        on: true,
    };
    let source = resolve_source(ctx, &p, false, None).await.ok();
    let floor = source.as_ref().map_or(0, |s| {
        if matches!(s.kind.as_str(), "opencode" | "hermes") {
            chrono::Utc::now().timestamp_millis().max(0) as u64
        } else {
            s.path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .map_or(0, |m| m.len())
        }
    });
    let prior_prompts = source.as_ref().zip(pane).map(|(s, pane)| {
        repomon_core::agent::conversation_queue::pane_inputs(&s.kind, pane).consumed
    });
    let now = chrono::Utc::now();
    let mut parsed = repomon_core::agent::repomail::split(cleaned.trim(), Some(now));
    let mut item = if parsed.len() == 1 {
        parsed.remove(0)
    } else {
        TranscriptItem::new("user", cleaned.trim(), Some(now))
    };
    item.partial = Some(true);
    Some(Ticket {
        source,
        floor,
        item,
        prior_prompts,
        consumed: false,
        submitted: cleaned.trim().into(),
    })
}
impl Inputs {
    pub fn sent(&self, ctx: &Ctx, window: &str, mut ticket: Ticket) {
        ticket.item.id = Some(format!(
            "sent:{}",
            self.serial.fetch_add(1, Ordering::Relaxed)
        ));
        self.windows
            .lock()
            .unwrap()
            .entry(window.into())
            .or_default()
            .pending
            .push(ticket);
        ctx.broadcast(INPUT_SENT, json!({"window": window}));
    }
    pub(super) fn reconcile(
        &self,
        window: &str,
        source: &Source,
        rows: &mut Vec<TranscriptItem>,
    ) -> Vec<String> {
        let mut replaced = Vec::new();
        let mut windows = self.windows.lock().unwrap();
        let Some(state) = windows.get_mut(window) else {
            return replaced;
        };
        if source.path.is_none() {
            // A pane echo alone is not proof of consumption. The pending ticket supplies this
            // mail row until the provider's consumed-prompt region confirms it.
            rows.retain(|row| {
                !row.mail.as_ref().is_some_and(|mail| {
                    state.pending.iter().any(|ticket| {
                        ticket
                            .item
                            .mail
                            .as_ref()
                            .is_some_and(|pending| pending.id == mail.id)
                    })
                })
            });
        }
        for row in rows {
            let Some(id) = row.id.clone() else {
                continue;
            };
            if let Some(alias) = state.aliases.get(&id) {
                replaced.push(id);
                row.id = Some(alias.clone());
                continue;
            }
            if row.role != "user" {
                continue;
            }
            let index = state.pending.iter().position(|pending| {
                let same_source = pending
                    .source
                    .as_ref()
                    .is_none_or(|old| old.path.is_none() || old == source);
                let offset = if matches!(source.kind.as_str(), "opencode" | "hermes") {
                    row.at.map(|t| t.timestamp_millis().max(0) as u64)
                } else {
                    source
                        .path
                        .as_ref()
                        .and_then(|p| id.strip_prefix(&format!("{}:", p.display())))
                        .and_then(|id| id.split(':').next())
                        .and_then(|v| v.parse::<u64>().ok())
                };
                let after_send = if pending.source.as_ref().is_some_and(|s| s.path.is_some()) {
                    offset.is_some_and(|offset| offset >= pending.floor)
                } else {
                    row.at.zip(pending.item.at).is_some_and(|(row, sent)| {
                        if source.kind == "antigravity" {
                            row.timestamp() >= sent.timestamp()
                        } else {
                            row >= sent
                        }
                    })
                };
                let same_mail = row
                    .mail
                    .as_ref()
                    .zip(pending.item.mail.as_ref())
                    .is_some_and(|(row, sent)| row.id == sent.id);
                // A globally unique mail ID is authoritative consumption evidence even if the
                // provider records receipt time before injection completes or collapses whitespace.
                same_source
                    && (same_mail || (after_send && row.text.trim() == pending.item.text.trim()))
            });
            if let Some(index) = index {
                let pending = state.pending.remove(index);
                let alias = pending.item.id.unwrap();
                replaced.push(id.clone());
                state.aliases.insert(id, alias.clone());
                row.id = Some(alias);
            }
        }
        replaced
    }
    pub(super) fn append(
        &self,
        window: &str,
        source: &Source,
        pane: &str,
        items: &mut Vec<TranscriptItem>,
        order: &mut Vec<String>,
    ) -> Value {
        let mut windows = self.windows.lock().unwrap();
        let mut states = serde_json::Map::new();
        let observed = repomon_core::agent::conversation_queue::pane_inputs(&source.kind, pane);
        let mut matched = HashMap::<String, usize>::new();
        let mut queued_text = observed.queued.clone();
        if let Some(window) = windows.get_mut(window) {
            for ticket in &mut window.pending {
                if ticket
                    .source
                    .as_ref()
                    .is_some_and(|s| s.path.is_some() && s != source)
                {
                    continue;
                }
                let id = ticket.item.id.clone().unwrap();
                let text = repomon_core::agent::conversation_queue::normalized(&ticket.submitted);
                let queued = if let Some(index) = queued_text.iter().position(|row| row == &text) {
                    queued_text.remove(index);
                    true
                } else {
                    false
                };
                let prior = ticket
                    .prior_prompts
                    .as_ref()
                    .map(|prompts| prompts.iter().filter(|s| *s == &text).count());
                let current = observed.consumed.iter().filter(|s| *s == &text).count();
                let used = matched.entry(text).or_default();
                let mail_consumed = ticket.item.mail.as_ref().is_some_and(|mail| {
                    observed.consumed.iter().any(|text| {
                        repomon_core::agent::repomail::split(text, None)
                            .iter()
                            .any(|row| row.mail.as_ref().is_some_and(|row| row.id == mail.id))
                    })
                });
                if !queued && (mail_consumed || prior.is_some_and(|prior| current > prior + *used))
                {
                    ticket.consumed = true;
                    *used += 1;
                }
                let state = if ticket.consumed {
                    "consumed"
                } else if queued || (source.kind == "codex" && observed.queue_reported) {
                    "queued"
                } else {
                    "sent"
                };
                states.insert(id.clone(), json!(state));
                order.push(id);
                items.push(ticket.item.clone());
            }
        }
        Value::Object(states)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn antigravity_second_precision_consumes_only_the_new_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _) = crate::transcript::round5_tests::context(dir.path()).await;
        let sent = chrono::DateTime::parse_from_rfc3339("2026-09-11T16:39:12.900Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let ticket = Ticket {
            source: None,
            floor: 0,
            item: TranscriptItem::new("user", "Write a poem", Some(sent)),
            prior_prompts: None,
            consumed: false,
            submitted: "Write a poem".into(),
        };
        ctx.transcript_inputs.sent(&ctx, "lane-1", ticket);
        let src = Source {
            window: "lane-1".into(),
            kind: "antigravity".into(),
            path: Some(dir.path().join("source.jsonl")),
            session: Some("session".into()),
        };
        let mut old = TranscriptItem::new(
            "user",
            "Write a poem",
            Some(sent - chrono::Duration::seconds(2)),
        );
        old.id = Some("old".into());
        let mut rows = vec![old];
        assert!(
            ctx.transcript_inputs
                .reconcile("lane-1", &src, &mut rows)
                .is_empty()
        );
        let mut current = TranscriptItem::new(
            "user",
            "Write a poem",
            chrono::DateTime::from_timestamp(sent.timestamp(), 0),
        );
        current.id = Some("current".into());
        rows.push(current);
        assert_eq!(
            ctx.transcript_inputs.reconcile("lane-1", &src, &mut rows),
            vec!["current"]
        );
        assert_eq!(rows[1].id.as_deref(), Some("sent:0"));
    }
}
