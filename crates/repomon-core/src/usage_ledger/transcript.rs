//! Maps records already decoded by the ledger scanners into the conversation contract.
use super::scan::strip_injected_blocks;
use crate::model::{ToolCallStatus, TranscriptItem};
use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptEntry {
    pub offset: u64,
    pub item: TranscriptItem,
}

#[derive(Default)]
pub(super) struct Mapper {
    pub rows: Vec<TranscriptEntry>,
    model: Option<String>,
    // Codex emits both response_item and event_msg copies of prose.
    messages: std::collections::HashMap<String, String>,
}

fn string(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}
fn text(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.into();
    }
    if let Some(a) = v.as_array() {
        return a
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
    }
    v.to_string()
}
fn at(v: &Value) -> Option<DateTime<Utc>> {
    string(v, "timestamp")
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&Utc))
}
fn edit_diff(input: &Value) -> Option<String> {
    let old = input.get("old_string")?.as_str()?;
    let new = input.get("new_string")?.as_str()?;
    let file = input
        .get("file_path")
        .and_then(Value::as_str)
        .unwrap_or("file");
    Some(format!(
        "--- a/{file}\n+++ b/{file}\n@@ -1,{} +1,{} @@\n{}{}",
        old.lines().count(),
        new.lines().count(),
        old.lines().map(|s| format!("-{s}\n")).collect::<String>(),
        new.lines().map(|s| format!("+{s}\n")).collect::<String>()
    ))
}

