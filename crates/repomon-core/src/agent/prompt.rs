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
/// Maximum number of non-empty context lines collected above a question.
const CONTEXT_MAX_LINES: usize = 12;

/// A fully parsed interactive dialog: what the agent is asking and the choices it offers.
/// [`detect_dialog`] extracts it from pane text; [`detect_pending_prompt`] remains the
/// compact one-line view for callers that only need the "why".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
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
    /// Context lines above the question (for boxless or boxed dialogs), capped at [`CONTEXT_MAX_LINES`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<String>,
}

/// One selectable dialog row: its printed number (if any) and its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
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
///
/// Two recognizers run in order. The box-drawing scan below reads Claude Code, Codex, Hermes and
/// OpenCode, all of which draw a contiguous run of numbered rows under a question. Antigravity
/// 1.1.x draws its menus without a box, follows them with key-hint footers the scan reads as
/// "content below the menu", and lets a long option wrap at the pane width, which splits the
/// contiguous run outright. Its layout gets its own recognizer rather than a weaker generic rule.
pub fn detect_dialog(pane: &str) -> Option<PendingDialog> {
    let stripped: Vec<String> = pane.lines().map(strip_ansi).collect();
    let cleaned: Vec<String> = stripped.iter().map(|l| content(l).to_string()).collect();
    detect_boxed_dialog(&stripped, &cleaned)
        .or_else(|| detect_antigravity_dialog(&stripped, &cleaned))
}

/// The box-drawing recognizer: a contiguous run of option rows under a question, as Claude Code
/// and its lookalikes draw it.
fn detect_boxed_dialog(stripped: &[String], cleaned: &[String]) -> Option<PendingDialog> {
    // Claude draws dialogs inside a box; ANSI and the `│` borders are already stripped, so
    // option/question lines parse the same whether boxed or bare.
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
            // If there is subsequent content below this option block (e.g. the dialog was answered
            // and the agent proceeded or finished), this dialog is dead scrollback, not an active prompt.
            if has_trailing_content(cleaned, block_end) {
                return None;
            }

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
            if let Some((title, question, body, context)) = describe(stripped, cleaned, start) {
                return Some(PendingDialog {
                    title,
                    question,
                    body,
                    options: opts,
                    selected,
                    context,
                });
            }
            // Hermes 0.19's dangerous-command panel has a title, command and numbered choices,
            // but deliberately no question sentence. Recognize that exact branded layout instead
            // of weakening the generic question requirement for arbitrary numbered menus.
            if let Some((title, question, body, context)) =
                describe_hermes_approval(stripped, cleaned, start)
            {
                return Some(PendingDialog {
                    title,
                    question,
                    body,
                    options: opts,
                    selected,
                    context,
                });
            }
            // Claude's folder-trust dialog ("Security guide" / "Yes, I trust this folder").
            // On a freshly spawned worker the question line ("Do you trust the files in this
            // folder?") can be scrolled out of the capture window entirely, so `describe`
            // above finds no question and comes back empty. Recognize the dialog by its
            // distinctive first option plus its confirmation footer instead — see the live
            // fixture in the tests below.
            if is_trust_dialog(&block) && has_confirm_footer(cleaned, block_end) {
                return Some(PendingDialog {
                    title: None,
                    question: "Do you trust this folder?".to_string(),
                    body: Vec::new(),
                    options: opts,
                    selected,
                    context: Vec::new(),
                });
            }
            return None;
        }
        end = start; // not a menu — keep scanning the lines above
    }
    None
}

/// The footers Antigravity 1.1.x prints under a live dialog: its in-flight status line
/// ("esc to cancel") and the key hints below the menu ("↑/↓ Navigate · tab Amend · ctrl+g
/// edit/expand command", "↑/↓ Navigate · enter Confirm"). Once a dialog is answered the status
/// line flips to "? for shortcuts", so the presence of one of these separates a pending prompt
/// from answered scrollback.
fn is_antigravity_live_footer(line: &str) -> bool {
    let t = line.trim();
    let lower = t.to_lowercase();
    if lower.starts_with("esc to cancel") {
        return true;
    }
    // A key-hint row: the word "Navigate" alongside the arrow keys or a hint separator. Requiring
    // both keeps an agent's own sentence about navigating out of the footer set.
    lower.contains("navigate")
        && (t.contains('\u{2191}') || t.contains('\u{2193}') || t.contains('\u{b7}'))
}

/// Lines that may sit between an Antigravity menu's last option and the bottom of the pane
/// without meaning the dialog is dead scrollback: its own footers, the empty composer, rules,
/// and the right-aligned model status bar.
fn is_antigravity_tail_furniture(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() || t == ">" || t == "\u{276f}" {
        return true;
    }
    if is_antigravity_live_footer(t) {
        return true;
    }
    if t.chars().all(|c| "\u{2570}\u{256f}\u{2500}\u{2501}\u{2502}\u{250c}\u{2510}\u{2514}\u{2518}\u{251c}\u{2524}\u{253c}_\u{256d}\u{256e}\u{b7}| ".contains(c)) {
        return true;
    }
    // The status bar Antigravity right-aligns on its own line ("Gemini 3.8 Flash · high",
    // "accept-edits · Gemini 3.8 Flash · high"): dot-separated fragments, no sentence.
    t.contains('\u{b7}') && !t.ends_with('?') && t.split_whitespace().count() <= 10
}

