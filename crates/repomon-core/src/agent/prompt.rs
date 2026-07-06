//! Detecting a pending interactive prompt (permission dialog, plan approval, trust dialog,
//! a question with options) in a managed agent's pane.
//!
//! A transcript that ends in a tool call reads as **Running**, but the pane may actually be
//! sitting on "Do you want to proceed? ❯ 1. Yes …" — blocked on the user, with nothing in the
//! JSONL to say so. This module is the pure, fixture-tested detector: given recent pane text it
//! decides whether the agent is waiting on an interactive prompt, and produces a compact
//! summary (dialog header + question) to use as the notification's "why". The daemon flips
//! such sessions to `Waiting` during `lane.list`.

use serde::{Deserialize, Serialize};

use super::limit::parse_option_line;
use super::text::strip_ansi;

/// How far above the option menu the question line may sit.
const QUESTION_REACH: usize = 5;
/// How far above the question to look for the dialog's top border (the `╭` line).
const HEADER_REACH: usize = 20;
/// How far below the option menu the confirmation footer ("Enter to confirm · Esc to cancel")
/// may sit — used only as corroborating evidence for the folder-trust dialog, which (unlike
/// every other dialog this module recognizes) can appear with no question line in view at all.
const FOOTER_REACH: usize = 3;

/// Detect a pending interactive prompt in an agent's recent pane text and summarize it
/// (`"Bash command — Do you want to proceed?"`). Returns `None` for ordinary output, for
/// numbered lists without a selection cursor, and for the usage-limit menu (which is a
/// rate-limit pause, not a permission ask — see [`super::limit`]).
pub fn detect_pending_prompt(pane: &str) -> Option<String> {
    detect_dialog(pane).map(|d| d.summary())
}

/// How many content lines of the dialog's body (the command being approved, the edit summary)
/// [`detect_dialog`] keeps — enough for a peek popup, small enough to ride in `lane.list`.
const BODY_MAX_LINES: usize = 8;

/// A fully parsed interactive dialog: what the agent is asking and the choices it offers.
/// [`detect_dialog`] extracts it from pane text; [`detect_pending_prompt`] remains the
/// compact one-line view for callers that only need the "why".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingDialog {
    /// The dialog's box header naming the tool ("Bash command", "Edit file"); `None` for
    /// boxless dialogs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The question line ("Do you want to proceed?"). For the folder-trust dialog whose
    /// question can scroll out of the capture window, the synthetic "Do you trust this
    /// folder?" stands in.
    pub question: String,
    /// Content lines between the header and the question, capped at [`BODY_MAX_LINES`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body: Vec<String>,
    /// The selectable options, in screen order.
    pub options: Vec<DialogOption>,
    /// Index into `options` of the row the selection cursor sits on, if visible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<usize>,
}

/// One selectable dialog row: its printed number (if any) and its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialogOption {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    pub text: String,
}

impl PendingDialog {
    /// The compact one-line summary — exactly what [`detect_pending_prompt`] returns.
    pub fn summary(&self) -> String {
        let s = match &self.title {
            Some(t) => format!("{t} — {}", self.question),
            None => self.question.clone(),
        };
        truncate(&s, 120)
    }
    /// Classify this dialog as a routine permission ask or a real decision.
    pub fn class(&self) -> PromptClass {
        classify_prompt(&self.summary())
    }
}