impl Mapper {
    fn push(&mut self, offset: i64, mut item: TranscriptItem) {
        if item.id.is_none() {
            item.id = Some(format!(
                "{offset}:{}",
                self.rows
                    .iter()
                    .rev()
                    .take_while(|r| r.offset == offset as u64)
                    .count()
            ));
        }
        self.rows.push(TranscriptEntry {
            offset: offset as u64,
            item,
        });
    }
    fn status(&mut self, offset: i64, v: &Value, kind: &str, message: &str) {
        let mut item = TranscriptItem::new("status", message, at(v));
        item.status_kind = Some(kind.into());
        self.push(offset, item);
    }
    fn tool(&mut self, offset: i64, v: &Value, block: &Value) {
        let name = string(block, "name").unwrap_or_else(|| "tool".into());
        let input = block.get("input").or_else(|| block.get("arguments"));
        let decoded = input
            .and_then(Value::as_str)
            .and_then(|s| serde_json::from_str::<Value>(s).ok());
        let input = decoded.as_ref().or(input);
        let summary = input.map(text).unwrap_or_default();
        let mut item = TranscriptItem::new("tool_call", format!("{name} {summary}"), at(v));
        item.id = string(block, "call_id")
            .or_else(|| string(block, "id"))
            .map(|s| format!("tool:{s}"));
        item.name = Some(name);
        item.input_summary = Some(summary);
        item.diff = input.and_then(edit_diff).or_else(|| {
            input
                .and_then(Value::as_str)
                .filter(|s| s.starts_with("--- "))
                .map(str::to_string)
        });
        item.status = Some(ToolCallStatus::Running);
        self.push(offset, item);
    }
    fn result(&mut self, offset: i64, v: &Value, id: Option<String>, output: &Value, error: bool) {
        let id = id.map(|s| format!("tool:{s}"));
        let summary = text(output);
        let status = if error {
            ToolCallStatus::Error
        } else {
            ToolCallStatus::Ok
        };
        if let Some(row) = self
            .rows
            .iter_mut()
            .rev()
            .find(|r| id.is_some() && r.item.id == id)
        {
            row.item.result_summary = Some(summary);
            row.item.status = Some(status);
        } else {
            let mut item = TranscriptItem::new("tool_call", &summary, at(v));
            item.id = id;
            item.result_summary = Some(summary);
            item.status = Some(status);
            self.push(offset, item);
        }
    }
    pub fn claude(&mut self, v: &Value, offset: i64) {
        let kind = v["type"].as_str().unwrap_or("");
        if kind == "unparsed" {
            self.push(
                offset,
                TranscriptItem::new("terminal_block", text(&v["raw"]), at(v)),
            );
            return;
        }
        if let Some(m) = string(&v["message"], "model") {
            self.model = Some(m);
        }
        if matches!(kind, "user" | "assistant") {
            let content = &v["message"]["content"];
            if let Some(s) = content.as_str() {
                self.message(offset, v, kind, s.into(), None);
            } else if let Some(blocks) = content.as_array() {
                // Adjacent user text blocks form one prose segment. Clean them together so a
                // wrapper split across blocks cannot leak its body as another user item.
                let mut user_text = Vec::new();
                for block in blocks {
                    if kind == "user" && block["type"] == "text" {
                        user_text.push(block["text"].as_str().unwrap_or(""));
                        continue;
                    }
                    if !user_text.is_empty() {
                        self.message(offset, v, kind, user_text.join("\n"), None);
                        user_text.clear();
                    }
                    match block["type"].as_str().unwrap_or("") {
                        "text" => {
                            self.message(
                                offset,
                                v,
                                kind,
                                block["text"].as_str().unwrap_or("").into(),
                                None,
                            );
                        }
                        "tool_use" => self.tool(offset, v, block),
                        "tool_result" => self.result(
                            offset,
                            v,
                            string(block, "tool_use_id"),
                            &block["content"],
                            block["is_error"].as_bool().unwrap_or(false),
                        ),
                        "thinking" | "redacted_thinking" => {}
                        _ => self.push(
                            offset,
                            TranscriptItem::new("terminal_block", block.to_string(), at(v)),
                        ),
                    }
                }
                if !user_text.is_empty() {
                    self.message(offset, v, kind, user_text.join("\n"), None);
                }
            }
            if v["message"]["stop_reason"] == "end_turn" {
                self.status(offset, v, "turn_finished", "Turn finished");
            }
        } else if kind == "system" {
            match v["subtype"].as_str().unwrap_or("") {
                "turn_duration" => self.status(offset, v, "turn_finished", "Turn finished"),
                "api_error" => {
                    let error = text(&v["error"]);
                    let kind = if error.to_ascii_lowercase().contains("rate")
                        || v["error"]["status"] == 429
                    {
                        "rate_limit"
                    } else {
                        "error"
                    };
                    self.status(offset, v, kind, &error);
                }
                _ => {}
            }
        }
    }
    pub fn antigravity(&mut self, v: &Value, offset: i64, model: &str) {
        if v["type"] == "unparsed" {
            self.push(
                offset,
                TranscriptItem::new("terminal_block", text(&v["raw"]), None),
            );
            return;
        }
        let stamp = serde_json::json!({"timestamp":v["created_at"]});
        self.model = Some(model.into());
        let role = if v["source"] == "MODEL" {
            "assistant"
        } else {
            "user"
        };
        if let Some(content) = v["content"].as_str().filter(|s| !s.trim().is_empty()) {
            self.message(offset, &stamp, role, content.into(), None);
        }
        if let Some(calls) = v["tool_calls"].as_array() {
            for (index, call) in calls.iter().enumerate() {
                let id = string(call, "id").unwrap_or_else(|| format!("agy:{offset}:{index}"));
                let block = serde_json::json!({"id":id,"name":call["name"],"input":call.get("args").or_else(|| call.get("arguments"))});
                self.tool(offset, &stamp, &block);
            }
        }
    }

