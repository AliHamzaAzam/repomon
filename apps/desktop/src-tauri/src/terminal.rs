use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use repomon_core::client::DaemonClient;
use repomon_core::protocol::Notification;
use serde::Serialize;
use serde_json::{Value, json};
use tauri::State;
use tauri::ipc::{Channel, InvokeResponseBody};
use tokio::sync::oneshot;

use crate::ipc::{RpcFailure, map_call_error};
use crate::state::AppState;

const FLUSH_INTERVAL: Duration = Duration::from_millis(16);
const FLUSH_BYTES: usize = 32 * 1024;
const MAX_PENDING: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TermWatchAck {
    pub cols: Option<u16>,
    pub rows: Option<u16>,
}

fn dimensions(value: &Value) -> TermWatchAck {
    TermWatchAck {
        cols: value.get("cols").and_then(Value::as_u64).map(|n| n as u16),
        rows: value.get("rows").and_then(Value::as_u64).map(|n| n as u16),
    }
}

fn event_bytes(event: &Notification, window: &str) -> Option<Vec<u8>> {
    if event.method != "event.agent.bytes"
        || event.params.get("window").and_then(Value::as_str) != Some(window)
    {
        return None;
    }
    let encoded = event.params.get("data").and_then(Value::as_str)?;
    STANDARD.decode(encoded).ok()
}

fn append_pending(pending: &mut Vec<u8>, bytes: &[u8]) -> bool {
    if pending.len().saturating_add(bytes.len()) > MAX_PENDING {
        pending.clear();
        return false;
    }
    pending.extend_from_slice(bytes);
    true
}

fn flush(channel: &Channel<InvokeResponseBody>, pending: &mut Vec<u8>) -> bool {
    if pending.is_empty() {
        return true;
    }
    channel
        .send(InvokeResponseBody::Raw(std::mem::take(pending)))
        .is_ok()
}