/// Detect a pending interactive dialog and return it fully parsed: header, question, body,
/// options, and cursor position. Same detection rules as [`detect_pending_prompt`].
pub fn detect_dialog(pane: &str) -> Option<PendingDialog> {
    // Claude draws dialogs inside a box; strip ANSI and the `│` borders so option/question
    // lines parse the same whether boxed or bare.
    let stripped: Vec<String> = pane.lines().map(strip_ansi).collect();
    let cleaned: Vec<String> = stripped.iter().map(|l| content(l).to_string()).collect();
    let options: Vec<Option<(bool, Option<u32>, String)>> =
        cleaned.iter().map(|l| parse_option_line(l)).collect();

    // The active dialog is the last thing on screen — find the bottom-most option block that
    // really looks like a selection menu: ≥2 numbered rows plus a visible `❯` cursor.
    let mut end = options.len();
    while end > 0 {
        let block_end = options[..end].iter().rposition(|p| p.is_some())?;
        let mut start = block_end;
        while start > 0 && options[start - 1].is_some() {
            start -= 1;
        }
        let block: Vec<&(bool, Option<u32>, String)> =
            options[start..=block_end].iter().flatten().collect();
        let numbered = block.iter().filter(|(_, n, _)| n.is_some()).count();
        let has_cursor = block.iter().any(|(c, _, _)| *c);
        if numbered >= 2 && has_cursor {
            // The usage-limit menu is handled by the auto-continue watcher, not as a prompt.
            if block
                .iter()
                .any(|(_, _, t)| is_limit_option(&t.to_lowercase()))
            {
                return None;
            }
            let opts: Vec<DialogOption> = block
                .iter()
                .map(|(_, n, t)| DialogOption {
                    number: *n,
                    text: t.clone(),
                })
                .collect();
            let selected = block.iter().position(|(c, _, _)| *c);
            if let Some((title, question, body)) = describe(&stripped, &cleaned, start) {
                return Some(PendingDialog {
                    title,
                    question,
                    body,
                    options: opts,
                    selected,
                });
            }
            // Claude's folder-trust dialog ("Security guide" / "Yes, I trust this folder").
            // On a freshly spawned worker the question line ("Do you trust the files in this
            // folder?") can be scrolled out of the capture window entirely, so `describe`
            // above finds no question and comes back empty. Recognize the dialog by its
            // distinctive first option plus its confirmation footer instead — see the live
            // fixture in the tests below.
            if is_trust_dialog(&block) && has_confirm_footer(&cleaned, block_end) {
                return Some(PendingDialog {
                    title: None,
                    question: "Do you trust this folder?".to_string(),
                    body: Vec::new(),
                    options: opts,
                    selected,
                });
            }
            return None;
        }
        end = start; // not a menu — keep scanning the lines above
    }
    None
}

/// The keystrokes (tmux `send-keys` names) that select `target` (0-based option index):
/// arrow from the visible cursor to the option's row, then Enter. Without a visible cursor,
/// fall back to the option's printed number — digit selection confirms immediately; the
/// trailing Enter then lands harmlessly on the empty input box. (Generalizes
/// [`super::limit::menu_select_keys`], which steers only the usage-limit menu's wait option.)
pub fn dialog_select_keys(dialog: &PendingDialog, target: usize) -> Vec<String> {
    match dialog.selected {
        Some(cur) => {
            let (from, to) = (cur as i64, target as i64);
            let arrow = if to > from { "Down" } else { "Up" };
            let mut keys = vec![arrow.to_string(); (to - from).unsigned_abs() as usize];
            keys.push("Enter".into());
            keys
        }
        None => match dialog.options.get(target).and_then(|o| o.number) {
            Some(n) => vec![n.to_string(), "Enter".into()],
            None => vec!["Enter".into()],
        },
    }
}

/// Describe the dialog whose menu starts at line `menu_start`: the question line just above
/// it, the header (the first content line under the box's `╭` border), and the body lines
/// between header and question (capped at [`BODY_MAX_LINES`]).
fn describe(
    stripped: &[String],
    cleaned: &[String],
    menu_start: usize,
) -> Option<(Option<String>, String, Vec<String>)> {
    let q_idx = (menu_start.saturating_sub(QUESTION_REACH)..menu_start)
        .rev()
        .find(|&i| is_question(&cleaned[i]))?;
    let question = cleaned[q_idx].trim().to_string();

    // Walk up to the dialog's top border; the first content line below it names the tool
    // ("Bash command", "Edit file", …). Boxless dialogs simply get no header.
    let header_idx = (q_idx.saturating_sub(HEADER_REACH)..q_idx)
        .rev()
        .find(|&i| stripped[i].trim_start().starts_with('╭'))
        .and_then(|b| (b + 1..q_idx).find(|&i| !cleaned[i].trim().is_empty()))
        .filter(|&i| cleaned[i].trim() != question);
    let title = header_idx.map(|i| cleaned[i].trim().to_string());

    let body = match header_idx {
        Some(h) => (h + 1..q_idx)
            .map(|i| cleaned[i].trim())
            .filter(|l| !l.is_empty())
            .take(BODY_MAX_LINES)
            .map(|l| truncate(l, 120))
            .collect(),
        None => Vec::new(),
    };
    Some((title, question, body))
}