    pub fn opencode(&mut self, message: &Value, part: &Value, id: &str, created_ms: i64) {
        let at = DateTime::<Utc>::from_timestamp_millis(created_ms);
        let stamp = serde_json::json!({"timestamp":at});
        self.model = string(message, "modelID");
        let start = self.rows.len();
        match part["type"].as_str().unwrap_or("") {
            "text" => {
                let role = message["role"].as_str().unwrap_or("assistant");
                self.message(
                    created_ms,
                    &stamp,
                    role,
                    part["text"].as_str().unwrap_or("").into(),
                    None,
                );
            }
            "tool" => {
                let state = &part["state"];
                let block = serde_json::json!({"id":id,"name":part["tool"],"input":state["input"]});
                self.tool(created_ms, &stamp, &block);
                if matches!(state["status"].as_str(), Some("completed" | "error")) {
                    let output = state
                        .get("output")
                        .or_else(|| state.get("error"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    self.result(
                        created_ms,
                        &stamp,
                        Some(id.into()),
                        &output,
                        state["status"] == "error",
                    );
                }
            }
            "step-start" => self.status(created_ms, &stamp, "turn_started", "Turn started"),
            "step-finish" => self.status(created_ms, &stamp, "turn_finished", "Turn finished"),
            "reasoning" => {}
            _ => self.push(
                created_ms,
                TranscriptItem::new("terminal_block", part.to_string(), at),
            ),
        }
        for row in &mut self.rows[start..] {
            row.item.id = Some(format!("part:{id}"));
        }
    }

    pub fn hermes(&mut self, v: &Value, offset: i64) {
        self.model = string(v, "model");
        let role = v["role"].as_str().unwrap_or("");
        if matches!(role, "user" | "assistant") {
            if let Some(content) = v["content"].as_str().filter(|s| !s.trim().is_empty()) {
                self.message(offset, v, role, content.into(), None);
            }
            if let Some(calls) = v["tool_calls"].as_array() {
                for call in calls {
                    let block = serde_json::json!({"id":call["id"], "name":call["function"]["name"], "arguments":call["function"]["arguments"]});
                    self.tool(offset, v, &block);
                }
            }
        } else if role == "tool" {
            self.result(offset, v, string(v, "tool_call_id"), &v["content"], false);
        }
    }

    pub fn codex(&mut self, v: &Value, offset: i64) {
        let p = &v["payload"];
        if let Some(m) = string(p, "model") {
            self.model = Some(m);
        }
        match v["type"].as_str().unwrap_or("") {
            "unparsed" => self.push(
                offset,
                TranscriptItem::new("terminal_block", text(&v["raw"]), at(v)),
            ),
            "response_item" => match p["type"].as_str().unwrap_or("") {
                "message" => {
                    let role = p["role"].as_str().unwrap_or("");
                    if matches!(role, "user" | "assistant") {
                        self.message(offset, v, role, text(&p["content"]), Some("response_item"));
                    }
                }
                "function_call" | "custom_tool_call" => self.tool(offset, v, p),
                "function_call_output" | "custom_tool_call_output" => {
                    let output = &p["output"];
                    let summary = text(output);
                    let error = summary
                        .split("Process exited with code ")
                        .nth(1)
                        .and_then(|s| s.split_whitespace().next())
                        .and_then(|s| s.parse::<i32>().ok())
                        .is_some_and(|code| code != 0)
                        || p["is_error"] == true
                        || output
                            .get("exit_code")
                            .and_then(Value::as_i64)
                            .is_some_and(|c| c != 0);
                    self.result(offset, v, string(p, "call_id"), output, error);
                }
                "reasoning" => {}
                _ => self.push(
                    offset,
                    TranscriptItem::new("terminal_block", p.to_string(), at(v)),
                ),
            },
            "event_msg" => match p["type"].as_str().unwrap_or("") {
                "user_message" => {
                    self.message(offset, v, "user", text(&p["message"]), Some("event_msg"))
                }
                "agent_message" => self.message(
                    offset,
                    v,
                    "assistant",
                    text(&p["message"]),
                    Some("event_msg"),
                ),
                "task_started" => {
                    self.messages.clear();
                    self.status(offset, v, "turn_started", "Turn started");
                }
                "token_count" => self.status(offset, v, "turn_usage", "Usage recorded"),
                "task_complete" => self.status(offset, v, "turn_finished", "Turn finished"),
                "rate_limit" | "rate_limit_error" => self.status(offset, v, "rate_limit", &text(p)),
                "usage_limit" => self.status(offset, v, "usage_limit", &text(p)),
                _ => {}
            },
            _ => {}
        }
    }
    fn message(
        &mut self,
        offset: i64,
        v: &Value,
        role: &str,
        mut value: String,
        source: Option<&str>,
    ) {
        if role == "user" {
            let cleaned = strip_injected_blocks(&value);
            if cleaned.trim().is_empty() {
                return;
            }
            if cleaned != value {
                // Keep Markdown whitespace inside the prose; only trim the removed frame edges.
                value = cleaned.trim().to_string();
            }
        }
        // Deduplicate provider mirrors after cleaning, including a raw/clean pair of user records.
        if let Some(source) = source {
            let key = format!("{role}:{value}");
            if self.messages.get(&key).is_some_and(|s| s != source) {
                self.messages.remove(&key);
                return;
            }
            self.messages.insert(key, source.into());
        }
        if role == "user" {
            let rows = crate::agent::repomail::split(&value, at(v));
            if rows.iter().any(|row| row.mail.is_some()) {
                for row in rows {
                    self.push(offset, row);
                }
                return;
            }
        }
        let mut item = if role == "assistant" && source.is_some() {
            crate::agent::codex_content::tool_item(&value)
                .map(|mut item| {
                    item.at = at(v);
                    item
                })
                .unwrap_or_else(|| TranscriptItem::new(role, value, at(v)))
        } else {
            TranscriptItem::new(role, value, at(v))
        };
        if role == "assistant" {
            item.model = self.model.clone();
        }
        self.push(offset, item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_ledger::scan::{
        ScanOptions, scan_claude_transcript_with_options, scan_codex_rollout_with_options,
    };
    const CONVERSATION: ScanOptions = ScanOptions {
        collect_transcript: true,
        before_offset: None,
    };
    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/usage_ledger/fixtures")
            .join(name)
    }
    #[test]
    fn other_provider_conversation_collection_is_opt_in_and_preserves_ledger_results() {
        use crate::usage_ledger::scan::{
            scan_antigravity_transcript, scan_antigravity_transcript_with_options,
            scan_opencode_db, scan_opencode_db_with_options,
        };
        let path = fixture("antigravity_usage_v0.jsonl");
        let legacy = scan_antigravity_transcript(&path, 0, "gemini-3", None).unwrap();
        let mut conversation =
            scan_antigravity_transcript_with_options(&path, 0, "gemini-3", None, CONVERSATION)
                .unwrap();
        assert!(
            conversation
                .transcript
                .iter()
                .any(|r| r.item.kind.as_deref() == Some("tool_call"))
        );
        conversation.transcript.clear();
        assert_eq!(legacy, conversation);
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        rusqlite::Connection::open(&db)
            .unwrap()
            .execute_batch(include_str!("fixtures/opencode_conversation_v0.sql"))
            .unwrap();
        let legacy = scan_opencode_db(&db, 0).unwrap();
        let mut conversation = scan_opencode_db_with_options(&db, 0, None, CONVERSATION).unwrap();
        assert!(
            conversation
                .transcript
                .iter()
                .any(|r| r.item.result_summary.as_deref() == Some("file content"))
        );
        conversation.transcript.clear();
        assert_eq!(legacy, conversation);
    }

    #[test]
    fn ledger_fixtures_map_both_providers() {
        let claude = scan_claude_transcript_with_options(
            &fixture("claude_usage_v0.jsonl"),
            0,
            None,
            CONVERSATION,
        )
        .unwrap();
        assert!(
            claude
                .transcript
                .iter()
                .any(|r| r.item.kind.as_deref() == Some("user")
                    && r.item.text == "Wire up the ledger")
        );
        assert!(
            claude
                .transcript
                .iter()
                .any(|r| r.item.model.as_deref() == Some("claude-sonnet-5"))
        );
        let tool = claude
            .transcript
            .iter()
            .find(|r| r.item.name.as_deref() == Some("Read"))
            .unwrap();
        assert_eq!(tool.item.status, Some(ToolCallStatus::Running));
        assert!(tool.item.input_summary.as_ref().unwrap().contains("a.rs"));
        let codex = scan_codex_rollout_with_options(
            &fixture("codex_injected_preamble_v0.jsonl"),
            0,
            CONVERSATION,
        )
        .unwrap();
        assert!(
            !codex
                .transcript
                .iter()
                .any(|r| r.item.kind.as_deref() == Some("user"))
        );
        let usage =
            scan_codex_rollout_with_options(&fixture("codex_usage_v0.jsonl"), 0, CONVERSATION)
                .unwrap();
        assert!(
            usage
                .transcript
                .iter()
                .any(|r| r.item.status_kind.as_deref() == Some("turn_finished"))
        );
    }
    #[test]
    fn injected_user_fixtures_are_cleaned_through_both_production_scanners() {
        let fixtures: Value =
            serde_json::from_str(include_str!("fixtures/injected_user_frames_v0.json")).unwrap();
        for case in fixtures["cases"].as_array().unwrap() {
            let raw = case["text"].as_str().unwrap();
            let expected = case["expected"].as_str();
            // Exercise Claude string content, a single text block, and wrappers split across
            // adjacent text blocks, plus both Codex copies (raw then cleaned) of the same turn.
            for layout in 0..4 {
                let mut records = Vec::new();
                if layout < 3 {
                    let content = match layout {
                        0 => serde_json::json!(raw),
                        1 => serde_json::json!([{"type":"text", "text":raw}]),
                        _ => serde_json::json!(
                            raw.split('\n')
                                .map(|part| { serde_json::json!({"type":"text", "text":part}) })
                                .collect::<Vec<_>>()
                        ),
                    };
                    records.push(serde_json::json!({"type":"user","message":{"content":content}}));
                } else {
                    records.push(serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":raw}}));
                    records.push(serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":expected.unwrap_or(raw)}]}}));
                }
                let file = tempfile::NamedTempFile::new().unwrap();
                let jsonl = records.iter().map(|v| format!("{v}\n")).collect::<String>();
                std::fs::write(file.path(), jsonl).unwrap();
                let scan = if layout < 3 {
                    scan_claude_transcript_with_options(file.path(), 0, None, CONVERSATION).unwrap()
                } else {
                    scan_codex_rollout_with_options(file.path(), 0, CONVERSATION).unwrap()
                };
                let users: Vec<_> = scan
                    .transcript
                    .iter()
                    .filter(|r| r.item.kind.as_deref() == Some("user"))
                    .collect();
                assert_eq!(
                    users.len(),
                    usize::from(expected.is_some()),
                    "{} layout {layout}: {:?}",
                    case["name"],
                    scan.transcript
                );
                if let Some(expected) = expected {
                    assert_eq!(
                        users[0].item.text, expected,
                        "{} layout {layout}",
                        case["name"]
                    );
                }
                // No hidden replacement terminal row should render an injection-only record.
                assert_eq!(
                    scan.transcript.len(),
                    users.len(),
                    "{} layout {layout}",
                    case["name"]
                );
            }
        }
    }

    #[test]
    fn assistant_markup_and_structured_tools_survive_user_frame_cleaning() {
        let raw = "<output-file>/private/tmp/result</output-file>";
        let mut mapper = Mapper::default();
        mapper.claude(&serde_json::json!({"type":"assistant","message":{"model":"claude-sonnet-5","content":[
            {"type":"text","text":raw},
            {"type":"tool_use","id":"read-1","name":"Read","input":{"file_path":"/private/tmp/result"}}
        ]}}), 0);
        mapper.claude(
            &serde_json::json!({"type":"user","message":{"content":[
                {"type":"text","text":"<task-notification>done</task-notification>"},
                {"type":"tool_result","tool_use_id":"read-1","content":raw},
                {"type":"text","text":"Please fix the preview."},
                {"type":"text","text":"[Image: source: /private/tmp/preview.png]"}
            ]}}),
            10,
        );
        assert_eq!(mapper.rows.len(), 3);
        assert_eq!(mapper.rows[0].item.text, raw);
        assert_eq!(
            mapper.rows[0].item.model.as_deref(),
            Some("claude-sonnet-5")
        );
        assert_eq!(mapper.rows[1].item.result_summary.as_deref(), Some(raw));
        assert_eq!(mapper.rows[1].item.status, Some(ToolCallStatus::Ok));
        assert_eq!(mapper.rows[2].item.text, "Please fix the preview.");
        let mut codex = Mapper::default();
        codex.codex(&serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":raw}}), 0);
        assert_eq!(codex.rows[0].item.text, raw);
    }

    #[test]
    fn tools_resolve_diffs_and_codex_message_mirrors_are_not_duplicated() {
        let mut mapper = Mapper::default();
        mapper.claude(&serde_json::json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"edit-1","name":"Edit","input":{"file_path":"a.rs","old_string":"old","new_string":"new"}}]}}), 0);
        mapper.claude(&serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"edit-1","content":"done","is_error":false}]}}), 100);
        assert_eq!(mapper.rows.len(), 1);
        assert_eq!(mapper.rows[0].item.status, Some(ToolCallStatus::Ok));
        assert!(
            mapper.rows[0]
                .item
                .diff
                .as_ref()
                .unwrap()
                .contains("-old\n+new")
        );
        let mut mapper = Mapper::default();
        mapper.codex(
            &serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5"}}),
            0,
        );
        mapper.codex(&serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":"hello"}}), 10);
        mapper.codex(&serde_json::json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}}), 20);
        assert_eq!(mapper.rows.len(), 1);
        assert_eq!(mapper.rows[0].item.model.as_deref(), Some("gpt-5"));
        mapper.codex(&serde_json::json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","call_id":"call-1","arguments":"{\"cmd\":\"false\"}"}}), 30);
        mapper.codex(&serde_json::json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call-1","output":"Process exited with code 1"}}), 40);
        assert_eq!(mapper.rows[1].item.status, Some(ToolCallStatus::Error));
    }
    #[test]
    fn malformed_complete_rows_fall_back_and_incomplete_tail_waits() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "broken record\n{\"unfinished\"").unwrap();
        for scan in [
            scan_codex_rollout_with_options(file.path(), 0, CONVERSATION).unwrap(),
            scan_claude_transcript_with_options(file.path(), 0, None, CONVERSATION).unwrap(),
        ] {
            assert_eq!(scan.transcript.len(), 1);
            assert_eq!(
                scan.transcript[0].item.kind.as_deref(),
                Some("terminal_block")
            );
            assert_eq!(scan.next_offset, 14);
        }
    }
    #[test]
    fn legacy_roles_still_deserialize_and_serialize() {
        for role in ["user", "assistant", "tools"] {
            let value = serde_json::json!({"role":role,"text":"text","at":null});
            let item: TranscriptItem = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(item).unwrap(), value);
        }
    }
}

