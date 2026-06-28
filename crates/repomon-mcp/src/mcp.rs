//! A minimal, hand-rolled Model Context Protocol (MCP) server over stdio.
//!
//! The transport is the MCP stdio convention: newline-delimited JSON-RPC 2.0 messages on
//! stdin/stdout (one message per line, no embedded newlines), with logs kept to stderr so they
//! never corrupt the protocol stream. We hand-roll it — exactly as `repomon-core::protocol`
//! hand-rolls the daemon's framed JSON-RPC — to avoid pulling a heavy SDK with a churning macro
//! API for what is a small, stable surface: `initialize`, `tools/list`, `tools/call`, `ping`.
//!
//! Each `tools/call` runs in its own task and writes its response when it completes, so a long
//! blocking tool (`wait_for_change`) never stalls `ping` or other calls. Responses are matched
//! by id on the client side, so out-of-order completion is fine.

use std::sync::Arc;

use anyhow::Result;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// The MCP revision we advertise when a client doesn't pin one. We otherwise echo the client's
/// requested version, since our JSON handling is version-agnostic.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// A tool advertised in `tools/list`.
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

/// The outcome of a `tools/call`: text content plus whether it represents a tool-level error
/// (surfaced to the model as `isError: true` so it can react, rather than a protocol error).
pub struct ToolResult {
    pub text: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn ok(text: impl Into<String>) -> Self {
        ToolResult {
            text: text.into(),
            is_error: false,
        }
    }
    pub fn error(text: impl Into<String>) -> Self {
        ToolResult {
            text: text.into(),
            is_error: true,
        }
    }
}

/// What a concrete MCP server must provide: the tool catalog and a dispatcher.
#[async_trait::async_trait]
pub trait ToolHandler: Send + Sync + 'static {
    fn tools(&self) -> Vec<ToolDef>;
    async fn call(&self, name: &str, args: Value) -> ToolResult;
}

/// Serve the MCP protocol over stdio until stdin reaches EOF (the client closed us).
pub async fn run_stdio<H: ToolHandler>(
    handler: Arc<H>,
    server_name: &str,
    server_version: &str,
) -> Result<()> {
    let server_name = server_name.to_string();
    let server_version = server_version.to_string();
    let tools = handler.tools();

    // One writer task owns stdout so concurrent tool tasks can't interleave bytes mid-line.
    let (tx, mut rx) = mpsc::channel::<String>(64);
    let writer = tokio::spawn(async move {
        let mut out = tokio::io::stdout();
        while let Some(msg) = rx.recv().await {
            if out.write_all(msg.as_bytes()).await.is_err() || out.write_all(b"\n").await.is_err() {
                break;
            }
            let _ = out.flush().await;
        }
    });

    let mut reader = BufReader::new(tokio::io::stdin());
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break; // EOF: the orchestrator's claude process closed the server.
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("mcp: ignoring unparseable message: {e}");
                continue;
            }
        };
        let id = msg.get("id").cloned();
        let Some(method) = msg.get("method").and_then(|m| m.as_str()) else {
            continue; // a response to a request we never sent — ignore.
        };

        match method {
            "initialize" => {
                let proto = msg
                    .get("params")
                    .and_then(|p| p.get("protocolVersion"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(PROTOCOL_VERSION)
                    .to_string();
                let result = json!({
                    "protocolVersion": proto,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": server_name, "version": server_version },
                });
                send_result(&tx, id, result).await;
            }
            "ping" => send_result(&tx, id, json!({})).await,
            "tools/list" => {
                let listed: Vec<Value> = tools
                    .iter()
                    .map(|t| {
                        json!({
                            "name": t.name,
                            "description": t.description,
                            "inputSchema": t.input_schema,
                        })
                    })
                    .collect();
                send_result(&tx, id, json!({ "tools": listed })).await;
            }
            "tools/call" => {
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                let name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let handler = handler.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let res = handler.call(&name, args).await;
                    let result = json!({
                        "content": [ { "type": "text", "text": res.text } ],
                        "isError": res.is_error,
                    });
                    send_result(&tx, id, result).await;
                });
            }
            // Lifecycle/utility notifications we acknowledge by doing nothing.
            "notifications/initialized" | "notifications/cancelled" => {}
            other => {
                if id.is_some() {
                    send_error(&tx, id, -32601, format!("method not found: {other}")).await;
                }
            }
        }
    }

    drop(tx);
    let _ = writer.await;
    Ok(())
}

async fn send_result(tx: &mpsc::Sender<String>, id: Option<Value>, result: Value) {
    let Some(id) = id else { return };
    let env = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    let _ = tx.send(env.to_string()).await;
}

async fn send_error(tx: &mpsc::Sender<String>, id: Option<Value>, code: i64, message: String) {
    let Some(id) = id else { return };
    let env = json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } });
    let _ = tx.send(env.to_string()).await;
}
