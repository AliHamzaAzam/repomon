//! Queue membership from the provider's prompt region, independent of elapsed time.
use super::{conversation_activity, text::strip_ansi};

#[derive(Default, Debug)]
pub struct PaneInputs {
    pub consumed: Vec<String>,
    pub queued: Vec<String>,
    pub queue_reported: bool,
    queue_start: Option<usize>,
}
pub fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn prompt(line: &str) -> Option<&str> {
    let line = line.trim();
    let rest = line.strip_prefix(['❯', '›', '>'])?;
    rest.starts_with(char::is_whitespace).then(|| rest.trim())
}
fn divider(line: &str) -> bool {
    let line = line.trim();
    line.chars().count() >= 3 && line.chars().all(|c| matches!(c, '─' | '━' | '-'))
}

/// Whether this kind's pane carries a consumed-prompt region we can actually read.
///
/// The pinned queue asserts that the agent has not read something yet. That assertion needs
/// evidence, and the pane is where it comes from. For a kind absent from this list there is no
/// such region to parse, so the claim is not ours to make and the caller must say what it knows
/// instead of guessing. This is the predicate to branch on - never the kind name.
pub fn observable(kind: &str) -> bool {
    matches!(kind, "claude-code" | "codex" | "antigravity" | "opencode")
}

pub fn pane_inputs(kind: &str, pane: &str) -> PaneInputs {
    match kind {
        "claude-code" | "codex" => claude_style_inputs(kind, pane),
        "antigravity" => antigravity_inputs(pane),
        "opencode" => opencode_inputs(pane),
        _ => PaneInputs::default(),
    }
}

/// Antigravity echoes each submitted prompt as a `> ` line above that turn's body, and parks the
/// composer in its own single-line cell between two rules near the foot of the screen. The cell is
/// what separates the two: a `> ` line fenced by a rule above and below is the composer (empty, or
/// carrying a mode hint such as "Accept-edits mode: ..."), and every `> ` line above it has been
/// read. Trailing background-task rows add further rules below the cell, so the composer is found
/// by its fencing rather than by counting rules up from the bottom.
fn antigravity_inputs(pane: &str) -> PaneInputs {
    let plain = strip_ansi(pane);
    let lines: Vec<_> = plain.lines().collect();
    // An idle composer is a bare `>` with nothing after it, which `prompt` rejects for want of the
    // separating space, so the fence tests the marker itself.
    let fenced = |i: usize| {
        i > 0
            && divider(lines[i - 1])
            && lines.get(i + 1).is_some_and(|next| divider(next))
            && lines[i].trim().starts_with('>')
    };
    let composer = (0..lines.len()).rev().find(|&i| fenced(i));
    let mut result = PaneInputs::default();
    for (i, line) in lines.iter().enumerate() {
        if composer.is_some_and(|composer| i >= composer) {
            break;
        }
        if let Some(text) = prompt(line).filter(|text| !text.is_empty()) {
            result.consumed.push(normalized(text));
        }
    }
    result
}

/// OpenCode draws a grid, not a stream: prompts sit in a `┃` gutter, and a right-hand sidebar
/// shares their physical lines, so the gutter text has to be cropped out of the row before it can
/// be compared with anything. `opencode_body` does that crop and also drops the trailing composer
/// block, which uses the same gutter as the prompts and would otherwise read as one.
fn opencode_inputs(pane: &str) -> PaneInputs {
    let plain = strip_ansi(pane);
    let body = super::conversation::opencode_body(&plain);
    let mut result = PaneInputs::default();
    let mut block: Vec<String> = Vec::new();
    for line in body.lines().chain(std::iter::once("")) {
        let trimmed = line.trim();
        match trimmed.strip_prefix('┃') {
            Some(text) if !text.trim().is_empty() => block.push(text.trim().into()),
            Some(_) => {}
            None => {
                if !block.is_empty() {
                    result.consumed.push(normalized(&block.join("\n")));
                    block.clear();
                }
            }
        }
    }
    result
}