/// How far above the pane's bottom an Antigravity question may sit and still be the live one.
const ANTIGRAVITY_TAIL_REACH: usize = 40;
/// How many lines of the dialog's own explanation may sit between its question and its first
/// option row (the folder-trust dialog prints one).
const PREAMBLE_MAX_LINES: usize = 3;
/// The longest an unnumbered, uncursored row may be and still read as a menu choice rather than
/// as prose that landed under the menu.
const OPTION_MAX_CHARS: usize = 120;

/// Antigravity 1.1.x's boxless dialog: a question line, then option rows, then key-hint footers.
///
/// Three things defeat [`detect_boxed_dialog`] here. The hint rows below the menu ("↑/↓ Navigate
/// · tab Amend …") read as content below the block, so a live prompt is discarded as scrollback.
/// The folder-trust dialog offers two rows with no numbers at all, below the "two numbered rows"
/// gate. And at the 80 columns the production tmux server usually runs, the long "(Persist to
/// settings.json)" option wraps, which breaks the contiguous run the scan walks. This recognizer
/// anchors on the question instead, absorbs wrapped remainders into the option above them, and
/// requires one of Antigravity's live footers below the menu so an answered dialog still in
/// scrollback does not resurrect.
fn detect_antigravity_dialog(stripped: &[String], cleaned: &[String]) -> Option<PendingDialog> {
    // Measure the tail from the last line with content, not from the bottom of the capture: tmux
    // pads a short pane with blank rows, and counting those pushed the dialog out of reach.
    let last_content = cleaned.iter().rposition(|l| !l.trim().is_empty())?;
    let first = (last_content + 1).saturating_sub(ANTIGRAVITY_TAIL_REACH);
    // The bottom-most question in the tail is the live one.
    let question_at = cleaned[first..]
        .iter()
        .rposition(|l| {
            let t = l.trim();
            t.ends_with('?') && t.split_whitespace().count() >= 3 && !t.starts_with('#')
        })
        .map(|i| i + first);
    // The folder-trust dialog opens the session, so on a 45-line capture of a 50-line pane its
    // question ("Do you trust the contents of this project?") has already scrolled out of view --
    // the same blind spot the Claude trust branch above covers. Anchor on the option wording and
    // stand in the question, as that branch does.
    let (q, synthetic) = match question_at {
        Some(q) => (q, None),
        None => {
            let row = cleaned[first..]
                .iter()
                .position(|l| {
                    parse_option_line(l).is_some_and(|(_, _, text)| is_trust_option(text.trim()))
                })
                .map(|i| i + first)?;
            (
                row.saturating_sub(1),
                Some("Do you trust this folder?".to_string()),
            )
        }
    };

    let mut options: Vec<DialogOption> = Vec::new();
    let mut selected: Option<usize> = None;
    let mut saw_footer = false;
    let mut preamble = 0usize;
    let synthetic_question = synthetic.is_some();
    let scan_from = if synthetic_question { q } else { q + 1 };
    for (line, raw) in cleaned[scan_from..].iter().zip(&stripped[scan_from..]) {
        if let Some((cursor, number, text)) = parse_option_line(line) {
            if cursor && number.is_none() && text.is_empty() {
                continue;
            }
            if cursor {
                selected = Some(options.len());
            }
            options.push(DialogOption {
                number,
                text: text.trim().to_string(),
            });
            continue;
        }
        if is_antigravity_live_footer(line) {
            saw_footer = true;
            continue;
        }
        if is_antigravity_tail_furniture(line) {
            continue;
        }
        if options.is_empty() {
            // Above the first option this is the dialog's own body -- the trust dialog explains
            // itself between its question and its choices. Allow a short preamble, no more.
            if preamble < PREAMBLE_MAX_LINES {
                preamble += 1;
                continue;
            }
            return None;
        }
        if saw_footer {
            // Content under the key hints means the dialog was answered and the agent moved on.
            return None;
        }
        // A row that carries neither the cursor nor a number (the trust dialog's "No, exit") is
        // still an option when the terminal indented it to align under its siblings. A hard wrap
        // resumes at column zero instead, so column zero is the remainder of the option above.
        if raw.starts_with(char::is_whitespace) && line.chars().count() <= OPTION_MAX_CHARS {
            options.push(DialogOption {
                number: None,
                text: line.trim().to_string(),
            });
            continue;
        }
        let last = options.last_mut()?;
        last.text.push(' ');
        last.text.push_str(line.trim());
    }
    if options.len() < 2 || !saw_footer {
        return None;
    }
    // The usage-limit menu is the auto-continue watcher's, not a permission ask.
    if options
        .iter()
        .any(|o| is_limit_option(&o.text.to_lowercase()))
    {
        return None;
    }
    let question = match synthetic {
        Some(q) => q,
        None => cleaned[q].trim().to_string(),
    };
    let context: Vec<String> = cleaned[first..q]
        .iter()
        .rev()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .take(CONTEXT_MAX_LINES)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let body: Vec<String> = context
        .iter()
        .rev()
        .take(BODY_MAX_LINES)
        .rev()
        .cloned()
        .collect();
    Some(PendingDialog {
        title: None,
        question,
        body,
        options,
        selected,
        context,
    })
}

