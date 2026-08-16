//! Restricted MCP surface for managed worker agents.

use repomon_core::client::DaemonClient;
use serde_json::{Value, json};

use crate::fleet::Fleet;
use crate::mcp::{ToolDef, ToolHandler, ToolResult};

pub struct AgentServer {
    client: DaemonClient,
    fleet: Fleet,
    identity_token: String,
}

impl AgentServer {
    pub fn new(client: DaemonClient, fleet: Fleet, identity_token: String) -> Self {
        Self {
            client,
            fleet,
            identity_token,
        }
    }

    async fn fleet_status(&self, args: Value) -> Result<Value, String> {
        let repo = args.get("repo").and_then(Value::as_str);
        let only_attention = args
            .get("only_attention")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let (generation, mut lanes) = self.fleet.current().await;
        if let Some(repo) = repo {
            lanes.retain(|lane| lane.repo == repo);
        }
        if only_attention {
            lanes.retain(|lane| lane.attention().needs_you());
        }
        Ok(json!({ "generation": generation, "lanes": lanes }))
    }

    fn require_identity(&self) -> Result<&str, String> {
        if self.identity_token.is_empty() {
            Err("missing managed-agent MCP identity".into())
        } else {
            Ok(&self.identity_token)
        }
    }

    async fn message_send(&self, args: Value) -> Result<Value, String> {
        let token = self.require_identity()?;
        let to = args
            .get("to")
            .filter(|value| value.is_string() || value.is_array())
            .cloned()
            .ok_or_else(|| "missing to".to_string())?;
        let body = args
            .get("body")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing body".to_string())?;
        self.client
            .call(
                "message.send",
                Some(json!({
                    "to": to,
                    "body": body,
                    "reply_to": args.get("reply_to"),
                    "identity_token": token,
                })),
            )
            .await
            .map_err(|error| error.to_string())
    }

    async fn message_inbox(&self, args: Value) -> Result<Value, String> {
        let token = self.require_identity()?;
        self.client
            .call(
                "message.inbox",
                Some(json!({
                    "unread_only": args.get("unread_only").and_then(Value::as_bool).unwrap_or(false),
                    "limit": args.get("limit"),
                    "before": args.get("before"),
                    "identity_token": token,
                })),
            )
            .await
            .map_err(|error| error.to_string())
    }

    async fn message_mark_read(&self, args: Value) -> Result<Value, String> {
        let token = self.require_identity()?;
        let id = args
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing id".to_string())?;
        self.client
            .call(
                "message.mark_read",
                Some(json!({ "id": id, "identity_token": token })),
            )
            .await
            .map_err(|error| error.to_string())
    }
}

#[async_trait::async_trait]
impl ToolHandler for AgentServer {
    fn tools(&self) -> Vec<ToolDef> {
        agent_tool_catalog()
    }

    async fn call(&self, name: &str, args: Value) -> ToolResult {
        let result = match name {
            "fleet_status" => self.fleet_status(args).await,
            "message_send" => self.message_send(args).await,
            "message_inbox" => self.message_inbox(args).await,
            "message_mark_read" => self.message_mark_read(args).await,
            other => Err(format!("unknown tool: {other}")),
        };
        match result {
            Ok(value) => ToolResult::ok(value.to_string()),
            Err(error) => ToolResult::error(error),
        }
    }
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

pub fn agent_tool_catalog() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "fleet_status",
            description: "Read a compact fleet snapshot. This worker-only server cannot mutate the fleet.",
            input_schema: object(
                json!({
                    "repo": { "type": "string" },
                    "only_attention": { "type": "boolean" }
                }),
                &[],
            ),
        },
        ToolDef {
            name: "message_send",
            description: "Send durable fleet mail. `to` accepts a single canonical address \
                (\"lane-2/1\", \"operator\", \"@label\"), a JSON array of addresses to fan the \
                same message out to each one, \"lane-2/*\" for every active agent in that lane, \
                or \"*\" for every active agent in the fleet. Wildcard/list sends never mail the \
                sending agent's own session; an explicit self-address still delivers. A single \
                plain address returns the sent message; a list or wildcard returns a \
                per-recipient summary (sent / no_such_session / delivery_error).",
            input_schema: object(
                json!({
                    "to": {
                        "oneOf": [
                            { "type": "string" },
                            { "type": "array", "items": { "type": "string" }, "minItems": 1 }
                        ],
                        "description": "\"lane-2/1\" | \"operator\" | \"@label\" | \"lane-2/*\" | \"*\" | an array of any of those."
                    },
                    "body": { "type": "string", "maxLength": 8192 },
                    "reply_to": { "type": "string" }
                }),
                &["to", "body"],
            ),
        },
        ToolDef {
            name: "message_inbox",
            description: "Read this agent's durable inbox. Polling marks returned mail delivered.",
            input_schema: object(
                json!({
                    "unread_only": { "type": "boolean" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
                    "before": { "type": "string" }
                }),
                &[],
            ),
        },
        ToolDef {
            name: "message_mark_read",
            description: "Mark one message in this agent's inbox read.",
            input_schema: object(json!({ "id": { "type": "string" } }), &["id"]),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricted_catalog_is_exact() {
        let names: Vec<&str> = agent_tool_catalog().iter().map(|tool| tool.name).collect();
        assert_eq!(
            names,
            [
                "fleet_status",
                "message_send",
                "message_inbox",
                "message_mark_read"
            ]
        );
        assert!(!names.contains(&"spawn_agent"));
        assert!(!names.contains(&"merge_lane"));
    }
}
