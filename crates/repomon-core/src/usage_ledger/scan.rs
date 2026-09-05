//! Readers that turn an agent's on-disk record of a turn into ledger events.
//!
//! Every reader is a pure function over one source file: it takes a byte (or timestamp) offset,
//! returns the events at or after it, and returns the offset to resume from. Nothing here writes,
//! and nothing here reaches for the fleet: attribution to a repo and a lane happens afterwards,
//! in [`super::FleetIndex`], so the readers stay testable against fixture files alone.
//!
//! Record shapes, as found on disk:
//!
//! - **Claude Code**: `<base>/projects/<encoded cwd>/<session>.jsonl`, one JSON object per line.
//!   A `type: "assistant"` line carries `message.model` and `message.usage` with
//!   `input_tokens`, `output_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`
//!   and `output_tokens_details.thinking_tokens`. `input_tokens` excludes the cached tokens.
//!   Turns whose model is `<synthetic>` are API errors the CLI injected, not billable work.
//! - **Codex**: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`. A `session_meta` header carries
//!   the session id and cwd; each `turn_context` sets the model for the turns that follow; an
//!   `event_msg` of type `token_count` carries `info.last_token_usage` (the delta for the turn
//!   just finished) beside `info.total_token_usage` (the running total). `input_tokens` there
//!   *includes* `cached_input_tokens`, so the reader subtracts to keep the two apart.
//! - **Antigravity**: `~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/transcript.jsonl`.
//!   The conversation store is protobuf and carries no token counts anywhere, so this reader
//!   estimates from content length at [`CHARS_PER_TOKEN`] characters per token and marks every
//!   row estimated.
//! - **OpenCode**: `~/.local/share/opencode/opencode.db`, read-only. An assistant row's `data`
//!   JSON carries `modelID`, `path.cwd` and `tokens` with `input`, `output`, `reasoning` and
//!   `cache.{read,write}`; `input` there excludes the cached tokens.

use std::path::Path;

use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::pricing::TokenCounts;

/// The characters-per-token ratio used when a source keeps no counts of its own.
pub const CHARS_PER_TOKEN: u64 = 4;

/// One billable turn, before fleet attribution.
#[derive(Debug, Clone, PartialEq)]
pub struct ScannedEvent {
    pub at: DateTime<Utc>,
    /// The [`crate::model::AgentKind`] wire string.
    pub agent_kind: String,
    pub model: String,
    /// Which account produced it, matching `agent::claude::account_key` for Claude.
    pub account: String,
    pub session_id: Option<String>,
    /// The working directory the turn ran in, used to attribute it to a repo and lane.
    pub cwd: Option<String>,
    pub tokens: TokenCounts,
    /// Thinking tokens, already counted inside `tokens.output` where the provider bills them so.
    pub thinking_tokens: u64,
    /// Whether the counts were estimated rather than reported.
    pub estimated: bool,
    pub source_path: String,
    /// Byte offset of the record inside `source_path`, or a timestamp watermark for databases.
    pub source_offset: i64,
}

/// What one session looked like, for the sessions table's headline and counters.
#[derive(Debug, Clone, PartialEq)]
pub struct ScannedSession {
    pub session_id: String,
    pub agent_kind: String,
    /// The first user prompt, trimmed to one line.
    pub headline: Option<String>,
    pub cwd: Option<String>,
    pub turns: u32,
    pub tool_calls: u32,
    /// Turns the provider or CLI had to retry: synthetic error turns in a Claude transcript.
    pub retries: u32,
    pub first_at: Option<DateTime<Utc>>,
    pub last_at: Option<DateTime<Utc>>,
}

/// The result of reading one source from an offset.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceScan {
    pub events: Vec<ScannedEvent>,
    pub sessions: Vec<ScannedSession>,
    /// Where a later scan of the same source should resume.
    pub next_offset: u64,
}