/// An Antigravity quota wall: the model refused the turn because the account's per-model quota is
/// spent, and the pane drops straight back to an idle composer. The status is honestly Idle, but
/// "no output for 4m" hides the cause, so report the wall (and its reset window when printed).
///
/// Claude's own usage limit is [`super::limit`]'s job (it has a menu and an auto-continue path),
/// so its wording is deliberately not matched here.
pub fn detect_quota_exhausted(pane: &str) -> Option<String> {
    let lines: Vec<String> = pane.lines().map(strip_ansi).collect();
    let hit = lines.iter().rposition(|l| {
        let t = l.trim();
        let lower = t.to_lowercase();
        // A wall is stated, not introduced: a trailing colon is the agent talking about quotas.
        !t.ends_with(':')
            && (lower.contains("quota reached")
                || lower.contains("quota exceeded")
                || lower.contains("out of quota"))
    })?;
    let mut reason = "quota exhausted".to_string();
    // The reset window is printed on the wall line or just below it.
    let window = lines[hit..(hit + 3).min(lines.len())]
        .iter()
        .find_map(|l| parse_reset_window(l));
    if let Some(w) = window {
        reason = format!("quota exhausted, resets in {w}");
    }
    Some(reason)
}

/// Pull "2h 15m" out of a "Resets in 2h 15m." line.
fn parse_reset_window(line: &str) -> Option<String> {
    let lower = line.to_lowercase();
    let at = lower
        .find("resets in ")
        .or_else(|| lower.find("reset in "))?;
    let rest = line[at..].split_once(" in ").map(|(_, r)| r)?;
    let window: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || c.is_ascii_alphabetic() || *c == ' ')
        .collect();
    let window = window.trim();
    if window.is_empty() {
        return None;
    }
    // Keep only the duration tokens ("2h", "15m"); stop at the first word that is not one.
    let tokens: Vec<&str> = window
        .split_whitespace()
        .take_while(|w| {
            let mut cs = w.chars();
            cs.next().is_some_and(|c| c.is_ascii_digit())
                && w.chars().all(|c| c.is_ascii_digit() || "hms".contains(c))
        })
        .collect();
    if tokens.is_empty() {
        return None;
    }
    Some(tokens.join(" "))
}

