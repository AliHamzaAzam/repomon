//! Structured provider chrome. Values describe only what is visible in the pane, never estimates.
use super::text::strip_ansi;
use serde::Serialize;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Activity {
    pub verb: Option<String>,
    pub elapsed_seconds: Option<f64>,
    pub token_count: Option<u64>,
    pub thought_seconds: Option<f64>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

fn duration(text: &str) -> Option<f64> {
    let mut sum = 0.0;
    let mut found = false;
    for word in text.split_whitespace() {
        let (number, factor) = [('h', 3600.0), ('m', 60.0), ('s', 1.0)]
            .into_iter()
            .find_map(|(unit, factor)| word.strip_suffix(unit).map(|number| (number, factor)))?;
        let n: f64 = number.parse().ok()?;
        if !n.is_finite() || n < 0.0 {
            return None;
        }
        sum += n * factor;
        found = true;
    }
    (found && sum.is_finite()).then_some(sum)
}

fn tokens(text: &str) -> Option<u64> {
    let text = text
        .trim()
        .strip_suffix(" tokens")?
        .trim()
        .trim_start_matches(['↑', '↓'])
        .trim();
    let (number, factor) = match text.chars().last()? {
        'k' | 'K' => (&text[..text.len() - 1], 1000.0),
        'm' | 'M' => (&text[..text.len() - 1], 1_000_000.0),
        _ => (text, 1.0),
    };
    let value = number.replace(',', "").parse::<f64>().ok()? * factor;
    (value.is_finite() && value >= 0.0 && value < u64::MAX as f64).then(|| value.round() as u64)
}

pub(super) fn timed_line(kind: &str, line: &str) -> Option<Activity> {
    let line = line.trim();
    // Recognize the timer/counter grammar independently of the CLI's rotating frame set.
    // An optional leading symbol is decoration; alphabetic prose remains part of the verb.
    let body = line
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .trim();
    let (verb, counters) = body.split_once(" (")?;
    let counters = counters.strip_suffix(')')?;
    let mut activity = Activity::default();
    if kind == "claude-code" {
        let verb = verb
            .strip_suffix("...")
            .or_else(|| verb.strip_suffix('…'))?;
        if verb.is_empty()
            || !verb
                .chars()
                .all(|c| c.is_alphabetic() || c == ' ' || c == '-')
        {
            return None;
        }
        // A timer plus token counter distinguishes a rotating activity label from prose.
        let normalized_counters = counters.replace(" · ", ", ").replace(" • ", ", ");
        let mut fields = normalized_counters.split(", ");
        activity.elapsed_seconds = Some(duration(fields.next()?)?);
        activity.token_count = Some(tokens(fields.next()?)?);
        for field in fields {
            activity.thought_seconds = field
                .strip_prefix("thought for ")
                .and_then(duration)
                .or(activity.thought_seconds);
        }
        activity.verb = Some(verb.into());
    } else if kind == "codex" {
        let (elapsed, hint) = counters
            .split_once('•')
            .or_else(|| counters.split_once('·'))?;
        if !hint.trim().starts_with("esc to interrupt") || verb.is_empty() {
            return None;
        }
        activity.elapsed_seconds = Some(duration(elapsed.trim())?);
        activity.verb = Some(verb.trim_end_matches('.').trim_end_matches('…').into());
    } else {
        return None;
    }
    Some(activity)
}

pub(super) fn model_line(line: &str) -> Option<(String, String)> {
    let fields: Vec<_> = line.split('·').map(str::trim).collect();
    if !fields.iter().any(|f| {
        f.starts_with("~/")
            || f.starts_with('/')
            || (f.as_bytes().get(1) == Some(&b':')
                && f.as_bytes()
                    .get(2)
                    .is_some_and(|c| matches!(c, b'/' | b'\\')))
    }) {
        return None;
    }
    fields.iter().find_map(|field| {
        let words: Vec<_> = field.split_whitespace().collect();
        match words.as_slice() {
            [name, effort]
                if name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                    && matches!(
                        *effort,
                        "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
                    ) =>
            {
                Some(((*name).into(), (*effort).into()))
            }
            _ => None,
        }
    })
}

pub fn pane_activity(kind: &str, pane: &str) -> Option<Activity> {
    let plain = strip_ansi(pane);
    let mut activity = None;
    let mut model = None;
    for line in plain.lines() {
        if let Some(value) = timed_line(kind, line) {
            activity = Some(value);
        }
        if kind == "codex" {
            model = model_line(line).or(model);
        }
    }
    if let Some((model, effort)) = model {
        let value = activity.get_or_insert_with(Activity::default);
        value.model = Some(model);
        value.effort = Some(effort);
    }
    activity
}

/// Anchored CLI labels, not prose discussing queues. Counts alone never invent message text.
pub fn queue_indicator(kind: &str, pane: &str) -> bool {
    strip_ansi(pane).lines().any(|line| {
        let line = line.trim().trim_start_matches(['•', '●', '⏺', '›', '❯']).trim();
        match kind {
            "codex" => line.strip_prefix("Queued follow-up inputs,").is_some_and(|tail| {
                let words: Vec<_> = tail.split_whitespace().collect();
                matches!(words.as_slice(), [n, "question" | "questions"] if n.parse::<u32>().is_ok())
            }),
            "claude-code" => line == "Press up to edit queued messages" || line.strip_prefix("Queued message").is_some_and(|tail| tail.is_empty() || tail.starts_with(':'))
                || line.strip_suffix(" queued messages").or_else(|| line.strip_suffix(" queued message")).is_some_and(|n| n.parse::<u32>().is_ok()),
            _ => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::conversation::{pane_content, pane_items};
    #[test]
    fn activity_grammar_handles_unknown_frames_and_middot_without_leaking_chrome() {
        let pane = include_str!("fixtures/claude_activity_middot_2026_09_11.txt");
        for frame in ["·", "¤", "⟳", "", "*"] {
            let pane = pane.replace("· Drizzling", &format!("{frame} Drizzling"));
            let activity = pane_activity("claude-code", &pane).unwrap();
            assert_eq!(activity.verb.as_deref(), Some("Drizzling"));
            assert_eq!(activity.elapsed_seconds, Some(47.0));
            assert_eq!(activity.token_count, Some(2700));
            assert_eq!(activity.thought_seconds, Some(15.0));
            assert!(!pane_content("claude-code", &pane).contains("Drizzling"));
            let rows = pane_items("claude-code", &pane);
            assert!(
                rows.iter()
                    .any(|r| r.text.contains("review is in progress"))
            );
            assert!(rows.iter().all(|r| !r.text.contains("Drizzling")));
        }
    }
    #[test]
    fn real_claude_pane_and_operator_status_extract_counters_without_inline_chrome() {
        // Existing captured Claude pane, with the operator-transcribed active footer replacing
        // its completed footer. We did not capture another worker's live session.
        let captured = include_str!("fixtures/claude_idle_done_spinner.txt");
        let observed = include_str!("fixtures/claude_activity_operator_2026_09_10.txt").trim();
        for (line, verb, elapsed, count, thought) in [
            (observed.to_string(), "Vibing", 1362.0, 100100, 1.0),
            (
                observed
                    .replace("Vibing", "Percolating")
                    .replace("22m 42s", "3m 7s")
                    .replace("100.1k", "2.4k")
                    .replace("for 1s", "for 12s"),
                "Percolating",
                187.0,
                2400,
                12.0,
            ),
        ] {
            let pane = captured.replace("✻ Crunched for 2m 19s · done 4:52 AM", &line);
            let activity = pane_activity("claude-code", &pane).unwrap();
            assert_eq!(activity.verb.as_deref(), Some(verb));
            assert_eq!(activity.elapsed_seconds, Some(elapsed));
            assert_eq!(activity.token_count, Some(count));
            assert_eq!(activity.thought_seconds, Some(thought));
            assert!(activity.model.is_none() && activity.effort.is_none());
            let rows = pane_items("claude-code", &pane);
            assert!(
                rows.iter()
                    .any(|r| r.text.contains("The models are two-tone now"))
            );
            assert!(rows.iter().any(|r| r.kind.as_deref() == Some("tool_call")));
            assert!(rows.iter().all(|r| !r.text.contains(&line)));
            assert!(!pane_content("claude-code", &pane).contains(&line));
        }
        for prose in [
            "Vibing is a useful description.",
            "Writing...",
            "+ Example... (not a timer, 3 tokens)",
            "+ Example... (文, 3 tokens)",
        ] {
            assert!(pane_activity("claude-code", prose).is_none());
            assert_eq!(pane_items("claude-code", prose)[0].text, prose);
        }
    }
    #[test]
    fn real_codex_pane_exposes_activity_elapsed_model_and_effort() {
        let pane = include_str!("fixtures/codex_working_footer_2026_09_10.ansi");
        let activity = pane_activity("codex", pane).unwrap();
        assert_eq!(activity.verb.as_deref(), Some("Working"));
        assert_eq!(activity.elapsed_seconds, Some(926.0));
        assert_eq!(activity.model.as_deref(), Some("gpt-6-astra"));
        assert_eq!(activity.effort.as_deref(), Some("high"));
        assert!(activity.token_count.is_none() && activity.thought_seconds.is_none());
        assert!(pane_activity("hermes", pane).is_none());
    }
}
