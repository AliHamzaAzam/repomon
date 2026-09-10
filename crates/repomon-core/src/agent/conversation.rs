//! Pane previews and stable reconciliation between live output and durable transcript rows.
use super::{prompt, text::strip_ansi};
use crate::model::{ToolCallStatus, TranscriptItem};
use std::collections::{HashMap, HashSet};

/// Strip only recognized CLI decorations. Unknown kinds preserve the raw plain pane tail.
pub fn pane_items(kind: &str, pane: &str) -> Vec<TranscriptItem> {
    let plain = strip_ansi(pane);
    let supported = matches!(kind, "claude-code" | "codex");
    let mut prose = Vec::new();
    let mut tools = Vec::new();
    let mut in_box = false;
    for raw in plain.lines() {
        let line = raw.trim();
        if supported {
            if line.starts_with('╭') {
                in_box = true;
                continue;
            }
            if line.starts_with('╰') {
                in_box = false;
                continue;
            }
            if in_box || line.starts_with('│') || line.starts_with('─') || line.starts_with('╭')
            {
                continue;
            }
            if line.starts_with('❯') || line.starts_with('›') {
                if line.chars().skip(1).any(|c| !c.is_whitespace()) {
                    prose.clear();
                    tools.clear();
                }
                continue;
            }
            if line.contains("esc to interrupt")
                || line.contains("? for shortcuts")
                || line.contains("context left")
                || line.contains("auto mode on")
                || line.contains("shift+tab to cycle")
                || line.contains("new task? /clear")
                || line.starts_with("Tip:")
                || line.starts_with("Learn more:")
                || line.starts_with("✻ Crunched")
                || line.starts_with("✻ Worked")
            {
                continue;
            }
            let body = line.trim_start_matches(['⏺', '●', '•']).trim();
            if let Some((name, input)) = body.split_once('(') {
                if matches!(
                    name,
                    "Bash"
                        | "Read"
                        | "Edit"
                        | "Write"
                        | "Grep"
                        | "Glob"
                        | "Task"
                        | "WebFetch"
                        | "WebSearch"
                ) {
                    let mut item = TranscriptItem::new("tool_call", body, None);
                    item.name = Some(name.into());
                    item.input_summary = Some(input.trim_end_matches(')').into());
                    item.status = Some(ToolCallStatus::Running);
                    tools.push(item);
                    continue;
                }
            }
            if kind == "codex" && (body.starts_with("Running ") || body.starts_with("Ran ")) {
                let mut item = TranscriptItem::new("tool_call", body, None);
                item.name = Some("exec_command".into());
                item.input_summary = Some(body.split_once(' ').map(|s| s.1).unwrap_or("").into());
                item.status = Some(ToolCallStatus::Running);
                tools.push(item);
                continue;
            }
            if line.starts_with('⎿') {
                continue;
            }
            prose.push(if line.starts_with(['⏺', '●', '•']) {
                body.to_string()
            } else {
                raw.trim_end().to_string()
            });
        } else {
            prose.push(raw.to_string());
        }
    }
    let text = prose.join("\n").trim().to_string();
    let mut items = Vec::new();
    if !text.is_empty() {
        let mut item = TranscriptItem::new(
            if supported {
                "assistant"
            } else {
                "terminal_block"
            },
            text,
            None,
        );
        item.partial = Some(true);
        items.push(item);
    }
    items.extend(tools);
    if let Some(dialog) = prompt::detect_dialog(&plain) {
        let mut item = TranscriptItem::new("dialog", &dialog.question, None);
        item.dialog = Some(dialog);
        items.push(item);
    }
    if super::detect_usage_limit(&plain).is_some()
        || prompt::detect_quota_exhausted(&plain).is_some()
    {
        let mut item = TranscriptItem::new("status", "Usage limit reached", None);
        item.status_kind = Some("usage_limit".into());
        items.push(item);
    } else if plain.to_ascii_lowercase().contains("rate limit") {
        let mut item = TranscriptItem::new("status", "Rate limited", None);
        item.status_kind = Some("rate_limit".into());
        items.push(item);
    }
    items
}

