//! Detecting a pending interactive prompt (permission dialog, plan approval, trust dialog,
//! a question with options) in a managed agent's pane.
//!
//! A transcript that ends in a tool call reads as **Running**, but the pane may actually be
//! sitting on "Do you want to proceed? ❯ 1. Yes …" — blocked on the user, with nothing in the
//! JSONL to say so. This module is the pure, fixture-tested detector: given recent pane text it
//! decides whether the agent is waiting on an interactive prompt, and produces a compact
//! summary (dialog header + question) to use as the notification's "why". The daemon flips
//! such sessions to `Waiting` during `lane.list`.

use super::limit::parse_option_line;
use super::text::strip_ansi;

/// How far above the option menu the question line may sit.
const QUESTION_REACH: usize = 5;
/// How far above the question to look for the dialog's top border (the `╭` line).
const HEADER_REACH: usize = 20;

/// Detect a pending interactive prompt in an agent's recent pane text and summarize it
/// (`"Bash command — Do you want to proceed?"`). Returns `None` for ordinary output, for
/// numbered lists without a selection cursor, and for the usage-limit menu (which is a
/// rate-limit pause, not a permission ask — see [`super::limit`]).
pub fn detect_pending_prompt(pane: &str) -> Option<String> {
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
            return summarize(&stripped, &cleaned, start);
        }
        end = start; // not a menu — keep scanning the lines above
    }
    None
}

/// Build the summary for a menu starting at line `menu_start`: the question line just above
/// it, prefixed with the dialog's header (the first content line under the box's `╭` border).
fn summarize(stripped: &[String], cleaned: &[String], menu_start: usize) -> Option<String> {
    let q_idx = (menu_start.saturating_sub(QUESTION_REACH)..menu_start)
        .rev()
        .find(|&i| is_question(&cleaned[i]))?;
    let question = cleaned[q_idx].trim().to_string();

    // Walk up to the dialog's top border; the first content line below it names the tool
    // ("Bash command", "Edit file", …). Boxless dialogs simply get no header.
    let header = (q_idx.saturating_sub(HEADER_REACH)..q_idx)
        .rev()
        .find(|&i| stripped[i].trim_start().starts_with('╭'))
        .and_then(|b| {
            (b + 1..q_idx)
                .map(|i| cleaned[i].trim())
                .find(|l| !l.is_empty())
        })
        .filter(|h| *h != question);

    let summary = match header {
        Some(h) => format!("{h} — {question}"),
        None => question,
    };
    Some(truncate(&summary, 120))
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
}
