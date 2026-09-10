//! Maps records already decoded by the ledger scanners into the conversation contract.
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
                let mut item = TranscriptItem::new(kind, s, at(v));
                if kind == "assistant" {
                    item.model = self.model.clone();
                }
                self.push(offset, item);
            } else if let Some(blocks) = content.as_array() {
                for block in blocks {
                    match block["type"].as_str().unwrap_or("") {
                        "text" => {
                            let mut item = TranscriptItem::new(
                                kind,
                                block["text"].as_str().unwrap_or(""),
                                at(v),
                            );
                            if kind == "assistant" {
                                item.model = self.model.clone();
                            }
                            self.push(offset, item);
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
                        self.message(offset, v, role, text(&p["content"]), "response_item");
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
                "user_message" => self.message(offset, v, "user", text(&p["message"]), "event_msg"),
                "agent_message" => {
                    self.message(offset, v, "assistant", text(&p["message"]), "event_msg")
                }
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
    fn message(&mut self, offset: i64, v: &Value, role: &str, value: String, source: &str) {
        let key = format!("{role}:{value}");
        if self.messages.get(&key).is_some_and(|s| s != source) {
            self.messages.remove(&key);
            return;
        }
        self.messages.insert(key, source.into());
        let mut item = TranscriptItem::new(role, value, at(v));
        if role == "assistant" {
            item.model = self.model.clone();
        }
        self.push(offset, item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_ledger::scan::{scan_claude_transcript, scan_codex_rollout};
    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/usage_ledger/fixtures")
            .join(name)
    }
    #[test]
    fn ledger_fixtures_map_both_providers() {
        let claude = scan_claude_transcript(&fixture("claude_usage_v0.jsonl"), 0, None).unwrap();
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
        let codex = scan_codex_rollout(&fixture("codex_injected_preamble_v0.jsonl"), 0).unwrap();
        assert!(
            codex
                .transcript
                .iter()
                .any(|r| r.item.kind.as_deref() == Some("user"))
        );
        let usage = scan_codex_rollout(&fixture("codex_usage_v0.jsonl"), 0).unwrap();
        assert!(
            usage
                .transcript
                .iter()
                .any(|r| r.item.status_kind.as_deref() == Some("turn_finished"))
        );
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
            scan_codex_rollout(file.path(), 0).unwrap(),
            scan_claude_transcript(file.path(), 0, None).unwrap(),
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
