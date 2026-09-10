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
}

pub async fn prepare_input(ctx: &Ctx, lane: LaneId, window: &str, text: &str) -> Option<Ticket> {
    let cleaned = repomon_core::usage_ledger::scan::strip_injected_blocks(text);
    if cleaned.trim().is_empty() {
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
    let source = resolve_source(ctx, &p, false).await.ok();
    let floor = source.as_ref().map_or(0, |s| {
        if s.kind == "opencode" {
            chrono::Utc::now().timestamp_millis().max(0) as u64
        } else {
            s.path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .map_or(0, |m| m.len())
        }
    });
    let mut item = TranscriptItem::new("user", cleaned.trim(), Some(chrono::Utc::now()));
    item.partial = Some(true);
    Some(Ticket {
        source,
        floor,
        item,
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
        rows: &mut [TranscriptItem],
    ) -> Vec<String> {
        let mut replaced = Vec::new();
        let mut windows = self.windows.lock().unwrap();
        let Some(state) = windows.get_mut(window) else {
            return replaced;
        };
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
                let offset = if source.kind == "opencode" {
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
                    row.at
                        .zip(pending.item.at)
                        .is_some_and(|(row, sent)| row >= sent)
                };
                same_source && after_send && row.text.trim() == pending.item.text.trim()
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
        let windows = self.windows.lock().unwrap();
        let mut states = serde_json::Map::new();
        if let Some(window) = windows.get(window) {
            let queued = queue_indicator(&source.kind, pane);
            for ticket in &window.pending {
                if ticket
                    .source
                    .as_ref()
                    .is_some_and(|s| s.path.is_some() && s != source)
                {
                    continue;
                }
                let id = ticket.item.id.clone().unwrap();
                states.insert(id.clone(), json!(if queued { "queued" } else { "sent" }));
                order.push(id);
                items.push(ticket.item.clone());
            }
        }
        Value::Object(states)
    }
}

use repomon_core::agent::conversation_activity::queue_indicator;
