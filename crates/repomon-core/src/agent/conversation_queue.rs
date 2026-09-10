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

pub fn pane_inputs(kind: &str, pane: &str) -> PaneInputs {
    if !matches!(kind, "claude-code" | "codex") {
        return PaneInputs::default();
    }
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
}