fn claude_style_inputs(kind: &str, pane: &str) -> PaneInputs {
    let plain = strip_ansi(pane);
    let lines: Vec<_> = plain.lines().collect();
    let queue_hint = lines.iter().rposition(|line| {
        prompt(line).unwrap_or(line.trim()) == "Press up to edit queued messages"
    });
    let mut result = PaneInputs {
        queue_reported: conversation_activity::queue_indicator(kind, pane),
        ..Default::default()
    };
    // Claude seats queued prompt blocks after the active status and before the composer rule.
    // Earlier prompt blocks are consumed. A footer without that separator is not enough to
    // declare every submitted message queued.
    let queued_start = queue_hint
        .and_then(|hint| {
            (0..hint)
                .rev()
                .find(|&i| conversation_activity::timed_line(kind, lines[i].trim()).is_some())
        })
        .map(|i| i + 1);
    let composer_start = queue_hint
        .and_then(|hint| (0..hint).rev().find(|&i| divider(lines[i])))
        .or_else(|| {
            let mut rules = (0..lines.len()).rev().filter(|&i| divider(lines[i]));
            let last = rules.next()?;
            Some(rules.next().unwrap_or(last))
        });
    result.queue_start = queued_start;
    let mut i = 0;
    while i < lines.len() {
        if composer_start.is_some_and(|start| i >= start) {
            break;
        }
        let Some(text) = prompt(lines[i]).filter(|text| !text.is_empty()) else {
            i += 1;
            continue;
        };
        if text == "Press up to edit queued messages" || text.starts_with("Ask Codex ") {
            i += 1;
            continue;
        }
        let start = i;
        let mut text = text.to_string();
        i += 1;
        while i < lines.len() {
            let next = lines[i];
            if next.trim().is_empty() {
                i += 1;
                continue;
            }
            if prompt(next).is_some() || divider(next) || !next.starts_with(char::is_whitespace) {
                break;
            }
            text.push('\n');
            text.push_str(next.trim());
            i += 1;
        }
        if queued_start.is_some_and(|first| start >= first) {
            result.queued.push(normalized(&text));
        } else {
            result.consumed.push(normalized(&text));
        }
    }
    result
}

/// A queued user block is not assistant output and must not reset the in-flight preview.
pub fn without_queue(kind: &str, pane: &str) -> String {
    let parsed = pane_inputs(kind, pane);
    let plain = strip_ansi(pane);
    plain
        .lines()
        .take(parsed.queue_start.unwrap_or(usize::MAX))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn operator_claude_queue_region_distinguishes_consumed_prompts_and_preserves_wrapped_attachment()
     {
        let pane = include_str!("fixtures/claude_queue_operator_2026_09_10.txt");
        let rows = pane_inputs("claude-code", pane);
        assert!(rows.queue_reported);
        assert_eq!(
            rows.consumed,
            vec!["First message", "Second message", "Third message"]
        );
        assert_eq!(
            rows.queued,
            vec!["Still waiting Attached file: \"/a path/file.png\""]
        );
        let changed = pane
            .replace("Boondoggling", "Percolating")
            .replace("4m 50s", "8s")
            .replace("9.8k", "2.1k");
        assert_eq!(pane_inputs("claude-code", &changed).queued, rows.queued);
        assert!(pane_inputs("hermes", pane).queued.is_empty());
    }

    /// Antigravity's composer is a `> ` cell fenced by two rules, and the prompts above it have
    /// been read. The distinction matters because the composer is not always empty: in
    /// accept-edits mode it carries a hint, and reading that as a prompt the agent had consumed
    /// would retire a ticket on no evidence at all.
    #[test]
    fn antigravity_reads_prompts_above_the_fenced_composer_and_never_the_composer_itself() {
        let working = include_str!("fixtures/antigravity_working_spinner.txt");
        assert_eq!(
            pane_inputs("antigravity", working).consumed,
            vec![
                "List the top-level files in this repository, read README.md, and reply with a two-line summary."
            ]
        );
        let hinted = include_str!("fixtures/antigravity_idle_prose_subagents.txt");
        assert!(
            pane_inputs("antigravity", hinted).consumed.is_empty(),
            "the accept-edits hint sits in the composer, and trailing task rules must not hide it"
        );
        // The operator's reproduction: chat cleared, so the echo that proved consumption is gone
        // and nothing can be read back out of the pane.
        let cleared = include_str!("fixtures/antigravity_cleared_chat_2026_09_12.txt");
        assert!(pane_inputs("antigravity", cleared).consumed.is_empty());
    }

    /// OpenCode shares physical lines between the prompt gutter and a right-hand sidebar, so a
    /// prompt read straight off the capture carries the sidebar's text with it and matches
    /// nothing. Cropping to the conversation column is what makes the comparison possible.
    #[test]
    fn opencode_reads_gutter_prompts_clear_of_the_sidebar_and_of_its_own_composer() {
        let live = include_str!("fixtures/opencode_sidebar_prompt_2026_09_12.txt");
        assert_eq!(
            pane_inputs("opencode", live).consumed,
            vec!["hi"],
            "the session banner shares this row and must not end up in the prompt"
        );
        let turns = include_str!("fixtures/opencode_turns_v0.txt");
        assert_eq!(
            pane_inputs("opencode", turns).consumed,
            vec!["write a poem", "Write a poem", "new", "new"]
        );
    }

    /// The pinned queue claims the agent has not read something. `observable` is what licenses
    /// that claim, and a kind missing from it has no prompt region to read, so a caller must say
    /// what it knows rather than assert an agent is ignoring the operator.
    #[test]
    fn unobservable_kinds_yield_no_evidence_rather_than_a_silent_empty_result() {
        for kind in ["hermes", "aider", "custom"] {
            assert!(!observable(kind));
            assert!(pane_inputs(kind, "> hh\n\n> \n").consumed.is_empty());
        }
        for kind in ["claude-code", "codex", "antigravity", "opencode"] {
            assert!(observable(kind));
        }
    }
}
