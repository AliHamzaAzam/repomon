//! Chat-mode first-open latency breakdown (round 9). Always on, no flag: cheap, bounded to a
//! handful of entries per chat open, and written to disk so it survives past the moment it
//! happens - the operator will not have devtools open when the 30-second delay recurs. Sits
//! beside the daemon's own logs (`repomon_core::service::log_dir()`) so the two halves can be
//! lined up on their shared wall-clock timestamps; nothing here depends on the daemon's file or
//! its format.
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use repomon_core::service;
use serde::Serialize;

/// Caps the file at a handful of chat-opens' worth of history (roughly 6-7 lines each), not an
/// unbounded trace - this runs forever in the background, so it must never grow without limit.
const MAX_LINES: usize = 500;

fn log_path() -> PathBuf {
    service::log_dir().join("desktop-chat-latency.jsonl")
}

fn now_unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[derive(Serialize)]
struct Entry {
    /// Unix epoch milliseconds, the same clock the daemon's own log uses - the only thing the two
    /// files need to agree on.
    at_ms: u128,
    label: String,
    lane_id: Option<i64>,
    window: Option<String>,
    duration_ms: Option<f64>,
}

/// Appends one bounded diagnostic line for the chat-mode first-open latency breakdown. Never
/// blocks or fails the caller: a diagnostic that can break the thing it measures is worse than no
/// diagnostic, so every error here is swallowed.
#[tauri::command]
pub fn record_chat_latency_event(
    label: String,
    lane_id: Option<i64>,
    window: Option<String>,
    duration_ms: Option<f64>,
) {
    let entry = Entry {
        at_ms: now_unix_ms(),
        label,
        lane_id,
        window,
        duration_ms,
    };
    let Ok(line) = serde_json::to_string(&entry) else {
        return;
    };
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let kept = bounded_lines(&existing, &line, MAX_LINES);
    let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
    else {
        return;
    };
    let _ = file.write_all(kept.as_bytes());
}

/// Appends `new_line` to `existing`'s lines, dropping the oldest as needed to keep at most `max`
/// lines total - a ring buffer, not an unbounded trace.
fn bounded_lines(existing: &str, new_line: &str, max: usize) -> String {
    let mut lines: Vec<&str> = existing.lines().collect();
    lines.push(new_line);
    if lines.len() > max {
        lines.drain(0..lines.len() - max);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn entry_serializes_with_only_the_fields_it_has() {
        let entry = Entry {
            at_ms: 12345,
            label: "chat_clicked".into(),
            lane_id: Some(7),
            window: Some("lane-7".into()),
            duration_ms: None,
        };
        let json: Value = serde_json::from_str(&serde_json::to_string(&entry).unwrap()).unwrap();
        assert_eq!(json["at_ms"], 12345);
        assert_eq!(json["label"], "chat_clicked");
        assert_eq!(json["lane_id"], 7);
        assert_eq!(json["window"], "lane-7");
        assert!(json["duration_ms"].is_null());
    }

    #[test]
    fn bounded_lines_keeps_at_most_max_dropping_the_oldest_first() {
        let existing = (0..5)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let result = bounded_lines(&existing, "line5", 3);
        assert_eq!(result, "line3\nline4\nline5\n");
    }

    #[test]
    fn bounded_lines_appends_without_dropping_when_under_the_cap() {
        let result = bounded_lines("line0\nline1\n", "line2", 10);
        assert_eq!(result, "line0\nline1\nline2\n");
    }

    #[test]
    fn bounded_lines_handles_an_empty_starting_file() {
        let result = bounded_lines("", "first", 500);
        assert_eq!(result, "first\n");
    }
}
