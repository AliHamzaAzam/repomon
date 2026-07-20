use anyhow::Error;
use repomon_core::protocol::{Notification, RpcError};
use serde::Serialize;
use serde_json::Value;
use tauri::State;
use tauri::ipc::Channel;

use crate::state::AppState;

const NOT_CONNECTED: i64 = -32098;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RpcFailure {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcFailure {
    fn not_connected() -> Self {
        Self {
            code: NOT_CONNECTED,
            message: "daemon connection is still starting".into(),
            data: None,
        }
    }
}

impl From<&RpcError> for RpcFailure {
    fn from(error: &RpcError) -> Self {
        Self {
            code: error.code,
            message: error.message.clone(),
            data: error.data.clone(),
        }
    }
}

fn map_call_error(error: Error) -> RpcFailure {
    error
        .downcast_ref::<RpcError>()
        .map(RpcFailure::from)
        .unwrap_or_else(|| RpcFailure {
            code: -32000,
            message: error.to_string(),
            data: None,
        })
}

#[tauri::command]
pub async fn daemon_call(
    state: State<'_, AppState>,
    method: String,
    params: Option<Value>,
) -> Result<Value, RpcFailure> {
    let client = state.client.get().ok_or_else(RpcFailure::not_connected)?;
    client.call(&method, params).await.map_err(map_call_error)
}

#[tauri::command]
pub async fn daemon_subscribe(
    state: State<'_, AppState>,
    on_event: Channel<Notification>,
) -> Result<(), RpcFailure> {
    let client = state
        .client
        .get()
        .ok_or_else(RpcFailure::not_connected)?
        .clone();
    let mut events = client.subscribe();
    client
        .call("subscribe", None)
        .await
        .map_err(map_call_error)?;

    tauri::async_runtime::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) if event.method != "event.agent.bytes" => {
                    if on_event.send(event).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;
    use serde_json::json;

    use super::{NOT_CONNECTED, RpcFailure, map_call_error};
    use repomon_core::protocol::RpcError;

    #[test]
    fn rpc_errors_keep_code_message_and_data() {
        let source = RpcError {
            code: -32010,
            message: "dialog changed".into(),
            data: Some(json!({ "dialog": null })),
        };
        let error = map_call_error(source.into());

        assert_eq!(error.code, -32010);
        assert_eq!(error.message, "dialog changed");
        assert_eq!(error.data, Some(json!({ "dialog": null })));
    }

    #[test]
    fn host_errors_use_a_stable_internal_code() {
        let error = map_call_error(anyhow!("socket closed"));
        assert_eq!(error.code, -32000);
        assert_eq!(error.message, "socket closed");
        assert_eq!(RpcFailure::not_connected().code, NOT_CONNECTED);
    }
}