/// Find the end of a prior answer despite terminal wrapping, retaining original byte boundaries.
fn prior_answer_end(pane: &str, answer: &str) -> Option<usize> {
    let answer = answer.trim();
    if answer.chars().filter(|c| !c.is_whitespace()).count() < 16 {
        return pane
            .strip_prefix(answer)
            .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
            .map(|_| answer.len());
    }
    let normalized: String = answer.chars().filter(|c| !c.is_whitespace()).collect();
    let mut flat = String::new();
    let mut boundaries = Vec::new();
    for (at, c) in pane.char_indices() {
        if !c.is_whitespace() {
            flat.push(c);
            boundaries.push((flat.len(), at + c.len_utf8()));
        }
    }
    let end = flat.rfind(&normalized)? + normalized.len();
    boundaries
        .into_iter()
        .find(|(n, _)| *n == end)
        .map(|(_, original)| original)
}

fn same_tool_name(a: Option<&str>, b: Option<&str>) -> bool {
    a.map(|name| name.rsplit('.').next().unwrap_or(name))
        == b.map(|name| name.rsplit('.').next().unwrap_or(name))
}

/// Upserts retain their ID as a pane preview becomes a durable row. Removed IDs clear ephemeral
/// dialogs and working indicators without asking clients to discard older history pages.
#[derive(Debug, Default)]
pub struct Update {
    pub items: Vec<TranscriptItem>,
    pub removed_ids: Vec<String>,
}