/// A line that reads as the dialog's question: the explicit ask phrasings, or any line ending
/// in `?` (covers arbitrary question dialogs). Requiring an adjacent `❯` menu keeps quoted
/// questions in ordinary output from matching.
fn is_question(cleaned: &str) -> bool {
    let t = cleaned.trim();
    if t.is_empty() {
        return false;
    }
    let lower = t.to_lowercase();
    lower.contains("do you want") || lower.contains("would you like") || t.ends_with('?')
}

/// Whether an option row belongs to the usage-limit menu.
fn is_limit_option(lower_text: &str) -> bool {
    lower_text.contains("stop and wait") || lower_text.contains("wait for limit")
}

/// Whether an options block is Claude's folder-trust dialog, recognized by its first option's
/// exact wording alone — the dialog carries no question line to anchor on when the pane tail is
/// captured mid-scroll (see [`detect_pending_prompt`]).
fn is_trust_dialog(block: &[&(bool, Option<u32>, String)]) -> bool {
    block
        .first()
        .is_some_and(|(_, _, text)| text.trim().eq_ignore_ascii_case("Yes, I trust this folder"))
}

/// Whether the trust dialog's confirmation footer appears within [`FOOTER_REACH`] lines below
/// the option menu. Required alongside the option wording in [`is_trust_dialog`] so an unrelated
/// "Yes, I trust this folder" string in ordinary output — with no question line nearby either —
/// can't false-positive as a pending prompt.
fn has_confirm_footer(cleaned: &[String], block_end: usize) -> bool {
    let end = (block_end + 1 + FOOTER_REACH).min(cleaned.len());
    cleaned[block_end + 1..end]
        .iter()
        .any(|l| l.to_lowercase().contains("enter to confirm"))
}

/// How a pending prompt should be handled by an orchestrator: a routine **permission** ask the
/// agent raised about its own next tool call (proceed / make this edit / trust the folder), or a
/// genuine **decision** the agent is deferring to a human ("Which auth method should we use?").
///
/// An orchestrator may auto-answer a [`PromptClass::Permission`] in an autonomous posture, but
/// must escalate a [`PromptClass::Decision`] to the human and never answer it itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptClass {
    /// The agent is asking to go ahead with an action it already proposed (yes/no/allow).
    Permission,
    /// The agent is asking the human to make a real choice between substantive options.
    Decision,
}

/// Classify a [`detect_pending_prompt`] summary as a routine permission ask or a real decision.
///
/// Conservative by construction: only the well-known permission phrasings Claude uses for its
/// own tool calls map to [`PromptClass::Permission`]; everything else (any other question) is a
/// [`PromptClass::Decision`] so an uncertain prompt is escalated to the human rather than
/// auto-answered.
pub fn classify_prompt(summary: &str) -> PromptClass {
    let l = summary.to_lowercase();
    // The phrasings Claude uses when asking to run its own proposed tool call. These are the
    // only cases an autonomous orchestrator may answer without a human.
    const PERMISSION_MARKERS: &[&str] = &[
        "do you want to proceed",
        "do you want to make this edit",
        "do you want to make these edits",
        "do you want to create",
        "do you want to run",
        "do you want to apply",
        "do you want to allow",
        // Folder-trust dialogs: lane windows only ever run inside worktrees of repos the human
        // has already explicitly registered with repomon, so trusting the folder is routine
        // housekeeping, not a decision-class ask — safe for an orchestrator to auto-answer.
        "do you trust",
        // Codex's MCP tool-call approval ("Allow the repomon MCP server to run tool
        // \"fleet_status\"?" — live fixture in the tests below): the same routine
        // own-next-tool-call ask as Claude's "do you want to run".
        "mcp server to run tool",
    ];
    if PERMISSION_MARKERS.iter().any(|m| l.contains(m)) {
        PromptClass::Permission
    } else {
        PromptClass::Decision
    }
}