#[cfg(test)]
mod codex_machine_content_tests {
    use super::*;
    #[test]
    fn codex_rollups_and_numbered_diffs_use_existing_tool_rows_but_user_code_stays_user() {
        for content in [
            "Ran 1 shell command",
            "Searched for 1 pattern, ran 2 shell commands",
            "Searched for 2 patterns, ran 7 shell commands",
            "205 + await screen.findByText(\"Ready\");\n206 + expect(button).toBeEnabled();",
        ] {
            let mut mapper = Mapper::default();
            mapper.codex(&serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":content}}), 0);
            assert_eq!(mapper.rows[0].item.kind.as_deref(), Some("tool_call"));
            assert_eq!(mapper.rows[0].item.status, Some(ToolCallStatus::Ok));
            // Provider mirror still deduplicates after machine content classification.
            mapper.codex(&serde_json::json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":content}]}}), 100);
            assert_eq!(mapper.rows.len(), 1);
            mapper.codex(&serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":content}}), 200);
            assert_eq!(mapper.rows[1].item.kind.as_deref(), Some("user"));
            assert_eq!(mapper.rows[1].item.text, content);
            if content.contains("205 +") {
                assert_eq!(mapper.rows[0].item.diff.as_deref(), Some(content));
            }
        }
    }
    #[test]
    fn actual_codex_tool_output_already_routes_to_result_not_assistant_fallthrough() {
        let mut mapper = Mapper::default();
        let output =
            "205 + await screen.findByText(\"Ready\");\n206 + expect(button).toBeEnabled();";
        mapper.codex(&serde_json::json!({"type":"response_item","payload":{"type":"function_call","call_id":"test","name":"exec_command","arguments":"{}"}}), 0);
        mapper.codex(&serde_json::json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"test","output":output}}), 100);
        assert_eq!(mapper.rows.len(), 1);
        assert_eq!(mapper.rows[0].item.kind.as_deref(), Some("tool_call"));
        assert_eq!(mapper.rows[0].item.result_summary.as_deref(), Some(output));
        assert!(mapper.rows.iter().all(|r| r.item.role != "assistant"));
    }
}