fn u64_at(v: Option<&Value>, key: &str) -> u64 {
    v.and_then(|v| v.get(key))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn str_at(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn parse_at(v: &Value, key: &str) -> Option<DateTime<Utc>> {
    let raw = v.get(key)?.as_str()?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Read `path` from `from_offset`, handing each line and its byte offset to `on_line`. Returns
/// the offset just past the last complete line, so a partially written tail is re-read next time.
fn for_each_line<F>(path: &Path, from_offset: u64, mut on_line: F) -> Result<u64>
where
    F: FnMut(&Value, i64),
{
    let text = std::fs::read_to_string(path)?;
    let bytes = text.as_bytes();
    let start = (from_offset as usize).min(bytes.len());
    let mut at = start;
    let mut consumed = start;
    while at < bytes.len() {
        let end = match bytes[at..].iter().position(|b| *b == b'\n') {
            Some(rel) => at + rel,
            // A line without a trailing newline is still being written; stop before it.
            None => break,
        };
        let line = &text[at..end];
        if !line.trim().is_empty() {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                on_line(&v, at as i64);
            }
        }
        at = end + 1;
        consumed = at;
    }
    Ok(consumed as u64)
}

/// Read a Claude Code transcript from `from_offset`. `account` is the account key the transcript
/// root belongs to, or `None` for the default account.
pub fn scan_claude_transcript(
    path: &Path,
    from_offset: u64,
    account: Option<&str>,
) -> Result<SourceScan> {
    let source_path = path.to_string_lossy().to_string();
    let account = account.unwrap_or("default").to_string();
    let mut events = Vec::new();
    let mut session: Option<ScannedSession> = None;
    let mut headline: Option<String> = None;

    let next_offset = for_each_line(path, from_offset, |v, offset| {
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        let cwd = str_at(v, "cwd");
        let session_id = str_at(v, "sessionId").or_else(|| str_at(v, "session_id"));
        let at = parse_at(v, "timestamp");
        if kind == "user" && headline.is_none() {
            headline = v
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(Value::as_str)
                .map(first_line);
        }
        if kind != "assistant" {
            return;
        }
        let message = match v.get("message") {
            Some(m) => m,
            None => return,
        };
        let model = message
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let synthetic = model.is_empty() || model == "<synthetic>";
        let tool_calls = message
            .get("content")
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
                    .count() as u32
            })
            .unwrap_or(0);

        if let Some(id) = session_id.clone() {
            let s = session.get_or_insert_with(|| ScannedSession {
                session_id: id,
                agent_kind: "claude-code".to_string(),
                headline: None,
                cwd: cwd.clone(),
                turns: 0,
                tool_calls: 0,
                retries: 0,
                first_at: at,
                last_at: at,
            });
            if synthetic {
                s.retries += 1;
            } else {
                s.turns += 1;
                s.tool_calls += tool_calls;
            }
            if at.is_some() {
                if s.first_at.is_none() {
                    s.first_at = at;
                }
                s.last_at = at;
            }
        }
        if synthetic {
            return;
        }
        let usage = match message.get("usage") {
            Some(u) => u,
            None => return,
        };
        let at = match at {
            Some(at) => at,
            None => return,
        };
        events.push(ScannedEvent {
            at,
            agent_kind: "claude-code".to_string(),
            model,
            account: account.clone(),
            session_id,
            cwd,
            tokens: TokenCounts {
                input: u64_at(Some(usage), "input_tokens"),
                output: u64_at(Some(usage), "output_tokens"),
                cache_read: u64_at(Some(usage), "cache_read_input_tokens"),
                cache_write: u64_at(Some(usage), "cache_creation_input_tokens"),
            },
            thinking_tokens: u64_at(usage.get("output_tokens_details"), "thinking_tokens"),
            estimated: false,
            source_path: source_path.clone(),
            source_offset: offset,
        });
    })?;

    if let Some(s) = session.as_mut() {
        s.headline = headline;
    }
    Ok(SourceScan {
        events,
        sessions: session.into_iter().collect(),
        next_offset,
    })
}