/// Parse a `detect_quota_exhausted` reason's trailing "resets in 2h 15m" window into an actual
/// duration, so a caller can turn it into an absolute deadline once (at first sight) instead of
/// trusting the wall message to still be accurate however long it lingers in a short pane
/// capture. Returns `None` when the reason names no reset window (the open-ended "quota
/// exhausted" case, which a caller should keep retrying on presence alone).
pub fn parse_reset_duration(reason: &str) -> Option<chrono::Duration> {
    let window = parse_reset_window(reason)?;
    let mut total = chrono::Duration::zero();
    let mut found = false;
    for tok in window.split_whitespace() {
        let digits: String = tok.chars().take_while(|c| c.is_ascii_digit()).collect();
        let unit = &tok[digits.len()..];
        let n: i64 = digits.parse().ok()?;
        total += match unit {
            "h" => chrono::Duration::hours(n),
            "m" => chrono::Duration::minutes(n),
            "s" => chrono::Duration::seconds(n),
            _ => return None,
        };
        found = true;
    }
    found.then_some(total)
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

type DialogDescription = (Option<String>, String, Vec<String>, Vec<String>);

fn describe_hermes_approval(
    stripped: &[String],
    cleaned: &[String],
    menu_start: usize,
) -> Option<DialogDescription> {
    let border = (menu_start.saturating_sub(HEADER_REACH)..menu_start)
        .rev()
        .find(|&i| stripped[i].trim_start().starts_with('╭'))?;
    let title_idx = (border + 1..menu_start).find(|&i| !cleaned[i].trim().is_empty())?;
    let title = cleaned[title_idx].trim();
    if !title.contains("Dangerous Command") {
        return None;
    }
    let body = (title_idx + 1..menu_start)
        .map(|i| cleaned[i].trim())
        .filter(|line| !line.is_empty())
        .take(BODY_MAX_LINES)
        .map(|line| truncate(line, 120))
        .collect();
    Some((
        Some("Dangerous Command".into()),
        "Do you want to allow this command?".into(),
        body,
        Vec::new(),
    ))
}

/// Describe the dialog whose menu starts at line `menu_start`: the question line just above
/// it, the header (the first content line under the box's `╭` border), the body lines
/// between header and question (capped at [`BODY_MAX_LINES`]), and context lines above the
/// question (capped at [`CONTEXT_MAX_LINES`]).
fn describe(
    stripped: &[String],
    cleaned: &[String],
    menu_start: usize,
) -> Option<DialogDescription> {
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

    let mut raw_context = Vec::new();
    let mut blank_run = 0;
    if q_idx > 0 {
        for i in (0..q_idx).rev() {
            let s = stripped[i].trim_start();
            if s.starts_with('╭')
                || s.starts_with('╰')
                || stripped[i].contains('╭')
                || stripped[i].contains('╰')
            {
                break;
            }
            let c = cleaned[i].trim();
            if c.is_empty() {
                blank_run += 1;
                if blank_run >= 2 {
                    break;
                }
            } else {
                blank_run = 0;
                raw_context.push(truncate(c, 120));
                if raw_context.len() >= CONTEXT_MAX_LINES {
                    break;
                }
            }
        }
    }
    raw_context.reverse();
    let context = raw_context;

    Some((title, question, body, context))
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
        .is_some_and(|(_, _, text)| is_trust_option(text.trim()))
}

/// The affirmative row every folder-trust dialog offers, Claude's and Antigravity's alike.
fn is_trust_option(text: &str) -> bool {
    text.eq_ignore_ascii_case("Yes, I trust this folder")
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

/// Whether content exists below `block_end` indicating this dialog is historical scrollback
/// rather than the active prompt at the tail of the pane.
fn has_trailing_content(cleaned: &[String], block_end: usize) -> bool {
    let is_footer_or_border = |line: &str| {
        let t = line.trim();
        if t.is_empty() {
            return true;
        }
        let lower = t.to_lowercase();
        if lower.contains("enter to confirm")
            || lower.contains("enter to submit")
            || lower.contains("esc to cancel")
            || lower.contains("type a number")
            || lower.contains("use arrow keys")
            || lower.contains("for shortcuts")
            || t == "❯"
            || t == ">"
            || t.chars().all(|c| "╰╯─━│┌┐└┘├┤┼_╭╮·| ".contains(c))
        {
            return true;
        }
        false
    };

    cleaned[block_end + 1..]
        .iter()
        .any(|l| !is_footer_or_border(l))
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

/// Glyphs a coding CLI cycles through while a turn is streaming. Claude Code 2.1.x rotates the
/// asterisk family.
const SPINNER_GLYPHS: [char; 6] = [
    '\u{273b}', '\u{273d}', '\u{2733}', '\u{2736}', '\u{2722}', '\u{2217}',
];

/// Whether `c` opens a spinner frame. Codex rotates the four-dot braille cycle
/// (U+2807, U+280B, U+2819 …) while Antigravity 1.1.x rotates the eight-dot one
/// (U+28FE, U+28FD, U+28FB, U+28BF, U+287F, U+28DF, U+28EF, U+28F7); enumerating either cycle
/// glyph by glyph is how the eight-dot family came to be missed entirely, so accept the whole
/// Braille Patterns block instead. U+2800 (the blank pattern) is excluded: TUIs use it as an
/// invisible spacer, and a blank cannot be a visible spinner frame.
fn is_spinner_glyph(c: char) -> bool {
    SPINNER_GLYPHS.contains(&c) || ('\u{2801}'..='\u{28ff}').contains(&c)
}

/// The "N background agents are still working" status line, when this pane line really IS that
/// status line rather than an agent's own prose *about* background agents.
///
/// The line is printed on its own, optionally behind a spinner glyph:
///
/// ```text
/// Waiting for 2 background agents to finish
/// 3 background agents running
/// ```
///
/// The previous rule accepted "background agent" plus "running" anywhere on a line, so an agent
/// that merely wrote "Here is the status of the running background agents:" pinned its own window
/// to Running for as long as that sentence stayed in the captured scrollback.
fn subagent_wait_count(line: &str) -> Option<usize> {
    // Drop a leading spinner/bullet glyph and its padding; a digit is alphanumeric, so a line
    // that opens with its count survives.
    let t = line
        .trim()
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .trim();
    let lower = t.to_lowercase();
    if !(lower.contains("background agent")
        || lower.contains("background task")
        || lower.contains("subagent"))
    {
        return None;
    }
    // Prose introduces something; a status line states it. A trailing colon is prose.
    if t.ends_with(':') {
        return None;
    }
    let is_wait_line = lower.starts_with("waiting for");
    let is_count_line = t
        .split_whitespace()
        .next()
        .is_some_and(|w| w.parse::<usize>().is_ok())
        && lower.contains("running");
    if !(is_wait_line || is_count_line) {
        return None;
    }
    Some(
        t.split_whitespace()
            .find_map(|w| w.parse::<usize>().ok())
            .unwrap_or(1),
    )
}

/// A live streaming/thinking indicator on screen, as the short phrase behind it.
///
/// This is the only signal that survives a long tool call: the transcript-derived status decays to
/// `Idle` after two silent minutes, so an agent grinding through a five-minute build reads Idle
/// while its pane is visibly working. A spinner line is what the operator actually sees, so it is
/// what "running" is defined against.
///
/// A finished turn keeps its glyph but stamps the result (`\u{273b} Baked for 1m 9s . done 1:00 PM`),
/// so a "done" stamp disqualifies the line.
pub fn detect_active_spinner(pane: &str) -> Option<String> {
    // Bottom-up: a capture carries scrollback, and only the last frame describes what the pane is
    // doing now. Scanning top-down reported a stale frame's phrase.
    let mut in_flight_footer = false;
    for line in pane.lines().rev().map(strip_ansi) {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let lower = t.to_lowercase();
        // Antigravity's in-flight footer. It prints "esc to cancel" at the head of its status line
        // for exactly as long as a turn is running and swaps it for "? for shortcuts" the moment
        // the turn ends, so it survives a capture that lands between spinner redraws. Claude
        // carries the same words inside a key hint ("Enter to confirm / Esc to cancel"), never at
        // the head of a line, so anchoring on the start keeps the two apart. It is the weaker
        // witness of the two: keep looking for a glyph line, whose phrase is what to report.
        if lower.starts_with("esc to cancel") {
            in_flight_footer = true;
            continue;
        }
        // A turn that has ended still shows its glyph plus a "done" stamp.
        if lower.contains("done ") || lower.ends_with("done") {
            continue;
        }
        let spinning = t.starts_with(is_spinner_glyph) || lower.contains("esc to interrupt");
        if !spinning {
            continue;
        }
        // The glyph alone (a bare redraw frame) says nothing worth reporting.
        let phrase = t.trim_start_matches(is_spinner_glyph).trim();
        if phrase.is_empty() {
            continue;
        }
        return Some(truncate(phrase, 60));
    }
    in_flight_footer.then(|| "turn in flight".to_string())
}

/// Detect running background subagents from pane text.
/// Returns a concise description if subagent(s) are actively running.
pub fn detect_subagent_running(pane: &str) -> Option<String> {
    let stripped: Vec<String> = pane.lines().map(strip_ansi).collect();
    let mut waiting_count: Option<usize> = None;
    let mut active_tasks: Vec<(String, Option<String>)> = Vec::new();

    for line in stripped.iter() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let lower = t.to_lowercase();

        // 1. The "N background agents still working" status line.
        if let Some(count) = subagent_wait_count(t) {
            waiting_count = Some(count);
        }

        // 2. Match active subagent row: `◯ <kind> <description> <timer> ...`
        // Ensure it is not a finished log (e.g. `⏺ Agent "..." finished · 14m 22s`)
        if (t.contains('◯') || t.contains('○') || t.starts_with("◯") || t.starts_with("○"))
            && !lower.contains("finished")
        {
            if let Some((desc, timer)) = parse_subagent_row(t) {
                if !active_tasks.iter().any(|(d, _)| d == &desc) {
                    active_tasks.push((desc, timer));
                }
            }
        }
    }

    if active_tasks.is_empty() && waiting_count.is_none() {
        return None;
    }

    if let Some((desc, timer)) = active_tasks.first() {
        let timer_str = timer
            .as_deref()
            .map(|t| format!(" ({t})"))
            .unwrap_or_default();
        if active_tasks.len() > 1 {
            Some(format!(
                "{desc}{timer_str} +{} more",
                active_tasks.len() - 1
            ))
        } else if let Some(count) = waiting_count {
            if count > 1 {
                Some(format!("{desc}{timer_str} +{} more", count - 1))
            } else {
                Some(format!("{desc}{timer_str}"))
            }
        } else {
            Some(format!("{desc}{timer_str}"))
        }
    } else if let Some(count) = waiting_count {
        if count == 1 {
            Some("Waiting for 1 background agent to finish".to_string())
        } else {
            Some(format!("Waiting for {count} background agents to finish"))
        }
    } else {
        None
    }
}

/// Parse an individual subagent row (e.g. `◯ general-purpose  Editing interruption defaults in main.py  6m 54s · ↓ 134.9k tokens`)
fn parse_subagent_row(line: &str) -> Option<(String, Option<String>)> {
    let clean = line.trim();
    let circle_idx = clean.find(['◯', '○'])?;
    let rest = clean[circle_idx + '◯'.len_utf8()..].trim();
    if rest.is_empty() || rest.starts_with("main") || rest.eq_ignore_ascii_case("main") {
        return None;
    }
    if rest.to_lowercase().contains("finished") {
        return None;
    }

    if let Some((timer, timer_start, _)) = extract_timer(rest) {
        let before_timer = rest[..timer_start].trim();
        if before_timer.is_empty() {
            return None;
        }
        // If there is a double space separating agent kind and task description:
        let desc = if let Some(idx) = before_timer.find("  ") {
            let task = before_timer[idx..].trim();
            if !task.is_empty() { task } else { before_timer }
        } else {
            before_timer
        };
        Some((truncate(desc, 60), Some(timer)))
    } else {
        let desc = if let Some(idx) = rest.find("  ") {
            let task = rest[idx..].trim();
            if !task.is_empty() { task } else { rest }
        } else {
            rest
        };
        Some((truncate(desc, 60), None))
    }
}

/// Extract timer pattern from text (e.g. "6m 54s", "14s", "3m 16s", "1h 12m", "45s")
fn extract_timer(s: &str) -> Option<(String, usize, usize)> {
    let bytes = s.as_bytes();
    let len = bytes.len();
    for i in 0..len {
        if !bytes[i].is_ascii_digit() {
            continue;
        }
        if i > 0
            && (bytes[i - 1].is_ascii_alphanumeric()
                || bytes[i - 1] == b'_'
                || bytes[i - 1] == b'-')
        {
            continue;
        }
        let mut curr = i;
        let mut found_unit = false;
        let mut has_h = false;
        let mut has_m = false;
        let mut has_s = false;

        while curr < len {
            let start_digits = curr;
            while curr < len && bytes[curr].is_ascii_digit() {
                curr += 1;
            }
            if curr == start_digits {
                break;
            }
            while curr < len && bytes[curr] == b' ' {
                curr += 1;
            }
            if curr < len && (bytes[curr] == b'h' || bytes[curr] == b'm' || bytes[curr] == b's') {
                let u = bytes[curr];
                curr += 1;
                found_unit = true;
                if u == b'h' {
                    has_h = true;
                }
                if u == b'm' {
                    has_m = true;
                }
                if u == b's' {
                    has_s = true;
                }

                while curr < len && bytes[curr] == b' ' {
                    curr += 1;
                }
                if curr < len && bytes[curr].is_ascii_digit() {
                    if (u == b'h' && (has_m || has_s)) || (u == b'm' && has_s) || u == b's' {
                        break;
                    }
                    continue;
                }
                break;
            } else {
                break;
            }
        }

        if found_unit && (has_h || has_m || has_s) {
            let timer_str = s[i..curr].trim().to_string();
            return Some((timer_str, i, curr));
        }
    }
    None
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
    fn detects_hermes_dangerous_command_dialog_without_question_line() {
        // Hermes 0.19's live renderer deliberately provides a title, command, and choices but
        // no question sentence. Keep this branded fixture narrow so ordinary numbered menus
        // without questions remain invisible to the prompt detector.
        let pane = "╭──────────────────────────────────────╮\n\
            │ ⚠️  Dangerous Command                │\n\
            │                                      │\n\
            │ cargo clean                          │\n\
            │                                      │\n\
            │ ❯ 1. Allow once                      │\n\
            │   2. Allow for this session          │\n\
            │   3. Add to permanent allowlist      │\n\
            │   4. Deny                            │\n\
            ╰──────────────────────────────────────╯";
        let dialog = detect_dialog(pane).expect("Hermes approval dialog");
        assert_eq!(dialog.title.as_deref(), Some("Dangerous Command"));
        assert_eq!(dialog.question, "Do you want to allow this command?");
        assert_eq!(dialog.body, ["cargo clean"]);
        assert_eq!(dialog.options.len(), 4);
        assert_eq!(dialog.selected, Some(0));
        assert_eq!(dialog.class(), PromptClass::Permission);
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
    fn answered_dialog_with_trailing_prose_is_not_a_pending_prompt() {
        let pane = "╭───────────────────────────────╮\n\
            │ Do you want to proceed?       │\n\
            │ ❯ 1. Yes                      │\n\
            │   2. No                       │\n\
            ╰───────────────────────────────╯\n\
            ● ran the tool\n\
            I have completed the task and all tests pass.";
        assert_eq!(detect_pending_prompt(pane), None);
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
            context: vec![],
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
            context: vec![],
        };
        assert_eq!(dialog_select_keys(&d, 1), vec!["2", "Enter"]);
    }

    #[test]
    fn pending_dialog_serde_round_trips() {
        let d = detect_dialog("Do you want to proceed?\n❯ 1. Yes\n  2. No").expect("dialog");
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<PendingDialog>(&json).unwrap(), d);
    }

    #[test]
    fn detects_antigravity_permission_menu() {
        let pane = r#"
Requesting permission for:
   ps aux | grep -i repomon

Do you want to proceed?
> 1. Yes
  2. Yes, and always allow in this conversation
  3. Yes, and always allow in settings
  4. No
"#;
        let dialog = detect_dialog(pane).expect("Antigravity dialog");
        assert_eq!(dialog.question, "Do you want to proceed?");
        assert_eq!(dialog.selected, Some(0));
        assert_eq!(dialog.options.len(), 4);
        assert_eq!(
            dialog.context,
            vec![
                "Requesting permission for:".to_string(),
                "ps aux | grep -i repomon".to_string()
            ]
        );
    }

    #[test]
    fn detects_claude_code_subagent_with_timer_and_tokens() {
        let pane = r#"
✻ Waiting for 1 background agent to finish

  ◯ general-purpose  Drafting 2026-08-api-contract.md  3m 16s . down 85.1k tokens
"#;
        let sub = detect_subagent_running(pane);
        assert_eq!(
            sub.as_deref(),
            Some("Drafting 2026-08-api-contract.md (3m 16s)")
        );
    }

    #[test]
    fn detects_multiple_claude_code_subagents() {
        let pane = r#"
✻ Waiting for 2 background agents to finish

───────────────────────────────────────────────────────────────────────────── voice-ai ──
❯ status update
─────────────────────────────────────────────────────────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to cycle) · ← 2 agents · ↓ to manage                     /rc

  ⏺ main
  ◯ general-purpose  Editing interruption defaults in main.py                  6m 54s · ↓ 134.9k tokens
  ◯ general-purpose  P9: widget design overhaul                                    14s · ↓ 37.6k tokens
"#;
        let sub = detect_subagent_running(pane);
        assert_eq!(
            sub.as_deref(),
            Some("Editing interruption defaults in main.py (6m 54s) +1 more")
        );
    }

    #[test]
    fn detects_waiting_for_background_agent_header_alone() {
        let pane = "✻ Waiting for 1 background agent to finish\n❯ idle prompt";
        let sub = detect_subagent_running(pane);
        assert_eq!(
            sub.as_deref(),
            Some("Waiting for 1 background agent to finish")
        );
    }

    /// Real capture, 2026-09-04, of an Antigravity window parked at an empty composer whose
    /// scrollback still held the agent's own sentence "Here is the status of the running
    /// background agents:". The daemon reported it Running for as long as that line stayed in
    /// the captured window.
    #[test]
    fn prose_about_background_agents_is_not_a_running_subagent() {
        let pane = include_str!("fixtures/antigravity_idle_prose_subagents.txt");
        assert_eq!(detect_subagent_running(pane), None);
        assert_eq!(detect_active_spinner(pane), None);
    }

    #[test]
    fn prose_mentioning_background_agents_never_counts() {
        assert_eq!(
            detect_subagent_running("Here is the status of the running background agents:"),
            None
        );
        assert_eq!(
            detect_subagent_running("I dispatched the background agents and they are running now"),
            None
        );
    }

    #[test]
    fn bare_count_status_line_still_counts() {
        assert_eq!(
            detect_subagent_running("3 background agents running").as_deref(),
            Some("Waiting for 3 background agents to finish")
        );
    }

    /// Real capture, 2026-09-04, of a Claude Code window whose pane changed inside six seconds
    /// while the daemon reported it `waiting` (rendered as NEEDS YOU).
    #[test]
    fn claude_pane_with_live_subagents_reads_running() {
        let pane = include_str!("fixtures/claude_running_subagents.txt");
        assert!(detect_subagent_running(pane).is_some());
        assert_eq!(
            detect_active_spinner(pane).as_deref(),
            Some("Waiting for 2 background agents to finish")
        );
        assert_eq!(detect_dialog(pane), None);
    }

    /// Real capture, 2026-09-04: a finished Claude turn keeps its spinner glyph but stamps the
    /// result, so the glyph alone must not read as "still working".
    #[test]
    fn finished_turn_spinner_stamp_is_not_active() {
        let pane = include_str!("fixtures/claude_idle_done_spinner.txt");
        assert_eq!(detect_active_spinner(pane), None);
        assert_eq!(detect_subagent_running(pane), None);
    }

    /// Real capture, 2026-09-04: an Antigravity window at an idle prompt whose scrollback holds
    /// the near-miss sentence "Waiting for cargo test -p repomon-daemon to finish."
    #[test]
    fn idle_antigravity_prompt_reads_neither_running_nor_dialog() {
        let pane = include_str!("fixtures/antigravity_idle_prompt.txt");
        assert_eq!(detect_subagent_running(pane), None);
        assert_eq!(detect_active_spinner(pane), None);
    }

    #[test]
    fn detects_streaming_spinner_without_done_stamp() {
        assert_eq!(
            detect_active_spinner("\u{273b} Thinking\u{2026} (12s \u{00b7} esc to interrupt)")
                .as_deref(),
            Some("Thinking\u{2026} (12s \u{00b7} esc to interrupt)")
        );
    }

    /// Live capture, 2026-09-04, of an Antigravity 1.1.12 window mid turn: the spinner is the
    /// eight-dot braille cycle (U+28FE family), not the four-dot one Claude and Codex use, and the
    /// live footer reads "esc to cancel" rather than "esc to interrupt". Neither matched, so a
    /// visibly working pane reported idle for the whole turn.
    #[test]
    fn antigravity_working_pane_reads_running() {
        let pane = include_str!("fixtures/antigravity_working_spinner.txt");
        assert_eq!(
            detect_active_spinner(pane).as_deref(),
            Some("Reading file...")
        );
        assert_eq!(detect_dialog(pane), None);
    }

    /// The eight-dot cycle Antigravity rotates through, each glyph on its own.
    #[test]
    fn every_braille_spinner_glyph_reads_as_working() {
        for glyph in [
            '\u{28fe}', '\u{28fd}', '\u{28fb}', '\u{28bf}', '\u{287f}', '\u{28df}', '\u{28ef}',
            '\u{28f7}',
        ] {
            let pane = format!("{glyph}  Understanding Task Parallelization...");
            assert_eq!(
                detect_active_spinner(&pane).as_deref(),
                Some("Understanding Task Parallelization..."),
                "glyph {glyph} did not read as a spinner"
            );
        }
    }

    /// A capture that lands between redraws loses the glyph but keeps the footer, which
    /// Antigravity prints only while a turn is in flight.
    #[test]
    fn antigravity_cancel_footer_alone_reads_as_working() {
        let pane = "\u{25cf} Bash(cargo test -p repomon-core)\nesc to cancel";
        assert_eq!(
            detect_active_spinner(pane).as_deref(),
            Some("turn in flight")
        );
    }

    /// Claude's folder-trust dialog carries "Enter to confirm \u{b7} Esc to cancel" as a key hint,
    /// which must not read as Antigravity's live footer.
    #[test]
    fn claude_confirm_hint_is_not_a_live_footer() {
        let pane = include_str!("fixtures/trust_prompt.txt");
        assert_eq!(detect_active_spinner(pane), None);
    }

    /// Live capture, 2026-09-04: Antigravity blocked on a Bash permission ask. The generic scan
    /// threw it away because the "\u{2191}/\u{2193} Navigate \u{b7} tab Amend" hint below the menu counted as
    /// trailing content, so the daemon reported idle while the pane waited on a human.
    #[test]
    fn antigravity_permission_dialog_is_a_pending_prompt() {
        let pane = include_str!("fixtures/antigravity_permission_dialog.txt");
        let dialog = detect_dialog(pane).expect("permission dialog");
        assert_eq!(dialog.question, "Do you want to proceed?");
        assert_eq!(dialog.options.len(), 4);
        assert_eq!(dialog.options[0].text, "Yes");
        assert_eq!(dialog.options[3].text, "No");
        assert_eq!(dialog.selected, Some(0));
    }

    /// The same capture re-wrapped at 80 columns, the width the production tmux server usually
    /// runs. The long "(Persist to settings.json)" option wraps, which splits the contiguous
    /// option run the generic scan needs.
    #[test]
    fn antigravity_permission_dialog_survives_an_80_column_wrap() {
        let pane = include_str!("fixtures/antigravity_permission_dialog_80col.txt");
        let dialog = detect_dialog(pane).expect("permission dialog at 80 columns");
        assert_eq!(dialog.question, "Do you want to proceed?");
        assert_eq!(dialog.options.len(), 4);
        assert_eq!(dialog.options[3].text, "No");
    }

    /// Live capture, 2026-09-04: Antigravity's folder-trust dialog offers two rows and no numbers
    /// at all, so the "two numbered rows" gate never let it through.
    #[test]
    fn antigravity_trust_dialog_is_a_pending_prompt() {
        let pane = include_str!("fixtures/antigravity_trust_dialog.txt");
        let dialog = detect_dialog(pane).expect("trust dialog");
        assert!(
            dialog.question.to_lowercase().contains("trust"),
            "unexpected question: {}",
            dialog.question
        );
        assert_eq!(dialog.options.len(), 2);
        assert_eq!(dialog.options[0].text, "Yes, I trust this folder");
    }

    /// Live capture, 2026-09-04: an Antigravity window parked at its composer while a background
    /// shell task keeps ticking in the footer. The task line is not the agent working.
    #[test]
    fn antigravity_background_task_ticking_is_still_idle() {
        let pane = include_str!("fixtures/antigravity_idle_background_task.txt");
        assert_eq!(detect_active_spinner(pane), None);
        assert_eq!(detect_subagent_running(pane), None);
        assert_eq!(detect_dialog(pane), None);
        assert_eq!(detect_quota_exhausted(pane), None);
    }

    /// Live capture, 2026-09-04: the same window one second after the turn ended. The footer flips
    /// from "esc to cancel" to "? for shortcuts" and the composer is bare.
    #[test]
    fn antigravity_finished_turn_reads_idle() {
        let pane = include_str!("fixtures/antigravity_idle_after_turn.txt");
        assert_eq!(detect_active_spinner(pane), None);
        assert_eq!(detect_dialog(pane), None);
    }

    /// A quota-exhausted Antigravity pane is idle, but "no output for 4m" hides why. Report the
    /// reset window so the sidebar can say what actually stopped the agent.
    #[test]
    fn antigravity_quota_error_is_reported_as_exhausted() {
        let pane = include_str!("fixtures/antigravity_quota_exhausted.txt");
        assert_eq!(
            detect_quota_exhausted(pane).as_deref(),
            Some("quota exhausted, resets in 2h 15m")
        );
        assert_eq!(detect_active_spinner(pane), None);
        assert_eq!(detect_dialog(pane), None);
    }

    #[test]
    fn parses_the_quota_reset_window_into_a_duration() {
        assert_eq!(
            parse_reset_duration("quota exhausted, resets in 2h 15m"),
            Some(chrono::Duration::hours(2) + chrono::Duration::minutes(15))
        );
        assert_eq!(
            parse_reset_duration("quota exhausted"),
            None,
            "an open-ended wall names no window to parse"
        );
    }

    /// Prose about quotas is not a quota wall, and neither is Claude's own usage-limit copy
    /// (which [`super::limit`] owns and auto-continues).
    #[test]
    fn quota_prose_and_claude_limits_are_not_antigravity_quota_walls() {
        assert_eq!(
            detect_quota_exhausted("I checked whether the individual quota reached its cap:"),
            None
        );
        assert_eq!(
            detect_quota_exhausted("Claude usage limit reached. Your limit will reset at 3:00 PM."),
            None
        );
    }

    #[test]
    fn ignores_finished_subagents() {
        let pane = r#"
⏺ Agent "P7: site content ingestion packet" finished · 14m 22s
⏺ All tasks completed
❯ 
"#;
        assert_eq!(detect_subagent_running(pane), None);
    }
}