#[derive(Default)]
pub struct ConversationStream {
    previous: HashMap<String, TranscriptItem>,
    durable: HashSet<String>,
    aliases: HashMap<String, String>,
    pending: Vec<TranscriptItem>,
    serial: u64,
    initialized: bool,
    last_live: Vec<TranscriptItem>,
    was_active: bool,
    settled_tools: HashSet<(Option<String>, Option<String>)>,
}
impl ConversationStream {
    pub fn update(
        &mut self,
        final_rows: Vec<TranscriptItem>,
        mut live: Vec<TranscriptItem>,
        active: bool,
    ) -> Update {
        let mut rows = Vec::new();
        let mut settled = false;
        if active && !self.was_active {
            self.settled_tools.clear();
        }
        // A pane contains recent scrollback too. Strip the last durable answer when it is still
        // visible so a new preview contains only the text that follows it.
        if let Some(answer) = final_rows
            .iter()
            .rev()
            .find(|i| i.kind.as_deref() == Some("assistant") && !i.text.trim().is_empty())
        {
            for item in &mut live {
                if item.kind.as_deref() == Some("assistant") {
                    if let Some(end) = prior_answer_end(&item.text, &answer.text) {
                        item.text = item.text[end..].trim().to_string();
                    }
                }
            }
            live.retain(|i| i.kind.as_deref() != Some("assistant") || !i.text.is_empty());
        }
        for mut row in final_rows {
            let Some(key) = row.id.clone() else {
                continue;
            };
            if self.initialized
                && !self.durable.contains(&key)
                && matches!(row.kind.as_deref(), Some("assistant" | "tool_call"))
            {
                if let Some(index) = self.pending.iter().position(|p| {
                    (p.kind == row.kind
                        || (p.partial == Some(true)
                            && p.kind.as_deref() == Some("terminal_block")
                            && row.kind.as_deref() == Some("assistant")))
                        && (p.kind.as_deref() != Some("tool_call")
                            || same_tool_name(p.name.as_deref(), row.name.as_deref()))
                }) {
                    let pending = self.pending.remove(index);
                    if pending.kind.as_deref() == Some("tool_call") {
                        self.settled_tools
                            .insert((pending.name.clone(), pending.input_summary.clone()));
                    }
                    if let Some(id) = pending.id {
                        self.aliases.insert(key.clone(), id);
                    }
                    settled |= row.kind.as_deref() == Some("assistant");
                }
            }
            if !key.starts_with("pane:") {
                self.durable.insert(key.clone());
            }
            if let Some(id) = self.aliases.get(&key) {
                row.id = Some(id.clone());
            }
            rows.push(row);
        }
        let dialogs: Vec<_> = live
            .iter()
            .filter(|i| matches!(i.kind.as_deref(), Some("dialog" | "status")))
            .cloned()
            .collect();
        live.retain(|i| !matches!(i.kind.as_deref(), Some("dialog" | "status")));
        let changed = live != self.last_live;
        self.last_live = live.clone();
        if active && !settled && changed {
            for mut item in live {
                if item.kind.as_deref() == Some("tool_call")
                    && self
                        .settled_tools
                        .contains(&(item.name.clone(), item.input_summary.clone()))
                {
                    continue;
                }
                if let Some(existing) = self.pending.iter_mut().find(|p| {
                    p.kind == item.kind
                        && p.name == item.name
                        && (p.kind.as_deref() != Some("tool_call")
                            || p.input_summary == item.input_summary)
                }) {
                    item.id = existing.id.clone();
                    item.at = existing.at;
                    *existing = item;
                } else {
                    self.serial += 1;
                    item.id = Some(format!("live:{}", self.serial));
                    item.at = Some(chrono::Utc::now());
                    self.pending.push(item);
                }
            }
        }
        rows.extend(self.pending.clone());
        if active || self.was_active {
            let mut turn = TranscriptItem::new(
                "status",
                if active {
                    "Turn started"
                } else {
                    "Turn finished"
                },
                None,
            );
            turn.id = Some("live:turn".into());
            turn.status_kind = Some(
                if active {
                    "turn_started"
                } else {
                    "turn_finished"
                }
                .into(),
            );
            rows.push(turn);
        }
        self.was_active = active;
        if active {
            let mut working = TranscriptItem::new("status", "Working", None);
            working.id = Some("live:working".into());
            working.status_kind = Some("working".into());
            rows.push(working);
        }
        // Dialogs always come from the current pane, independent of transcript activity.
        for mut item in dialogs {
            item.id = Some(if item.kind.as_deref() == Some("dialog") {
                "live:dialog".into()
            } else {
                format!("live:{}", item.status_kind.as_deref().unwrap_or("status"))
            });
            rows.push(item);
        }
        let next: HashMap<_, _> = rows
            .iter()
            .filter_map(|i| i.id.clone().map(|id| (id, i.clone())))
            .collect();
        let update = Update {
            items: rows
                .into_iter()
                .filter(|item| {
                    item.id
                        .as_ref()
                        .is_some_and(|id| self.previous.get(id) != Some(item))
                })
                .collect(),
            removed_ids: self
                .previous
                .keys()
                .filter(|id| {
                    !next.contains_key(*id)
                        && !self.durable.contains(*id)
                        && !self.aliases.values().any(|alias| alias == *id)
                })
                .cloned()
                .collect(),
        };
        self.previous = next;
        self.initialized = true;
        update
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_claude_fixture_strips_footer_and_preserves_prose_and_tools() {
        let pane = include_str!("fixtures/claude_idle_done_spinner.txt");
        let items = pane_items("claude-code", pane);
        assert!(items.iter().any(|i| i.kind.as_deref() == Some("tool_call")));
        let prose = items
            .iter()
            .find(|i| i.kind.as_deref() == Some("assistant"))
            .unwrap();
        assert!(prose.text.contains("The models are two-tone now"));
        assert!(!prose.text.contains("auto mode on"));
        assert!(!prose.text.contains("Crunched for"));
    }
    #[test]
    fn real_codex_fixture_strips_ansi_boxes_and_cli_banner() {
        let items = pane_items("codex", include_str!("fixtures/codex_status_v0.ansi"));
        assert!(
            items
                .iter()
                .all(|i| !i.text.contains('\u{1b}') && !i.text.contains("OpenAI Codex"))
        );
    }
    #[test]
    fn partial_updates_and_final_replace_one_identity_without_resurrection() {
        for kind in ["claude-code", "codex"] {
            let mut stream = ConversationStream::default();
            stream.update(Vec::new(), Vec::new(), false);
            let first = stream.update(Vec::new(), pane_items(kind, "• Building"), true);
            let item = first
                .items
                .iter()
                .find(|i| i.partial == Some(true))
                .unwrap();
            let id = item.id.clone();
            let next = stream.update(Vec::new(), pane_items(kind, "• Building the view"), true);
            assert_eq!(
                next.items
                    .iter()
                    .find(|i| i.partial == Some(true))
                    .unwrap()
                    .id,
                id
            );
            let mut final_row = TranscriptItem::new("assistant", "Building the view.", None);
            final_row.id = Some("source:100:0".into());
            let final_update = stream.update(
                vec![final_row.clone()],
                pane_items(kind, "• Building the view"),
                true,
            );
            assert!(
                final_update
                    .items
                    .iter()
                    .any(|i| i.id == id && i.partial != Some(true))
            );
            assert!(!final_update.removed_ids.contains(id.as_ref().unwrap()));
            let repeat = stream.update(
                vec![final_row],
                pane_items(kind, "• Building the view"),
                true,
            );
            assert!(!repeat.items.iter().any(|i| i.partial == Some(true)));
        }
    }
    #[test]
    fn dialogs_clear_and_unknown_panes_remain_readable() {
        let mut stream = ConversationStream::default();
        let live = pane_items("claude-code", include_str!("fixtures/trust_prompt.txt"));
        assert!(live.iter().any(|i| i.kind.as_deref() == Some("dialog")));
        let first = stream.update(Vec::new(), live, true);
        assert!(
            first
                .items
                .iter()
                .any(|i| i.id.as_deref() == Some("live:dialog"))
        );
        let cleared = stream.update(Vec::new(), Vec::new(), false);
        assert!(cleared.removed_ids.contains(&"live:dialog".into()));
        let fallback = pane_items("other", "unfamiliar output\nmore text");
        assert_eq!(fallback[0].kind.as_deref(), Some("terminal_block"));
        assert_eq!(fallback[0].text, "unfamiliar output\nmore text");
    }
}

#[cfg(test)]
mod tool_tests {
    use super::*;
    #[test]
    fn live_tool_resolves_in_place_and_status_changes_are_upserts() {
        let mut stream = ConversationStream::default();
        stream.update(Vec::new(), Vec::new(), false);
        let first = stream.update(
            Vec::new(),
            pane_items("claude-code", "⏺ Bash(cargo test)"),
            true,
        );
        let running = first
            .items
            .iter()
            .find(|i| i.kind.as_deref() == Some("tool_call"))
            .unwrap();
        assert_eq!(running.status, Some(ToolCallStatus::Running));
        let mut tool = TranscriptItem::new("tool_call", "Bash cargo test", None);
        tool.id = Some("file:tool:call-1".into());
        tool.name = Some("Bash".into());
        tool.status = Some(ToolCallStatus::Ok);
        let next = stream.update(vec![tool], Vec::new(), false);
        let resolved = next
            .items
            .iter()
            .find(|i| i.kind.as_deref() == Some("tool_call"))
            .unwrap();
        assert_eq!(resolved.id, running.id);
        assert_eq!(resolved.status, Some(ToolCallStatus::Ok));
        assert!(
            next.items
                .iter()
                .any(|i| i.status_kind.as_deref() == Some("turn_finished"))
        );
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;
    #[test]
    fn rolling_watch_tail_does_not_delete_paged_history() {
        let mut stream = ConversationStream::default();
        let mut old = TranscriptItem::new("user", "older page", None);
        old.id = Some("file:0:0".into());
        stream.update(vec![old], Vec::new(), false);
        let update = stream.update(Vec::new(), Vec::new(), false);
        assert!(update.removed_ids.is_empty());
    }
    #[test]
    fn newly_streamed_text_does_not_repeat_the_previous_answer() {
        let mut stream = ConversationStream::default();
        let mut answer = TranscriptItem::new("assistant", "Previous answer", None);
        answer.id = Some("file:0:0".into());
        stream.update(vec![answer.clone()], Vec::new(), false);
        let update = stream.update(
            vec![answer],
            pane_items("codex", "• Previous answer\n• New answer"),
            true,
        );
        let partial = update
            .items
            .iter()
            .find(|i| i.partial == Some(true))
            .unwrap();
        assert_eq!(partial.text, "New answer");
    }
}