/// Read a Codex rollout from `from_offset`.
pub fn scan_codex_rollout(path: &Path, from_offset: u64) -> Result<SourceScan> {
    let source_path = path.to_string_lossy().to_string();
    let mut events = Vec::new();
    let mut session_id: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut model = String::new();
    let mut headline: Option<String> = None;
    let mut turns = 0u32;
    let mut first_at: Option<DateTime<Utc>> = None;
    let mut last_at: Option<DateTime<Utc>> = None;

    let next_offset = for_each_line(path, from_offset, |v, offset| {
        let payload = v.get("payload");
        let at = parse_at(v, "timestamp");
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "session_meta" => {
                if let Some(p) = payload {
                    session_id = str_at(p, "session_id");
                    cwd = str_at(p, "cwd");
                }
            }
            "turn_context" => {
                if let Some(p) = payload {
                    if let Some(m) = p.get("model").and_then(Value::as_str) {
                        model = m.to_string();
                    }
                    if let Some(c) = str_at(p, "cwd") {
                        cwd = Some(c);
                    }
                }
            }
            "event_msg" => {
                let p = match payload {
                    Some(p) => p,
                    None => return,
                };
                match p.get("type").and_then(Value::as_str).unwrap_or("") {
                    "user_message" if headline.is_none() => {
                        headline = p.get("message").and_then(Value::as_str).map(first_line);
                    }
                    "token_count" => {
                        let last = match p.get("info").and_then(|i| i.get("last_token_usage")) {
                            Some(l) => l,
                            None => return,
                        };
                        let at = match at {
                            Some(at) => at,
                            None => return,
                        };
                        let cached = u64_at(Some(last), "cached_input_tokens");
                        let input = u64_at(Some(last), "input_tokens").saturating_sub(cached);
                        let output = u64_at(Some(last), "output_tokens");
                        if input == 0 && output == 0 && cached == 0 {
                            return;
                        }
                        turns += 1;
                        if first_at.is_none() {
                            first_at = Some(at);
                        }
                        last_at = Some(at);
                        events.push(ScannedEvent {
                            at,
                            agent_kind: "codex".to_string(),
                            model: model.clone(),
                            account: "codex".to_string(),
                            session_id: session_id.clone(),
                            cwd: cwd.clone(),
                            tokens: TokenCounts {
                                input,
                                output,
                                cache_read: cached,
                                cache_write: u64_at(Some(last), "cache_write_input_tokens"),
                            },
                            thinking_tokens: u64_at(Some(last), "reasoning_output_tokens"),
                            estimated: false,
                            source_path: source_path.clone(),
                            source_offset: offset,
                        });
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    })?;

    let sessions = session_id
        .map(|id| ScannedSession {
            session_id: id,
            agent_kind: "codex".to_string(),
            headline,
            cwd,
            turns,
            // The rollout logs no tool-call or retry events the ledger can count.
            tool_calls: 0,
            retries: 0,
            first_at,
            last_at,
        })
        .into_iter()
        .collect();
    Ok(SourceScan {
        events,
        sessions,
        next_offset,
    })
}

/// Read an Antigravity brain transcript from `from_offset`, estimating every count.
pub fn scan_antigravity_transcript(
    path: &Path,
    from_offset: u64,
    model: &str,
    cwd: Option<&str>,
) -> Result<SourceScan> {
    let source_path = path.to_string_lossy().to_string();
    let session_id = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .map(|s| s.to_string_lossy().to_string());
    let mut events = Vec::new();
    let mut headline: Option<String> = None;
    let mut turns = 0u32;
    let mut tool_calls = 0u32;
    let mut first_at: Option<DateTime<Utc>> = None;
    let mut last_at: Option<DateTime<Utc>> = None;

    let next_offset = for_each_line(path, from_offset, |v, offset| {
        let at = match parse_at(v, "created_at") {
            Some(at) => at,
            None => return,
        };
        let content = v.get("content").and_then(Value::as_str).unwrap_or("");
        let thinking = v.get("thinking").and_then(Value::as_str).unwrap_or("");
        let tools = v
            .get("tool_calls")
            .map(|t| serde_json::to_string(t).unwrap_or_default())
            .unwrap_or_default();
        if let Some(list) = v.get("tool_calls").and_then(Value::as_array) {
            tool_calls += list.len() as u32;
        }
        let from_model = v.get("source").and_then(Value::as_str) == Some("MODEL");
        let chars = (content.len() + thinking.len() + tools.len()) as u64;
        if chars == 0 {
            return;
        }
        let estimate = chars / CHARS_PER_TOKEN;
        if !from_model && headline.is_none() {
            headline = Some(first_line(content));
        }
        if from_model {
            turns += 1;
        }
        if first_at.is_none() {
            first_at = Some(at);
        }
        last_at = Some(at);
        events.push(ScannedEvent {
            at,
            agent_kind: "antigravity".to_string(),
            model: model.to_string(),
            account: "antigravity".to_string(),
            session_id: session_id.clone(),
            cwd: cwd.map(str::to_string),
            tokens: TokenCounts {
                input: if from_model { 0 } else { estimate },
                output: if from_model { estimate } else { 0 },
                cache_read: 0,
                cache_write: 0,
            },
            thinking_tokens: thinking.len() as u64 / CHARS_PER_TOKEN,
            estimated: true,
            source_path: source_path.clone(),
            source_offset: offset,
        });
    })?;

    let sessions = session_id
        .map(|id| ScannedSession {
            session_id: id,
            agent_kind: "antigravity".to_string(),
            headline,
            cwd: cwd.map(str::to_string),
            turns,
            tool_calls,
            retries: 0,
            first_at,
            last_at,
        })
        .into_iter()
        .collect();
    Ok(SourceScan {
        events,
        sessions,
        next_offset,
    })
}

/// Read OpenCode's SQLite store for assistant messages created after `after_ms`, a millisecond
/// epoch watermark that doubles as this source's ingest offset.
pub fn scan_opencode_db(path: &Path, after_ms: u64) -> Result<SourceScan> {
    use rusqlite::OpenFlags;
    let source_path = path.to_string_lossy().to_string();
    let conn = rusqlite::Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut stmt = conn.prepare(
        "SELECT m.time_created, m.data, m.session_id, s.directory, s.title
         FROM message m LEFT JOIN session s ON s.id = m.session_id
         WHERE m.time_created > ?1
         ORDER BY m.time_created ASC",
    )?;
    let rows = stmt.query_map([after_ms as i64], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;

    let mut events = Vec::new();
    let mut sessions: std::collections::BTreeMap<String, ScannedSession> = Default::default();
    let mut watermark = after_ms;
    for row in rows {
        let (created_ms, data, session_id, directory, title) = row?;
        watermark = watermark.max(created_ms.max(0) as u64);
        let v: Value = match serde_json::from_str(&data) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let tokens = match v.get("tokens") {
            Some(t) => t,
            None => continue,
        };
        let at = match Utc.timestamp_millis_opt(created_ms).single() {
            Some(at) => at,
            None => continue,
        };
        let cwd = v
            .get("path")
            .and_then(|p| p.get("cwd"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or(directory);
        if let Some(id) = session_id.clone() {
            let s = sessions
                .entry(id.clone())
                .or_insert_with(|| ScannedSession {
                    session_id: id,
                    agent_kind: "opencode".to_string(),
                    headline: title,
                    cwd: cwd.clone(),
                    turns: 0,
                    tool_calls: 0,
                    retries: 0,
                    first_at: Some(at),
                    last_at: Some(at),
                });
            s.turns += 1;
            s.last_at = Some(at);
        }
        let cache = tokens.get("cache");
        events.push(ScannedEvent {
            at,
            agent_kind: "opencode".to_string(),
            model: v
                .get("modelID")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            account: "opencode".to_string(),
            session_id,
            cwd,
            tokens: TokenCounts {
                input: u64_at(Some(tokens), "input"),
                output: u64_at(Some(tokens), "output"),
                cache_read: u64_at(cache, "read"),
                cache_write: u64_at(cache, "write"),
            },
            thinking_tokens: u64_at(Some(tokens), "reasoning"),
            estimated: false,
            source_path: source_path.clone(),
            source_offset: created_ms,
        });
    }
    Ok(SourceScan {
        events,
        sessions: sessions.into_values().collect(),
        next_offset: watermark,
    })
}

/// The first non-empty line of `s`, trimmed and capped so a headline stays one line.
fn first_line(s: &str) -> String {
    let line = s
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string();
    if line.chars().count() > 160 {
        line.chars().take(157).collect::<String>() + "..."
    } else {
        line
    }
}

/// Reject a source path that is not a readable file, so a scan error is distinguishable from an
/// empty read.
pub fn require_readable(path: &Path) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(Error::NotFound(format!(
            "usage source {} is not a file",
            path.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn claude_scan_reads_usage_model_and_session_from_each_assistant_turn() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            dir.path(),
            "s.jsonl",
            include_str!("fixtures/claude_usage_v0.jsonl"),
        );
        let scan = scan_claude_transcript(&p, 0, None).unwrap();
        assert_eq!(
            scan.events.len(),
            3,
            "three billable assistant turns, synthetic excluded"
        );
        let first = &scan.events[0];
        assert_eq!(first.agent_kind, "claude-code");
        assert_eq!(first.model, "claude-sonnet-5");
        assert_eq!(first.session_id.as_deref(), Some("sess-claude-1"));
        assert_eq!(first.cwd.as_deref(), Some("/repos/demo"));
        assert_eq!(first.tokens.input, 2);
        assert_eq!(first.tokens.output, 611);
        assert_eq!(first.tokens.cache_write, 26034);
        assert_eq!(first.tokens.cache_read, 30449);
        assert!(!first.estimated);
    }

    #[test]
    fn claude_scan_reports_a_session_headline_turns_tool_calls_and_retries() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            dir.path(),
            "s.jsonl",
            include_str!("fixtures/claude_usage_v0.jsonl"),
        );
        let scan = scan_claude_transcript(&p, 0, None).unwrap();
        let s = scan.sessions.first().expect("one session");
        assert_eq!(s.session_id, "sess-claude-1");
        assert_eq!(s.headline.as_deref(), Some("Wire up the ledger"));
        assert_eq!(s.tool_calls, 1);
        assert_eq!(
            s.retries, 1,
            "the synthetic API error turn counts as a retry"
        );
        assert_eq!(s.turns, 3);
    }

    #[test]
    fn claude_scan_resumes_from_a_byte_offset_and_skips_what_it_already_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            dir.path(),
            "s.jsonl",
            include_str!("fixtures/claude_usage_v0.jsonl"),
        );
        let first = scan_claude_transcript(&p, 0, None).unwrap();
        let again = scan_claude_transcript(&p, first.next_offset, None).unwrap();
        assert!(again.events.is_empty());
        assert_eq!(again.next_offset, first.next_offset);
    }

    #[test]
    fn claude_scan_offsets_are_the_byte_position_of_each_line() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            dir.path(),
            "s.jsonl",
            include_str!("fixtures/claude_usage_v0.jsonl"),
        );
        let scan = scan_claude_transcript(&p, 0, None).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        for e in &scan.events {
            let at = e.source_offset as usize;
            assert!(
                text[at..].starts_with('{'),
                "offset {at} must start a record"
            );
        }
        let offsets: Vec<i64> = scan.events.iter().map(|e| e.source_offset).collect();
        let mut sorted = offsets.clone();
        sorted.dedup();
        assert_eq!(offsets, sorted, "offsets are distinct and ascending");
    }

    #[test]
    fn claude_scan_tags_the_account_it_was_read_from() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            dir.path(),
            "s.jsonl",
            include_str!("fixtures/claude_usage_v0.jsonl"),
        );
        let scan = scan_claude_transcript(&p, 0, Some("work")).unwrap();
        assert!(scan.events.iter().all(|e| e.account == "work"));
    }

    #[test]
    fn codex_scan_charges_the_per_turn_delta_not_the_running_total() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            dir.path(),
            "r.jsonl",
            include_str!("fixtures/codex_usage_v0.jsonl"),
        );
        let scan = scan_codex_rollout(&p, 0).unwrap();
        assert_eq!(scan.events.len(), 2);
        let e = &scan.events[0];
        assert_eq!(e.agent_kind, "codex");
        assert_eq!(e.model, "gpt-5.6-sol");
        assert_eq!(e.session_id.as_deref(), Some("sess-codex-1"));
        assert_eq!(e.cwd.as_deref(), Some("/repos/demo"));
        // The rollout reports 20210 input of which 4864 was cached; the ledger stores them apart.
        assert_eq!(e.tokens.input, 20210 - 4864);
        assert_eq!(e.tokens.cache_read, 4864);
        assert_eq!(e.tokens.output, 231);
        assert_eq!(e.thinking_tokens, 172);
        assert!(!e.estimated);
    }

    #[test]
    fn antigravity_scan_estimates_tokens_from_content_length_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            dir.path(),
            "t.jsonl",
            include_str!("fixtures/antigravity_usage_v0.jsonl"),
        );
        let scan =
            scan_antigravity_transcript(&p, 0, "gemini-3-flash", Some("/repos/demo")).unwrap();
        assert_eq!(scan.events.len(), 3);
        assert!(
            scan.events.iter().all(|e| e.estimated),
            "the store keeps no counts"
        );
        assert_eq!(scan.events[0].model, "gemini-3-flash");
        assert_eq!(scan.events[0].cwd.as_deref(), Some("/repos/demo"));
        // The user step is 400 characters of content, charged as input at four chars per token.
        assert_eq!(scan.events[0].tokens.input, 400 / CHARS_PER_TOKEN);
        assert_eq!(scan.events[0].tokens.output, 0);
        // The model step is 200 characters of thinking plus 800 of content, charged as output.
        assert_eq!(scan.events[1].tokens.output, (200 + 800) / CHARS_PER_TOKEN);
        assert_eq!(scan.events[1].thinking_tokens, 200 / CHARS_PER_TOKEN);
        assert_eq!(scan.events[1].tokens.input, 0);
        // A tool-call step is charged on the serialized arguments it emitted.
        assert!(scan.events[2].tokens.output > 0);
    }

    #[test]
    fn opencode_scan_reads_assistant_token_counts_after_a_watermark() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        seed_opencode_fixture(&db);
        let scan = scan_opencode_db(&db, 0).unwrap();
        assert_eq!(scan.events.len(), 2);
        let e = &scan.events[0];
        assert_eq!(e.agent_kind, "opencode");
        assert_eq!(e.model, "kimi-k2-thinking");
        assert_eq!(e.tokens.input, 10066);
        assert_eq!(e.tokens.output, 18);
        assert_eq!(e.tokens.cache_read, 256);
        assert_eq!(e.cwd.as_deref(), Some("/repos/demo"));
        let after = scan_opencode_db(&db, scan.next_offset).unwrap();
        assert!(
            after.events.is_empty(),
            "the watermark makes a re-read idempotent"
        );
    }

    fn seed_opencode_fixture(path: &std::path::Path) {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session(id TEXT PRIMARY KEY, directory TEXT, title TEXT);
             CREATE TABLE message(id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
             INSERT INTO session VALUES('sess-oc-1', '/repos/demo', 'Ledger work');",
        )
        .unwrap();
        let row = |id: &str, at: i64, out: i64| {
            format!(
                r#"INSERT INTO message VALUES('{id}','sess-oc-1',{at},'{{"role":"assistant","modelID":"kimi-k2-thinking","providerID":"kimi-for-coding","path":{{"cwd":"/repos/demo"}},"tokens":{{"input":10066,"output":{out},"reasoning":0,"cache":{{"read":256,"write":0}}}}}}');"#
            )
        };
        conn.execute_batch(&row("m1", 1_788_000_000_000, 18))
            .unwrap();
        conn.execute_batch(&row("m2", 1_788_000_060_000, 40))
            .unwrap();
        conn.execute_batch(
            r#"INSERT INTO message VALUES('m0','sess-oc-1',1788000000000,'{"role":"user"}');"#,
        )
        .unwrap();
    }
}