/// Assemble a resync repaint: clear the screen + scrollback and home, then the captured screen.
/// tmux joins captured lines with a bare LF, which only moves the cursor DOWN in a raw stream, so
/// re-anchor each line to column 0 (`\r\n`) — otherwise the repaint staircases to the right (the
/// same reason the TUI's `Emu::seed_capture` rewrites newlines).
fn resync_frame(content: &str, alternate: bool) -> Vec<u8> {
    let body = content.replace('\n', "\r\n");
    let mut bytes = Vec::with_capacity(body.len() + 16);
    bytes.extend_from_slice(if alternate {
        b"\x1b[?1049h"
    } else {
        b"\x1b[?1049l"
    });
    bytes.extend_from_slice(b"\x1b[H\x1b[2J\x1b[3J");
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

async fn capture_resync(
    client: &DaemonClient,
    channel: &Channel<InvokeResponseBody>,
    lane_id: i64,
    window: &str,
) -> bool {
    let capture = client
        .call(
            "agent.capture",
            Some(json!({
                "lane_id": lane_id,
                "window": window,
                "lines": 500,
                "include_state": true
            })),
        )
        .await;
    let Ok(value) = capture else { return true };
    let Some(content) = value.get("content").and_then(Value::as_str) else {
        return true;
    };
    let alternate = value
        .get("alternate")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    channel
        .send(InvokeResponseBody::Raw(resync_frame(content, alternate)))
        .is_ok()
}

#[tauri::command]
pub async fn term_watch(
    state: State<'_, AppState>,
    lane_id: i64,
    window: String,
    on_bytes: Channel<InvokeResponseBody>,
) -> Result<TermWatchAck, RpcFailure> {
    // A rapid unmount/remount (tab switch, or a re-render that reuses a window id) can arrive
    // before the previous pane's fire-and-forget `term_unwatch` has finished. Rather than reject
    // the new pane, tear the stale watch down first and let this one take over. Done before the
    // daemon `on:true` join below so the old task's `on:false` can't deregister the new watch.
    let stale = state.terminal_watches.lock().unwrap().remove(&window);
    if let Some(cancel) = stale {
        let (ack_tx, ack_rx) = oneshot::channel();
        if cancel.send(ack_tx).is_ok() {
            let _ = ack_rx.await;
        }
    }

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
    // Unix byte streams contain future PTY output only. Paint the current screen first so a cold
    // terminal is useful immediately, then attach the live stream for subsequent output.
    #[cfg(not(target_os = "windows"))]
    if !capture_resync(&client, &on_bytes, lane_id, &window).await {
        return Err(RpcFailure {
            code: -32000,
            message: "terminal channel closed during initial repaint".into(),
            data: None,
        });
    }
    let value = client
        .call(
            "agent.watch_bytes",
            Some(json!({ "lane_id": lane_id, "window": window, "on": true })),
        )
        .await
        .map_err(map_call_error)?;
    let ack = dimensions(&value);

    let (cancel_tx, mut cancel_rx) = oneshot::channel::<oneshot::Sender<()>>();
    state
        .terminal_watches
        .lock()
        .unwrap()
        .insert(window.clone(), cancel_tx);
    let watches = state.terminal_watches.clone();
    let task_window = window.clone();
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(FLUSH_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut pending = Vec::with_capacity(FLUSH_BYTES);
        let mut resync = false;
        let mut cancelled = None;

        loop {
            tokio::select! {
                ack = &mut cancel_rx => {
                    cancelled = ack.ok();
                    break;
                }
                event = events.recv() => match event {
                    Ok(event) => {
                        if let Some(bytes) = event_bytes(&event, &task_window) {
                            if !append_pending(&mut pending, &bytes) {
                                resync = true;
                            } else if pending.len() >= FLUSH_BYTES && !flush(&on_bytes, &mut pending) {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => resync = true,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
                _ = ticker.tick() => {
                    if resync {
                        pending.clear();
                        resync = false;
                        if !capture_resync(&client, &on_bytes, lane_id, &task_window).await {
                            break;
                        }
                    } else if !flush(&on_bytes, &mut pending) {
                        break;
                    }
                }
            }
        }

        let _ = client
            .call(
                "agent.watch_bytes",
                Some(json!({ "lane_id": lane_id, "window": task_window, "on": false })),
            )
            .await;
        watches.lock().unwrap().remove(&window);
        if let Some(ack) = cancelled {
            let _ = ack.send(());
        }
    });

    Ok(ack)
}

#[tauri::command]
pub async fn term_unwatch(state: State<'_, AppState>, window: String) -> Result<(), RpcFailure> {
    let cancel = state.terminal_watches.lock().unwrap().remove(&window);
    if let Some(cancel) = cancel {
        let (ack_tx, ack_rx) = oneshot::channel();
        if cancel.send(ack_tx).is_ok() {
            let _ = ack_rx.await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use repomon_core::protocol::Notification;
    use serde_json::json;

    use super::{MAX_PENDING, append_pending, dimensions, event_bytes, resync_frame};

    #[test]
    fn resync_frame_reanchors_bare_newlines() {
        let frame = resync_frame("line1\nline2", true);
        let text = String::from_utf8(frame).unwrap();
        // Clears screen + scrollback, homes, then repaints with CR-anchored lines.
        assert!(text.starts_with("\x1b[?1049h\x1b[H\x1b[2J\x1b[3J"));
        assert!(text.contains("line1\r\nline2"));
        assert!(!text.contains("line1\nline2"));
    }

    #[test]
    fn resync_frame_restores_normal_screen_mode() {
        let frame = String::from_utf8(resync_frame("shell", false)).unwrap();
        assert!(frame.starts_with("\x1b[?1049l\x1b[H\x1b[2J\x1b[3J"));
    }

    #[test]
    fn routes_and_decodes_only_the_requested_window() {
        let bytes = b"\x1b[32mready\x1b[0m";
        let event = Notification::new(
            "event.agent.bytes",
            json!({ "window": "lane-7", "data": STANDARD.encode(bytes) }),
        );
        assert_eq!(event_bytes(&event, "lane-7"), Some(bytes.to_vec()));
        assert_eq!(event_bytes(&event, "lane-8"), None);
    }

    #[test]
    fn overflow_drops_pending_and_requests_resync() {
        let mut pending = vec![0; MAX_PENDING];
        assert!(!append_pending(&mut pending, b"x"));
        assert!(pending.is_empty());
    }

    #[test]
    fn watch_dimensions_allow_missing_pane_size() {
        assert_eq!(
            dimensions(&json!({ "cols": 120, "rows": 40 })).cols,
            Some(120)
        );
        assert_eq!(
            dimensions(&json!({ "cols": null, "rows": null })).rows,
            None
        );
    }
}
