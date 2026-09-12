//! Pane previews and stable reconciliation between live output and durable transcript rows.
use super::{conversation_activity, prompt};
use crate::model::{ToolCallStatus, TranscriptItem};
use std::collections::{HashMap, HashSet};

/// A single CLI slash command, excluding paths, prose, and multiline attachment prompts.
pub fn is_slash_command(text: &str) -> bool {
    let text = text.trim();
    if text.contains(['\n', '\r']) {
        return false;
    }
    let Some(command) = text
        .strip_prefix('/')
        .and_then(|s| s.split_whitespace().next())
    else {
        return false;
    };
    command.starts_with(|c: char| c.is_ascii_alphabetic())
        && command
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// Codex's footer is a set of dot-separated fields, including a model/effort and a working
/// directory. Neither model names nor project paths are fixed. Its composer animation consists
/// solely of dots/braille cells; those lines carry no prose.
fn codex_chrome(line: &str) -> bool {
    let animation = line
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<Vec<_>>();
    if !animation.is_empty()
        && animation
            .iter()
            .all(|c| matches!(c, '.' | '·' | '•' | '\u{2800}'..='\u{28ff}'))
        && (animation.len() >= 4
            || animation
                .iter()
                .any(|c| matches!(c, '\u{2800}'..='\u{28ff}')))
    {
        return true;
    }
    conversation_activity::model_line(line).is_some()
}

/// Antigravity draws a start-up banner (a half-block logo, the account, the model, the cwd) and
/// a collapsed thinking disclosure above each answer. Its turn separators and prompt echoes are
/// handled structurally alongside every other CLI's; these are the decorations left over, and
/// none of them has a counterpart in the durable brain transcript.
fn antigravity_chrome(line: &str) -> bool {
    // The running-turn frame: a braille spinner cell, a verb, and the cancel hint with the
    // model name right-aligned onto the same row. It carries none of the timer-and-counter
    // grammar `conversation_activity::timed_line` recognises, so nothing caught it and it was
    // seated as assistant prose.
    if line.starts_with(|c: char| ('\u{2800}'..='\u{28ff}').contains(&c)) {
        return true;
    }
    if line.to_lowercase().contains("esc to cancel") {
        return true;
    }
    line.starts_with('▸')
        // A banner row is the half-block logo on the left with the account, model or working
        // directory printed to its right, so match the row by the glyph that opens it.
        || line.starts_with(['▄', '▀', '█', '▌', '▐'])
        || line.starts_with("Antigravity CLI ")
}

/// OpenCode renders its conversation and a status sidebar as columns of one grid, so a sidebar
/// row shares every line with the conversation. The columns are separated by a wide run of
/// spaces that ordinary prose does not contain, which is what splits them. A fenced code block
/// with deep indentation could in principle be cut short here; the sidebar is the common case
/// and a truncated preview is recoverable, where a preview full of sidebar text is not.
const OPENCODE_COLUMN_GAP: usize = 6;

fn opencode_conversation_column(line: &str) -> &str {
    let mut run = 0usize;
    for (at, c) in line.char_indices() {
        if c == ' ' {
            run += 1;
            continue;
        }
        if run >= OPENCODE_COLUMN_GAP && at > run && line[..at - run].trim().len() > 2 {
            return &line[..at - run];
        }
        run = 0;
    }
    line
}

/// The composer and status bar OpenCode paints under the conversation: a rule drawn from `╹`
/// and `▀`, the gutter rows of the empty input above it, and the status line below. The gutter
/// glyph is the same one that marks a user prompt, so the composer is identified by position
/// (the run of gutter rows that reaches the rule) rather than by its content.
fn opencode_body(plain: &str) -> String {
    let lines: Vec<&str> = plain.lines().map(opencode_conversation_column).collect();
    let rule = lines.iter().rposition(|l| {
        let t = l.trim();
        !t.is_empty() && t.chars().all(|c| matches!(c, '╹' | '▀' | ' '))
    });
    let mut end = rule.unwrap_or(lines.len());
    while end > 0 {
        let t = lines[end - 1].trim();
        if t.starts_with('┃') {
            end -= 1;
            continue;
        }
        break;
    }
    lines[..end].join("\n")
}

/// OpenCode's per-turn decorations: the thinking time above an answer and the model footer
/// below it. Neither has a counterpart in its message table.
fn opencode_chrome(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("+ Thought:") || t.starts_with('▣')
}

/// Decorations this CLI draws around its conversation, which are never content. One definition
/// for both the excerpt and the prose parser, so the two can never disagree about a line.
fn kind_chrome(kind: &str, line: &str) -> bool {
    conversation_activity::queue_indicator(kind, line)
        || conversation_activity::timed_line(kind, line).is_some()
        || (kind == "codex" && codex_chrome(line.trim()))
        || (kind == "antigravity" && antigravity_chrome(line.trim()))
        || (kind == "opencode" && opencode_chrome(line))
}

/// Preserve excerpt content while removing the same structured status decorations as prose.
pub fn pane_content(kind: &str, pane: &str) -> String {
    let plain = super::conversation_queue::without_queue(kind, pane);
    let plain = if kind == "opencode" {
        opencode_body(&plain)
    } else {
        plain
    };
    plain
        .lines()
        .filter(|line| !kind_chrome(kind, line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Strip only recognized CLI decorations. Unknown kinds preserve the raw plain pane tail.
pub fn pane_items(kind: &str, pane: &str) -> Vec<TranscriptItem> {
    let source = super::conversation_queue::without_queue(kind, pane);
    let source = if kind == "opencode" {
        opencode_body(&source)
    } else {
        source
    };
    let plain = super::repomail::split(&source, None)
        .into_iter()
        .filter(|row| row.mail.is_none())
        .map(|row| row.text)
        .collect::<Vec<_>>()
        .join("\n");
    // Antigravity's pane is as structured as Codex's: a banner, rules between turns, a "> "
    // prompt echo and a thinking disclosure. Parsing it is what stops a live preview from being
    // a raw dump of the banner and every earlier answer.
    // OpenCode is a grid, not a stream, but it is still structured: a gutter for prompts, a
    // thinking row, an answer, and a model footer per turn. Parsing it is what stops its
    // preview from being an opaque dump of the whole pane.
    let supported = matches!(kind, "claude-code" | "codex" | "antigravity" | "opencode");
    let mut prose = Vec::new();
    let mut tools: Vec<TranscriptItem> = Vec::new();
    let mut codex_tool_output = false;
    let mut in_box = false;
    for raw in plain.lines() {
        let line = raw.trim();
        if supported {
            if kind_chrome(kind, line) {
                continue;
            }
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
            if line.starts_with('❯')
                || line.starts_with('›')
                || (kind == "antigravity" && line.starts_with('>'))
                || (kind == "opencode" && line.starts_with('┃'))
            {
                let input = line.chars().skip(1).collect::<String>();
                let placeholder = kind == "codex"
                    && (input.trim().starts_with("Ask Codex ")
                        || input.trim().starts_with("Use /")
                        || input.trim().starts_with("Try \""));
                if !placeholder && !input.trim().is_empty() {
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
            if kind == "codex"
                && codex_tool_output
                && !line.is_empty()
                && (raw.starts_with("  ") || line.starts_with(['└', '│']))
            {
                if let Some(tool) = tools.last_mut() {
                    let output = tool.result_summary.get_or_insert_with(String::new);
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(raw.trim_end());
                    if super::codex_content::numbered_diff(output) {
                        tool.diff = Some(output.clone());
                    }
                }
                continue;
            }
            if !line.is_empty() {
                codex_tool_output = false;
            }
            let body = line.trim_start_matches(['⏺', '●', '•']).trim();
            if kind == "codex" {
                if let Some(item) = super::codex_content::tool_item(body) {
                    tools.push(item);
                    codex_tool_output = true;
                    continue;
                }
            }
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
                item.status = Some(if body.starts_with("Ran ") {
                    ToolCallStatus::Ok
                } else {
                    ToolCallStatus::Running
                });
                tools.push(item);
                codex_tool_output = true;
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
        let item = if kind == "codex" {
            super::codex_content::tool_item(&text)
        } else {
            None
        }
        .unwrap_or_else(|| {
            TranscriptItem::new(
                if supported {
                    "assistant"
                } else {
                    "terminal_block"
                },
                text,
                None,
            )
        });
        // `partial` is deliberately NOT set here. A pane parser cannot know whether the turn that
        // produced this text is still running, and a flag minted unconditionally here can only be
        // unset by a durable final row. `ConversationStream::update` owns the turn lifecycle and
        // stamps the flag from it.
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
/// A pane preview must never seat text the conversation already knows: a durable answer it is
/// still showing, or the echo of what the operator just submitted. Both are the same defect and
/// this is the single guard for them.
///
/// The capture window is the hard part. A long submission is echoed by the CLI and then clipped,
/// so the pane holds only its tail and begins mid sentence. Equality cannot see that, which is how
/// the operator's own message came to be seated as an assistant reply starting part way through a
/// word. Matching therefore accepts the pane beginning with any sufficiently long suffix of the
/// known text, as well as containing the whole of it.
const CLIPPED_ECHO_MIN: usize = 24;

fn known_text_end(pane: &str, known: &str) -> Option<usize> {
    prior_answer_end(pane, known).or_else(|| clipped_echo_end(pane, known))
}

/// Where a clipped echo ends: the pane opens with a suffix of `known`, because the capture cut the
/// start of it away.
fn clipped_echo_end(pane: &str, known: &str) -> Option<usize> {
    let wanted: String = known.chars().filter(|c| !c.is_whitespace()).collect();
    if wanted.chars().count() < CLIPPED_ECHO_MIN {
        return None;
    }
    let mut flat = String::new();
    let mut boundaries = Vec::new();
    for (at, c) in pane.char_indices() {
        if !c.is_whitespace() {
            flat.push(c);
            boundaries.push((flat.len(), at + c.len_utf8()));
        }
    }
    // Longest opening run of the pane that is a tail of the known text, stepping by characters so
    // a multi-byte glyph at the boundary cannot split.
    let cuts: Vec<usize> = flat
        .char_indices()
        .map(|(at, _)| at)
        .skip(1)
        .chain(std::iter::once(flat.len()))
        .collect();
    let best = cuts
        .iter()
        .rev()
        .copied()
        .filter(|end| flat[..*end].chars().count() >= CLIPPED_ECHO_MIN)
        .find(|end| wanted.ends_with(&flat[..*end]))?;
    boundaries
        .into_iter()
        .find(|(n, _)| *n == best)
        .map(|(_, original)| original)
}

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

/// How many of the newest durable rows a preview is checked against. The echo of a submission and
/// the answer it produced are both within a couple of rows of the end.
const KNOWN_REACH: usize = 4;

/// Upserts retain their ID as a pane preview becomes a durable row. Removed IDs clear ephemeral
/// dialogs and working indicators without asking clients to discard older history pages.
#[derive(Debug, Default)]
pub struct Update {
    pub items: Vec<TranscriptItem>,
    pub removed_ids: Vec<String>,
    /// Complete order of the current window, including unchanged upsert identities.
    pub order: Vec<String>,
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
    /// Whether `id` is this window's live assistant preview, used to seat consumed input rows
    /// just above the reply. Read from the pending previews themselves, not from the emitted
    /// `partial` flag, so the ordering anchor survives a turn ending.
    pub fn is_partial_assistant(&self, id: &str) -> bool {
        self.pending
            .iter()
            .any(|r| r.id.as_deref() == Some(id) && r.role == "assistant")
    }

    /// A fast CLI may persist a user row before the verified send acknowledges it. Remove the
    /// earlier source identity once when the shared input registry resolves its submission id.
    pub fn forget_replaced_user(&mut self, id: &str) -> bool {
        if self.previous.get(id).is_some_and(|r| r.role == "user") {
            self.previous.remove(id);
            self.durable.remove(id);
            true
        } else {
            false
        }
    }
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
        // A pane contains recent scrollback and the echo of whatever was just submitted. A
        // preview is only ever the text that follows everything already known, whichever kind of
        // row that text came from.
        let known: Vec<&TranscriptItem> = final_rows
            .iter()
            .rev()
            .filter(|i| {
                matches!(i.kind.as_deref(), Some("assistant") | Some("user"))
                    && !i.text.trim().is_empty()
            })
            .take(KNOWN_REACH)
            .collect();
        for item in &mut live {
            if !matches!(
                item.kind.as_deref(),
                Some("assistant") | Some("terminal_block")
            ) {
                continue;
            }
            if let Some(end) = known
                .iter()
                .filter_map(|row| known_text_end(&item.text, &row.text))
                .max()
            {
                item.text = item.text[end..].trim().to_string();
            }
        }
        live.retain(|i| {
            !matches!(
                i.kind.as_deref(),
                Some("assistant") | Some("terminal_block")
            ) || !i.text.is_empty()
        });
        if let Some(answer) = final_rows
            .iter()
            .rev()
            .find(|i| i.kind.as_deref() == Some("assistant") && !i.text.trim().is_empty())
        {
            // `terminal_block` is the preview for every kind whose pane we cannot parse into
            // messages, and it carries the whole visible pane. It needs this strip more than an
            // assistant preview does, not less: without it the excerpt repeats, verbatim, text
            // that is already seated as durable rows above it.
            for item in &mut live {
                if matches!(
                    item.kind.as_deref(),
                    Some("assistant") | Some("terminal_block")
                ) {
                    if let Some(end) = prior_answer_end(&item.text, &answer.text) {
                        item.text = item.text[end..].trim().to_string();
                    }
                }
            }
            live.retain(|i| {
                !matches!(
                    i.kind.as_deref(),
                    Some("assistant") | Some("terminal_block")
                ) || !i.text.is_empty()
            });
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
                    // Everything in `pending` is a pane preview by construction, so pairing a
                    // preview with its durable row never needs to consult the streaming flag.
                    (p.kind == row.kind
                        || (p.kind.as_deref() == Some("terminal_block")
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
        // A preview reads "Writing..." only while its turn is running. Stamping the flag here,
        // the one place that knows the turn lifecycle, is what keeps a partial from outliving its
        // turn: a usage limit, an interrupt, an auth failure or a crash all end a turn by going
        // quiet, and none of them writes the durable final row that reconciliation waits for.
        rows.extend(self.pending.iter().cloned().map(|mut item| {
            item.partial = active.then_some(true);
            item
        }));
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
            order: rows.iter().filter_map(|i| i.id.clone()).collect(),
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
    fn real_codex_fixture_strips_footer_and_preserves_prose_and_tools() {
        // Captured from this worker's own verified pane %355, lane-48357719, 2026-09-10.
        let pane = include_str!("fixtures/codex_working_footer_2026_09_10.ansi");
        for pane in [
            pane.to_string(),
            pane.replace("gpt-6-astra high", "o9-example medium")
                .replace(
                    "~/code/repomon-wt/feat-conversation-view-daemon",
                    "/different/project",
                )
                .replace("/different/project", "/different/project · Work [default]"),
        ] {
            let items = pane_items("codex", &pane);
            let prose = items
                .iter()
                .find(|i| i.kind.as_deref() == Some("assistant"))
                .unwrap();
            assert!(prose.text.starts_with("Bounded paging brought warm"));
            assert!(!prose.text.contains("esc to interrupt"));
            assert!(!prose.text.contains("Ask Codex"));
            assert!(
                !prose
                    .text
                    .chars()
                    .any(|c| matches!(c, '\u{2800}'..='\u{28ff}'))
            );
            assert!(!prose.text.contains("[default]"));
            assert!(items.iter().any(|i| i.kind.as_deref() == Some("tool_call")));
        }
        assert!(
            pane_items(
                "codex",
                ". . . . . . . . · model-z high · /project · Work [default]"
            )
            .is_empty()
        );
        assert!(pane_items("codex", ". . . . . . . .").is_empty());
        assert_eq!(
            pane_items("codex", "Use model-z high in /project for this task.")[0].text,
            "Use model-z high in /project for this task."
        );
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
    /// The operator's screenshot: while a turn was in flight the Antigravity window showed a raw
    /// excerpt carrying the CLI banner, the account, the model, the cwd, the thinking disclosure
    /// and the answer a second time, next to the same answer as a proper message row. The pane is
    /// the real one, captured read only from the live window; only the account address is
    /// substituted. Everything the durable brain transcript does not record must be gone, and the
    /// preview must contain the current turn only.
    #[test]
    fn real_antigravity_fixture_keeps_only_the_current_turn_and_drops_the_banner() {
        let pane = include_str!("fixtures/antigravity_status_v0.ansi");
        let items = pane_items("antigravity", pane);
        let prose: Vec<_> = items
            .iter()
            .filter(|i| {
                matches!(
                    i.kind.as_deref(),
                    Some("assistant") | Some("terminal_block")
                )
            })
            .collect();
        assert_eq!(prose.len(), 1, "{items:#?}");
        let preview = prose[0];
        // A pane we can parse is a message, not an opaque terminal dump.
        assert_eq!(preview.kind.as_deref(), Some("assistant"));
        assert_eq!(
            preview.text.trim(),
            "Hello again! What would you like to work on tonight?"
        );
        for banned in [
            "Antigravity CLI",
            "example.invalid",
            "Gemini 3.8 Flash",
            "Developer/Github",
            "Thought for",
            "? for shortcuts",
            // Earlier turns, including the answer that is already a seated message row.
            "A spark ignites",
            "Did you mean to type",
            "Hello! How can I help you today?",
        ] {
            assert!(
                !preview.text.contains(banned),
                "preview still carries {banned:?}: {:?}",
                preview.text
            );
        }
        assert!(!preview.text.contains('\u{1b}'));
        // pane_content backs the unbound-window excerpt; it strips the same decorations.
        let excerpt = pane_content("antigravity", pane);
        assert!(!excerpt.contains("Antigravity CLI"), "{excerpt}");
        assert!(!excerpt.contains("Thought for"), "{excerpt}");
    }

    /// Mid turn, before the model has produced any answer, there is nothing to preview but the
    /// current prompt's own progress. The banner and every earlier turn must already be gone,
    /// which is the state the operator screenshotted.
    #[test]
    fn antigravity_in_flight_turn_previews_only_what_follows_its_own_prompt() {
        let settled = include_str!("fixtures/antigravity_status_v0.ansi");
        // Same real pane, cut where it stood while the last turn was still running.
        let in_flight = settled
            .split_once("Hello again! What would you like to work on tonight?")
            .expect("fixture contains the final answer")
            .0;
        let items = pane_items("antigravity", in_flight);
        for item in &items {
            assert!(!item.text.contains("Antigravity CLI"), "{item:#?}");
            assert!(!item.text.contains("A spark ignites"), "{item:#?}");
            assert!(!item.text.contains("Thought for"), "{item:#?}");
        }
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
    /// A turn can end without ever writing a durable assistant row: a usage limit, an interrupt,
    /// an auth failure or an outright crash all just go quiet. Reconciliation against a final row
    /// is therefore not a lifetime for the streaming flag, and the preview must stop claiming it
    /// is still being written the moment the turn stops.
    #[test]
    fn a_turn_that_ends_with_no_durable_row_stops_claiming_it_is_still_writing() {
        let endings = [
            (
                "usage limit",
                "• The answer is forty two.\n• You've hit your usage limit. Try again at Sep 15th.",
            ),
            (
                "interrupt",
                "• The answer is forty two.\n• Interrupted by user",
            ),
            (
                "auth failure",
                "• The answer is forty two.\n• Authentication failed. Run /login to sign in again.",
            ),
            // A crash leaves the last frame on screen and the byte stream closed; the watch loop
            // reports `active = false` with the pane unchanged.
            ("crash", "• The answer is forty two."),
        ];
        for (ending, pane) in endings {
            for kind in ["claude-code", "codex"] {
                let mut stream = ConversationStream::default();
                stream.update(Vec::new(), Vec::new(), false);
                let streaming =
                    stream.update(Vec::new(), pane_items(kind, "• The answer is"), true);
                let id = streaming
                    .items
                    .iter()
                    .find(|i| i.partial == Some(true))
                    .unwrap_or_else(|| panic!("no preview while streaming: {kind} {ending}"))
                    .id
                    .clone();
                // The turn ends. No durable row is written, and none ever will be.
                let ended = stream.update(Vec::new(), pane_items(kind, pane), false);
                assert!(
                    !ended.items.iter().any(|i| i.partial == Some(true)),
                    "{kind} still claims to be writing after a {ending}"
                );
                // The preview keeps its identity and its text: the answer stays seated, only the
                // streaming claim is withdrawn.
                let seated = ended
                    .items
                    .iter()
                    .find(|i| i.id == id)
                    .unwrap_or_else(|| panic!("preview vanished on {ending} for {kind}"));
                assert_eq!(seated.partial, None, "{kind} {ending}");
                assert!(!seated.text.trim().is_empty(), "{kind} {ending}");
                assert!(
                    !ended.removed_ids.contains(id.as_ref().unwrap()),
                    "{kind} {ending}"
                );
                // Still quiet on the next tick, and still not resurrected as streaming.
                let quiet = stream.update(Vec::new(), pane_items(kind, pane), false);
                assert!(
                    !quiet.items.iter().any(|i| i.partial == Some(true)),
                    "{kind} resurrected the writing marker after a {ending}"
                );
                // A new turn starts: the preview is allowed to stream again.
                let resumed = stream.update(
                    Vec::new(),
                    pane_items(kind, "• A second answer begins"),
                    true,
                );
                assert!(
                    resumed.items.iter().any(|i| i.partial == Some(true)),
                    "{kind} cannot stream again after a {ending}"
                );
            }
        }
    }

    /// An opaque `terminal_block` preview is the whole visible pane, so it repeats answers that
    /// are already seated as durable rows above it. Antigravity showed this directly: the raw
    /// block under the message rows carried the same text again.
    #[test]
    fn an_opaque_preview_does_not_repeat_an_answer_already_seated_above_it() {
        let mut stream = ConversationStream::default();
        let answer = "The tide rolls in with steady grace, erasing footsteps from the place.";
        let mut durable = TranscriptItem::new("assistant", answer, None);
        durable.id = Some("/db:1".into());
        stream.update(vec![durable.clone()], Vec::new(), false);
        // The pane still shows that answer, followed by the next turn's opening line.
        let pane = format!("{answer}\nA second line the transcript does not have yet.");
        let mut live = TranscriptItem::new("terminal_block", pane, None);
        live.id = None;
        let update = stream.update(vec![durable], vec![live], true);
        let preview = update
            .items
            .iter()
            .find(|i| i.kind.as_deref() == Some("terminal_block"))
            .expect("the new text still previews");
        assert!(
            !preview.text.contains("steady grace"),
            "preview repeats a seated answer: {:?}",
            preview.text
        );
        assert!(preview.text.contains("A second line"));
    }

    /// The running-turn frame was seated as an assistant message: the spinner row itself, read
    /// as prose, with the streaming marker under it. Its shape is a braille cell plus a verb
    /// plus the cancel hint and model name, with none of the timer-and-counter grammar
    /// `conversation_activity::timed_line` recognises, so nothing was catching it.
    #[test]
    fn antigravity_running_turn_chrome_is_never_seated_as_prose() {
        let pane = include_str!("fixtures/antigravity_suggestion_chips.txt");
        let items = pane_items("antigravity", pane);
        for item in &items {
            for banned in ["Loading", "esc to cancel", "Gemini 3.8 Flash"] {
                assert!(
                    !item.text.contains(banned),
                    "{banned:?} seated as content: {:?}",
                    item.text
                );
            }
        }
        // The answer that was actually in flight is still previewed.
        assert!(
            items.iter().any(|i| i.text.contains("The tide rolls in")),
            "the in-flight answer must still preview: {items:#?}"
        );
        // The same rows are chrome for the excerpt that backs an unbound window.
        let excerpt = pane_content("antigravity", pane);
        assert!(!excerpt.contains("esc to cancel"), "{excerpt}");
        assert!(!excerpt.contains("Loading"), "{excerpt}");
    }

    /// OpenCode previews were an opaque dump of the whole pane: banner, sidebar, every past
    /// turn. Both fixtures are the operator's real window, one with the status sidebar open and
    /// one without, so the column split is exercised either way.
    #[test]
    fn real_opencode_panes_preview_only_the_current_turn() {
        for (name, pane, answer) in [
            (
                "sidebar",
                include_str!("fixtures/opencode_sidebar_v0.txt"),
                "Code compiles, tests pass green,",
            ),
            (
                "no sidebar",
                include_str!("fixtures/opencode_turns_v0.txt"),
                "Keyboard clacks, the logic bends,",
            ),
        ] {
            let items = pane_items("opencode", pane);
            let prose: Vec<_> = items
                .iter()
                .filter(|i| {
                    matches!(
                        i.kind.as_deref(),
                        Some("assistant") | Some("terminal_block")
                    )
                })
                .collect();
            assert_eq!(prose.len(), 1, "{name}: {items:#?}");
            let preview = prose[0];
            // A pane we can parse is a message, not an opaque terminal dump.
            assert_eq!(preview.kind.as_deref(), Some("assistant"), "{name}");
            assert!(preview.text.contains(answer), "{name}: {:?}", preview.text);
            for banned in [
                "+ Thought:",            // the per turn thinking row
                "Build \u{b7} Nemotron", // the per turn model footer
                "Context",               // sidebar
                "tokens",                // sidebar
                "OpenCode",              // status bar
                "ctrl+p",                // status bar
            ] {
                assert!(
                    !preview.text.contains(banned),
                    "{name}: preview still carries {banned:?}: {:?}",
                    preview.text
                );
            }
        }
    }

    /// The OpenCode terminal-excerpt defect, traced on lane-81-4. Every `chat_open` resolved
    /// `source=durable`, so binding was not involved. The failing order is the durable row
    /// landing BEFORE the pane preview is captured: the pairing at the top of `update` runs
    /// against an empty `pending` and consumes nothing, and the preview is seated afterwards
    /// with no durable row left to claim it.
    ///
    /// Parsing the pane is what dissolves it. The preview is now an `assistant` item carrying
    /// only the current turn, so the durable-answer strip empties it the moment that answer is
    /// already known, and an orphan can never be seated, in either arrival order.
    #[test]
    fn an_opencode_answer_already_durable_is_never_seated_as_an_excerpt() {
        let pane = include_str!("fixtures/opencode_turns_v0.txt");
        let answer = "Keyboard clacks, the logic bends,";
        let mut durable = TranscriptItem::new(
            "assistant",
            "Keyboard clacks, the logic bends,\nA feature born, a bug depends.\n\
             Push to prod, the metrics rise\u{2014}\nCode lives on, it never dies.",
            None,
        );
        durable.id = Some("/db:answer".into());
        let mut stream = ConversationStream::default();
        stream.update(Vec::new(), Vec::new(), false);
        // The durable row lands first, so `pending` is empty and nothing pairs with it.
        stream.update(vec![durable.clone()], Vec::new(), true);
        // Only now is the pane captured, still showing that same answer.
        let update = stream.update(vec![durable.clone()], pane_items("opencode", pane), true);
        assert!(
            !update
                .items
                .iter()
                .any(|i| i.kind.as_deref() == Some("terminal_block")),
            "an opaque excerpt was seated: {:#?}",
            update.items
        );
        let quiet = stream.update(vec![durable], pane_items("opencode", pane), false);
        assert!(
            !quiet.items.iter().any(|i| i.text.contains(answer)),
            "the already durable answer was previewed again: {:#?}",
            quiet.items
        );
    }

    /// The operator's own message, seated as an assistant reply attributed to Claude and
    /// beginning mid sentence. The CLI echoed his prompt, the capture held only its tail, so the
    /// echo matched no durable row by equality and was taken for fresh prose.
    ///
    /// This is the same defect as Antigravity's duplicated answer and OpenCode's orphaned
    /// excerpt: a preview seating text the conversation already knows. One guard covers all
    /// three, and the clipping is why it cannot be equality.
    #[test]
    fn a_clipped_echo_of_the_operators_own_prompt_is_never_seated_as_an_answer() {
        let pane = include_str!("fixtures/claude_clipped_prompt_echo.txt");
        let submitted = "codex does not work and says not supported in repomon yet so complete \
                         that implementation. for agy you can switch model but the active model \
                         that shows in the chat box doesn't update.";
        let mut durable = TranscriptItem::new("user", submitted, None);
        durable.id = Some("/db:user".into());
        let mut stream = ConversationStream::default();
        stream.update(vec![durable.clone()], Vec::new(), false);
        let update = stream.update(vec![durable], pane_items("claude-code", pane), true);
        for item in &update.items {
            assert!(
                !item.text.contains("so complete that implementation"),
                "the echo was seated as an answer: {:?}",
                item.text
            );
            assert!(
                !item.text.contains("chat box doesn't update"),
                "the clipped tail was seated: {:?}",
                item.text
            );
        }
    }

    /// The clipped matcher must not fire on a short or unrelated opening, or every preview would
    /// be eaten by coincidence.
    #[test]
    fn a_clipped_match_needs_a_real_run_of_the_known_text() {
        let known = "a submission long enough to be recognised from its tail alone";
        // The pane opens with the tail of it: matched, and the preview is what follows.
        let pane = "enough to be recognised from its tail alone\nAnd then the real answer.";
        let end = known_text_end(pane, known).expect("tail should match");
        assert_eq!(pane[end..].trim(), "And then the real answer.");
        // A short opening is coincidence, not an echo.
        assert_eq!(
            known_text_end("alone\nAnd then the real answer.", known),
            None
        );
        // Unrelated text never matches.
        assert_eq!(
            known_text_end("A completely different answer entirely.", known),
            None
        );
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

#[cfg(test)]
mod ordering_tests {
    use super::*;
    #[test]
    fn delayed_user_ingestion_reproduces_arrival_order_bug_without_any_queue() {
        let mut stream = ConversationStream::default();
        stream.update(Vec::new(), Vec::new(), false);
        let partial = stream.update(
            Vec::new(),
            pane_items("claude-code", "⏺ The answer is forty two."),
            true,
        );
        let answer_id = partial
            .items
            .iter()
            .find(|i| i.partial == Some(true))
            .unwrap()
            .id
            .clone()
            .unwrap();
        let mut user = TranscriptItem::new("user", "What is the answer?", None);
        user.id = Some("source:0:user".into());
        let mut answer = TranscriptItem::new("assistant", "The answer is forty two.", None);
        answer.id = Some("source:100:assistant".into());
        let final_update = stream.update(vec![user, answer], Vec::new(), false);
        assert_eq!(final_update.items[0].role, "user");
        assert_eq!(
            final_update.items[1].id.as_deref(),
            Some(answer_id.as_str())
        );
        // Existing UI keeps the old id in place and appends the newly observed user id.
        let mut arrival = vec![answer_id.clone()];
        for row in &final_update.items {
            let id = row.id.clone().unwrap();
            if !arrival.contains(&id) {
                arrival.push(id);
            }
        }
        assert_eq!(arrival[0], answer_id);
        assert_eq!(arrival[1], "source:0:user");
        // New explicit order repairs this even when the user has no timestamp at all.
        assert_eq!(
            &final_update.order[..2],
            &["source:0:user".to_string(), answer_id]
        );
    }
}

#[cfg(test)]
mod codex_output_tests {
    use super::*;
    #[test]
    fn codex_rollup_and_indented_tool_result_are_not_assistant_prose() {
        let pane = "• Searched for 2 patterns, ran 7 shell commands\n  └ Test diff\n    205 + await screen.findByText(\"Ready\");\n    206 + expect(button).toBeEnabled();\n\n• The fix is ready for review.";
        let rows = pane_items("codex", pane);
        let prose = rows
            .iter()
            .find(|r| r.kind.as_deref() == Some("assistant"))
            .unwrap();
        assert_eq!(prose.text, "The fix is ready for review.");
        let tool = rows
            .iter()
            .find(|r| r.kind.as_deref() == Some("tool_call"))
            .unwrap();
        assert_eq!(tool.name.as_deref(), Some("tool_summary"));
        assert!(
            tool.result_summary
                .as_deref()
                .unwrap()
                .contains("screen.findByText")
        );
        assert!(tool.diff.as_deref().unwrap().contains("206 +"));
    }
}
