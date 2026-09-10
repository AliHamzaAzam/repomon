//! Recognize Codex's machine summaries and numbered diff gutters without classifying prose by topic.
use crate::model::{ToolCallStatus, TranscriptItem};

fn counted(words: &[&str], action: &str, singular: &str, plural: &str) -> bool {
    matches!(words, [verb, n, unit] if verb.eq_ignore_ascii_case(action) && n.parse::<u32>().is_ok() && (*unit == singular || *unit == plural))
}
pub fn tool_rollup(text: &str) -> bool {
    if !text.split_whitespace().next().is_some_and(|word| {
        ["ran", "searched", "used"]
            .iter()
            .any(|verb| word.eq_ignore_ascii_case(verb))
    }) {
        return false;
    }
    let clauses: Vec<_> = text.trim().split(", ").collect();
    !clauses.is_empty()
        && clauses.iter().all(|clause| {
            let words: Vec<_> = clause.split_whitespace().collect();
            match words.as_slice() {
                [action, "for", n, unit] if action.eq_ignore_ascii_case("searched") => {
                    counted(&["searched", n, unit], "searched", "pattern", "patterns")
                }
                [action, n, "shell", unit] => {
                    counted(&[action, n, unit], "ran", "command", "commands")
                }
                words => counted(words, "used", "tool", "tools"),
            }
        })
}
fn numbered_diff_line(line: &str) -> bool {
    let mut words = line.split_whitespace();
    words.next().is_some_and(|n| n.parse::<u32>().is_ok())
        && words
            .next()
            .is_some_and(|marker| matches!(marker, "+" | "-"))
}
pub fn numbered_diff(text: &str) -> bool {
    if text.contains("```") {
        return false;
    }
    let lines: Vec<_> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let count = lines.iter().filter(|line| numbered_diff_line(line)).count();
    count >= 2 && count * 2 >= lines.len()
}
pub fn tool_item(text: &str) -> Option<TranscriptItem> {
    let text = text.trim();
    // Ordinary prose cannot have either grammar. Avoid several scans/allocations of every
    // assistant answer just to reject it, especially on a cold conversation page.
    if !text
        .as_bytes()
        .first()
        .is_some_and(|c| c.is_ascii_digit() || matches!(c, b'R' | b'r' | b'S' | b's' | b'U' | b'u'))
    {
        return None;
    }
    let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
    let rollup = tool_rollup(first);
    let diff = numbered_diff(text);
    if !(diff || rollup && (rest.trim().is_empty() || numbered_diff(rest))) {
        return None;
    }
    let mut item = TranscriptItem::new("tool_call", if rollup { first } else { "Tool diff" }, None);
    item.name = Some(
        if rollup {
            "tool_summary"
        } else {
            "apply_patch"
        }
        .into(),
    );
    item.status = Some(ToolCallStatus::Ok);
    if rollup {
        item.input_summary = Some(first.into());
    }
    if diff {
        item.result_summary = Some(if rollup { rest } else { text }.into());
        item.diff = item.result_summary.clone();
    }
    Some(item)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn summaries_match_counts_and_grammar_and_leave_sentences_alone() {
        for text in [
            "Ran 1 shell command",
            "Ran 17 shell commands",
            "Searched for 1 pattern, ran 2 shell commands",
            "Searched for 2 patterns, ran 7 shell commands",
            "Used 2 tools",
        ] {
            assert!(tool_rollup(text), "{text}");
            assert_eq!(tool_item(text).unwrap().kind.as_deref(), Some("tool_call"));
        }
        for text in [
            "I ran 2 shell commands",
            "Ran 2 shell commands to reproduce the bug.",
            "Searched for the missing pattern",
            "Used these tools",
            "1 + one example",
            "```diff\n205 + code\n206 + more\n```",
        ] {
            assert!(tool_item(text).is_none(), "{text}");
        }
    }
}