/// Strip the dialog box borders (`│ … │`) and padding from an ANSI-stripped line.
fn content(stripped: &str) -> &str {
    stripped
        .trim()
        .trim_start_matches(['│', '┃'])
        .trim_end_matches(['│', '┃'])
        .trim()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_boxed_permission_dialog_with_header() {
        let pane = "● Running cargo test…\n\
            ╭──────────────────────────────────────────────╮\n\
            │ Bash command                                 │\n\
            │                                              │\n\
            │   cargo install --path crates/repomon-tui    │\n\
            │   Install the repomon TUI                    │\n\
            │                                              │\n\
            │ Do you want to proceed?                      │\n\
            │ ❯ 1. Yes                                     │\n\
            │   2. Yes, and don't ask again for cargo      │\n\
            │   3. No, and tell Claude what to do          │\n\
            ╰──────────────────────────────────────────────╯";
        assert_eq!(
            detect_pending_prompt(pane).as_deref(),
            Some("Bash command — Do you want to proceed?")
        );
    }

    #[test]
    fn detects_bare_trust_dialog_without_header() {
        let pane = "Do you trust the files in this folder?\n\
            ❯ 1. Yes, proceed\n\
              2. No, exit";
        assert_eq!(
            detect_pending_prompt(pane).as_deref(),
            Some("Do you trust the files in this folder?")
        );
    }

    #[test]
    fn detects_folder_trust_dialog_without_question_line() {
        // Ground-truth pane capture from a live worker stuck on Claude's folder-trust dialog
        // (see repomind fix-1 brief). The question line ("Do you trust the files in this
        // folder?") had scrolled out of the capture window, leaving only this tail — which
        // used to be invisible to the detector entirely.
        let pane = " Security guide\n\n ❯ 1. Yes, I trust this folder\n   2. No, exit\n\n Enter to confirm · Esc to cancel";
        assert_eq!(
            parse_option_line(" ❯ 1. Yes, I trust this folder"),
            Some((true, Some(1), "Yes, I trust this folder".to_string()))
        );
        assert_eq!(
            parse_option_line("   2. No, exit"),
            Some((false, Some(2), "No, exit".to_string()))
        );
        let summary = detect_pending_prompt(pane);
        assert_eq!(summary.as_deref(), Some("Do you trust this folder?"));
        assert_eq!(classify_prompt(&summary.unwrap()), PromptClass::Permission);
    }

    #[test]
    fn detects_folder_trust_dialog_with_question_line_visible() {
        // The unscrolled dialog: the question line sits above the "Security guide" section.
        // Same classification, but the real question is used instead of the synthetic label.
        let pane = "Do you trust the files in this folder?\n\nSecurity guide\n\n❯ 1. Yes, I trust this folder\n  2. No, exit\n\nEnter to confirm · Esc to cancel";
        assert_eq!(
            detect_pending_prompt(pane).as_deref(),
            Some("Do you trust the files in this folder?")
        );
    }

    #[test]
    fn trust_wording_without_confirm_footer_is_not_a_prompt() {
        // Same first-option wording, but no confirmation footer nearby and no question line —
        // not enough evidence, so this must not match (guards against loosening detection).
        let pane = "Security guide\n\n❯ 1. Yes, I trust this folder\n  2. No, exit";
        assert_eq!(detect_pending_prompt(pane), None);
    }

    #[test]
    fn detects_codex_mcp_tool_approval_dialog() {
        // Ground-truth pane capture from a live codex orchestrator in supervised mode
        // (`-a on-request`) hitting its MCP tool-call approval. Codex draws its selection
        // cursor as `›` (U+203A), not Claude's `❯` (U+276F) — this dialog was invisible to the
        // detector until `parse_option_line` learned the glyph.
        let pane = "  Field 1/1\n\
              Allow the repomon MCP server to run tool \"fleet_status\"?\n\
              › 1. Allow                   Run the tool and continue.\n\
                2. Allow for this session  Run the tool and remember this choice for this session.\n\
                3. Always allow            Run the tool and remember this choice for future tool calls.\n\
                4. Cancel                  Cancel this tool call\n\
              enter to submit | esc to cancel";
        let summary = detect_pending_prompt(pane);
        assert_eq!(
            summary.as_deref(),
            Some("Allow the repomon MCP server to run tool \"fleet_status\"?")
        );
        assert_eq!(classify_prompt(&summary.unwrap()), PromptClass::Permission);
    }

    #[test]
    fn detects_question_dialog_by_trailing_question_mark() {
        let pane = "╭───────────────────────────────╮\n\
            │ Which auth method should we use?  │\n\
            │ ❯ 1. OAuth                        │\n\
            │   2. API keys                     │\n\
            │   3. Sessions                     │\n\
            ╰───────────────────────────────╯";
        assert_eq!(
            detect_pending_prompt(pane).as_deref(),
            Some("Which auth method should we use?")
        );
    }

    #[test]
    fn handles_ansi_escapes() {
        let pane = "Do you want to make this edit to app.rs?\n\
            \u{1b}[7m❯ 1. Yes\u{1b}[0m\n\
            \u{1b}[2m  2. Yes, allow all edits during this session\u{1b}[0m\n\
            \u{1b}[2m  3. No\u{1b}[0m";
        assert_eq!(
            detect_pending_prompt(pane).as_deref(),
            Some("Do you want to make this edit to app.rs?")
        );
    }

    #[test]
    fn usage_limit_menu_is_not_a_prompt() {
        // The limit menu is the auto-continue watcher's job; double-alerting would be noise.
        let pane = "What do you want to do?\n\
            ❯ 1. Stop and wait for limit to reset\n\
              2. Upgrade your plan";
        assert_eq!(detect_pending_prompt(pane), None);
    }

    #[test]
    fn numbered_list_without_cursor_is_not_a_prompt() {
        let pane = "Which option do you prefer?\n\
            1. Refactor the parser\n\
            2. Add tests first";
        assert_eq!(detect_pending_prompt(pane), None);
    }

    #[test]
    fn menu_without_question_is_not_a_prompt() {
        let pane = "❯ 1. alpha\n  2. beta\n  3. gamma";
        assert_eq!(detect_pending_prompt(pane), None);
    }

    #[test]
    fn ordinary_output_and_input_box_do_not_match() {
        let pane = "test result: ok. 121 passed; 0 failed\n\
            ╭─────────────────────────────╮\n\
            │ >                           │\n\
            ╰─────────────────────────────╯\n\
            ? for shortcuts";
        assert_eq!(detect_pending_prompt(pane), None);
    }

    #[test]
    fn picks_the_bottom_most_dialog() {
        // Scrollback may contain an old (answered) dialog; only the last one on screen counts.
        let pane = "Do you want to proceed?\n\
            ❯ 1. Yes\n\
              2. No\n\
            ● ran the command\n\
            Do you want to apply the patch?\n\
            ❯ 1. Yes\n\
              2. No";
        assert_eq!(
            detect_pending_prompt(pane).as_deref(),
            Some("Do you want to apply the patch?")
        );
    }

    #[test]
    fn long_summaries_truncate() {
        let q = format!("Do you want to {}?", "x".repeat(200));
        let pane = format!("{q}\n❯ 1. Yes\n  2. No");
        let s = detect_pending_prompt(&pane).unwrap();
        assert_eq!(s.chars().count(), 120);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn classify_permission_dialogs() {
        for s in [
            "Bash command — Do you want to proceed?",
            "Do you want to make this edit to app.rs?",
            "Do you trust the files in this folder?",
            "Do you trust this folder?",
            "Do you want to create README.md?",
        ] {
            assert_eq!(classify_prompt(s), PromptClass::Permission, "{s}");
        }
    }

    #[test]
    fn classify_real_questions_as_decisions() {
        // Anything that isn't a known permission phrasing escalates to the human.
        for s in [
            "Which auth method should we use?",
            "Should I target Postgres or SQLite for this?",
            "What should the default timeout be?",
        ] {
            assert_eq!(classify_prompt(s), PromptClass::Decision, "{s}");
        }
    }

    #[test]
    fn extracts_structured_dialog_from_boxed_permission() {
        let pane = "● Running cargo test…\n\
            ╭──────────────────────────────────────────────╮\n\
            │ Bash command                                 │\n\
            │                                              │\n\
            │   cargo install --path crates/repomon-tui    │\n\
            │   Install the repomon TUI                    │\n\
            │                                              │\n\
            │ Do you want to proceed?                      │\n\
            │ ❯ 1. Yes                                     │\n\
            │   2. Yes, and don't ask again for cargo      │\n\
            │   3. No, and tell Claude what to do          │\n\
            ╰──────────────────────────────────────────────╯";
        let d = detect_dialog(pane).expect("dialog");
        assert_eq!(d.title.as_deref(), Some("Bash command"));
        assert_eq!(d.question, "Do you want to proceed?");
        assert_eq!(
            d.body,
            vec![
                "cargo install --path crates/repomon-tui".to_string(),
                "Install the repomon TUI".to_string(),
            ]
        );
        assert_eq!(
            d.options,
            vec![
                DialogOption {
                    number: Some(1),
                    text: "Yes".into()
                },
                DialogOption {
                    number: Some(2),
                    text: "Yes, and don't ask again for cargo".into()
                },
                DialogOption {
                    number: Some(3),
                    text: "No, and tell Claude what to do".into()
                },
            ]
        );
        assert_eq!(d.selected, Some(0));
        assert_eq!(d.summary(), "Bash command — Do you want to proceed?");
        assert_eq!(d.class(), PromptClass::Permission);
    }

    #[test]
    fn extracts_codex_dialog_without_box_header() {
        // Codex's boxless MCP approval: no `╭` header → no title, no body; `›` cursor row 0.
        let pane = "  Field 1/1\n\
              Allow the repomon MCP server to run tool \"fleet_status\"?\n\
              › 1. Allow                   Run the tool and continue.\n\
                2. Allow for this session  Run the tool and remember this choice for this session.\n\
                3. Always allow            Run the tool and remember this choice for future tool calls.\n\
                4. Cancel                  Cancel this tool call\n\
              enter to submit | esc to cancel";
        let d = detect_dialog(pane).expect("dialog");
        assert_eq!(d.title, None);
        assert_eq!(
            d.question,
            "Allow the repomon MCP server to run tool \"fleet_status\"?"
        );
        assert!(d.body.is_empty());
        assert_eq!(d.options.len(), 4);
        assert_eq!(d.options[3].number, Some(4));
        assert!(d.options[3].text.starts_with("Cancel"));
        assert_eq!(d.selected, Some(0));
        assert_eq!(d.class(), PromptClass::Permission);
    }

    #[test]
    fn trust_dialog_without_question_gets_synthetic_question() {
        let pane = " Security guide\n\n ❯ 1. Yes, I trust this folder\n   2. No, exit\n\n Enter to confirm · Esc to cancel";
        let d = detect_dialog(pane).expect("dialog");
        assert_eq!(d.title, None);
        assert_eq!(d.question, "Do you trust this folder?");
        assert_eq!(d.summary(), "Do you trust this folder?");
        assert_eq!(d.options.len(), 2);
        assert_eq!(d.selected, Some(0));
    }

    #[test]
    fn dialog_matches_summary_on_every_detection() {
        // The structured detector and the summary shim must never disagree.
        for pane in [
            "Do you want to make this edit to app.rs?\n❯ 1. Yes\n  2. No",
            "Which auth method should we use?\n❯ 1. OAuth\n  2. API keys",
        ] {
            let d = detect_dialog(pane).expect("dialog");
            assert_eq!(
                detect_pending_prompt(pane).as_deref(),
                Some(d.summary().as_str())
            );
        }
    }

    #[test]
    fn dialog_body_is_capped() {
        let body: String = (1..=12).map(|i| format!("│ line {i}\n")).collect();
        let pane = format!(
            "╭────────────╮\n│ Bash command │\n{body}│ Do you want to proceed?  │\n│ ❯ 1. Yes │\n│   2. No  │\n╰────────────╯"
        );
        let d = detect_dialog(&pane).expect("dialog");
        assert_eq!(d.body.len(), BODY_MAX_LINES);
        assert_eq!(d.body[0], "line 1");
    }

    #[test]
    fn dialog_select_keys_steers_from_cursor() {
        let d = PendingDialog {
            title: None,
            question: "Do you want to proceed?".into(),
            body: vec![],
            options: vec![
                DialogOption {
                    number: Some(1),
                    text: "Yes".into(),
                },
                DialogOption {
                    number: Some(2),
                    text: "Yes, always".into(),
                },
                DialogOption {
                    number: Some(3),
                    text: "No".into(),
                },
            ],
            selected: Some(0),
        };
        assert_eq!(dialog_select_keys(&d, 2), vec!["Down", "Down", "Enter"]);
        assert_eq!(dialog_select_keys(&d, 0), vec!["Enter"]);
        let up = PendingDialog {
            selected: Some(2),
            ..d
        };
        assert_eq!(dialog_select_keys(&up, 0), vec!["Up", "Up", "Enter"]);
    }

    #[test]
    fn dialog_select_keys_falls_back_to_number_without_cursor() {
        let d = PendingDialog {
            title: None,
            question: "Do you want to proceed?".into(),
            body: vec![],
            options: vec![
                DialogOption {
                    number: Some(1),
                    text: "Yes".into(),
                },
                DialogOption {
                    number: Some(2),
                    text: "No".into(),
                },
            ],
            selected: None,
        };
        assert_eq!(dialog_select_keys(&d, 1), vec!["2", "Enter"]);
    }

    #[test]
    fn pending_dialog_serde_round_trips() {
        let d = detect_dialog("Do you want to proceed?\n❯ 1. Yes\n  2. No").expect("dialog");
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<PendingDialog>(&json).unwrap(), d);
    }
}
