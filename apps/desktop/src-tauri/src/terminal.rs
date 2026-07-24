use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use repomon_core::client::DaemonClient;
use repomon_core::protocol::Notification;
use serde::Serialize;
use serde_json::{Value, json};
use tauri::State;
use tauri::ipc::{Channel, InvokeResponseBody};
use tokio::sync::{broadcast, oneshot};

use crate::ipc::{RpcFailure, map_call_error};
use crate::state::AppState;

const FLUSH_INTERVAL: Duration = Duration::from_millis(16);
const FLUSH_BYTES: usize = 32 * 1024;
const MAX_PENDING: usize = 1024 * 1024;
/// Minimum spacing between resync capture attempts. While a pane streams heavily the daemon
/// keeps answering `stable: false`; retrying on every 16ms flush tick hammered a 500-line
/// scrollback capture (held under the host's dispatcher lock) ~60x/s. One attempt per 100ms
/// still repaints within a frame or two of the pane going quiet.
const RESYNC_RETRY: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TermWatchAck {
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub generation: Option<u64>,
    pub sequence: Option<u64>,
}

fn dimensions(value: &Value) -> TermWatchAck {
    TermWatchAck {
        cols: value.get("cols").and_then(Value::as_u64).map(|n| n as u16),
        rows: value.get("rows").and_then(Value::as_u64).map(|n| n as u16),
        generation: value.get("generation").and_then(Value::as_u64),
        sequence: value.get("sequence").and_then(Value::as_u64),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StreamCursor {
    generation: u64,
    sequence: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ByteChunk {
    cursor: StreamCursor,
    bytes: Vec<u8>,
}

/// One routed item on a window's byte channel: a decoded chunk, or notice that the upstream
/// daemon subscription lagged (every receiver must resync — chunks were dropped).
#[derive(Debug, PartialEq, Eq)]
pub enum RouteFrame {
    Chunk(ByteChunk),
    Lagged,
}

/// Match and decode one `event.agent.bytes` notification into `(window, chunk)`. Base64 is
/// decoded exactly once, in the demux — panes receive ready bytes instead of each scanning
/// and decoding every event on the connection.
fn event_chunk(event: &Notification) -> Option<(String, ByteChunk)> {
    if event.method != "event.agent.bytes" {
        return None;
    }
    let window = event.params.get("window").and_then(Value::as_str)?;
    let generation = event.params.get("generation").and_then(Value::as_u64)?;
    let sequence = event.params.get("sequence").and_then(Value::as_u64)?;
    let encoded = event.params.get("data").and_then(Value::as_str)?;
    Some((
        window.to_string(),
        ByteChunk {
            cursor: StreamCursor {
                generation,
                sequence,
            },
            bytes: STANDARD.decode(encoded).ok()?,
        },
    ))
}

/// Start the one demux task that owns the app's daemon event subscription: byte chunks are
/// decoded once and routed to exactly their window's channel; every other event is
/// re-broadcast on `ui_events` for `daemon_subscribe`. Before this, every mounted pane held
/// its own subscription — each byte chunk was cloned per pane and filtered N-1 times.
pub(crate) async fn ensure_demux(state: &State<'_, AppState>) -> Result<(), RpcFailure> {
    let client = state
        .client
        .get()
        .ok_or_else(RpcFailure::not_connected)?
        .clone();
    let routes = state.terminal_routes.clone();
    let ui_events = state.ui_events.clone();
    state
        .demux_started
        .get_or_try_init(|| async move {
            let mut events = client.subscribe();
            client.call("subscribe", None).await.map_err(map_call_error)?;
            tauri::async_runtime::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(event) => {
                            if let Some((window, chunk)) = event_chunk(&event) {
                                let tx = routes.lock().unwrap().get(&window).cloned();
                                if let Some(tx) = tx {
                                    let _ = tx.send(Arc::new(RouteFrame::Chunk(chunk)));
                                }
                            } else {
                                let _ = ui_events.send(event);
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            // Chunks were dropped upstream; every routed pane must resync.
                            let senders: Vec<_> =
                                routes.lock().unwrap().values().cloned().collect();
                            for tx in senders {
                                let _ = tx.send(Arc::new(RouteFrame::Lagged));
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            Ok(())
        })
        .await
        .map(|_: &()| ())
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
    // Swap in a pre-sized buffer: `mem::take` would leave capacity 0 and re-grow every tick.
    let bytes = std::mem::replace(pending, Vec::with_capacity(FLUSH_BYTES));
    channel.send(InvokeResponseBody::Raw(bytes)).is_ok()
}

/// Assemble a resync repaint: clear the screen + scrollback and home, then the captured screen.
/// tmux joins captured lines with a bare LF, which only moves the cursor DOWN in a raw stream, so
/// re-anchor each line to column 0 (`\r\n`) — otherwise the repaint staircases to the right (the
/// same reason the TUI's `Emu::seed_capture` rewrites newlines).
fn resync_frame(content: &str, alternate: bool, cursor: Option<(u16, u16)>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(content.len() + 64);
    bytes.extend_from_slice(if alternate {
        b"\x1b[?1049h"
    } else {
        b"\x1b[?1049l"
    });
    bytes.extend_from_slice(b"\x1b[?25l\x1b[?7h\x1b[H\x1b[2J\x1b[3J");
    if alternate {
        // A full-screen capture represents cells, not a raw terminal transcript. Position each
        // row explicitly so wrapping or a grid-edge newline cannot shift the repaint.
        for (row, line) in content.lines().enumerate() {
            bytes.extend_from_slice(format!("\x1b[{};1H", row + 1).as_bytes());
            bytes.extend_from_slice(line.as_bytes());
        }
    } else {
        // Normal-screen captures may include shell history. Replaying rows sequentially seeds
        // xterm's local scrollback while CR anchors tmux's bare LF line separators.
        bytes.extend_from_slice(content.replace('\n', "\r\n").as_bytes());
    }
    if let Some((col, row)) = cursor {
        bytes.extend_from_slice(format!("\x1b[{};{}H\x1b[?25h", row + 1, col + 1).as_bytes());
    }
    bytes
}

struct Resync {
    cursor: StreamCursor,
    stable: bool,
}

async fn capture_resync(
    client: &DaemonClient,
    channel: &Channel<InvokeResponseBody>,
    lane_id: i64,
    window: &str,
) -> Option<Resync> {
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
    let Ok(value) = capture else { return None };
    let content = value.get("content").and_then(Value::as_str)?;
    let alternate = value.get("alternate").and_then(Value::as_bool)?;
    let cursor = value.get("cursor").and_then(|cursor| {
        Some((
            cursor.get("col")?.as_u64()? as u16,
            cursor.get("row")?.as_u64()? as u16,
        ))
    });
    let repaint_cursor = StreamCursor {
        generation: value.get("generation").and_then(Value::as_u64)?,
        sequence: value.get("sequence").and_then(Value::as_u64)?,
    };
    channel
        .send(InvokeResponseBody::Raw(resync_frame(
            content, alternate, cursor,
        )))
        .is_ok()
        .then_some(Resync {
            cursor: repaint_cursor,
            stable: value
                .get("stable")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
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

    ensure_demux(&state).await?;
    let client = state
        .client
        .get()
        .ok_or_else(RpcFailure::not_connected)?
        .clone();
    // Register this window's byte route (after the stale-watch teardown above, whose ack
    // guarantees the old watch's cleanup — including its route removal — already ran).
    let mut route_rx = {
        let mut routes = state.terminal_routes.lock().unwrap();
        routes
            .entry(window.clone())
            .or_insert_with(|| broadcast::channel(512).0)
            .subscribe()
    };
    let value = client
        .call(
            "agent.watch_bytes",
            Some(json!({ "lane_id": lane_id, "window": window, "on": true })),
        )
        .await
        .map_err(map_call_error)?;
    let ack = dimensions(&value);
    if ack.generation.is_none() || ack.sequence.is_none() {
        let _ = client
            .call(
                "agent.watch_bytes",
                Some(json!({ "lane_id": lane_id, "window": window, "on": false })),
            )
            .await;
        return Err(RpcFailure {
            code: -32012,
            message: "The running daemon is too old for reliable terminal rendering. Restart the Repomon daemon, then reopen this agent.".into(),
            data: None,
        });
    }

    // Start the stream first, then capture a sequenced checkpoint and ignore every queued chunk
    // already represented by that repaint. Unix needs this because pipe-pane is future-only;
    // Windows uses the same contract so a raced first replay frame cannot leave the pane blank.
    let Some(repaint) = capture_resync(&client, &on_bytes, lane_id, &window).await else {
        let _ = client
            .call(
                "agent.watch_bytes",
                Some(json!({ "lane_id": lane_id, "window": window, "on": false })),
            )
            .await;
        return Err(RpcFailure {
            code: -32000,
            message: "could not establish an authoritative terminal repaint".into(),
            data: None,
        });
    };
    let mut stream_cursor = repaint.cursor;
    let initial_resync = !repaint.stable;

    let (cancel_tx, mut cancel_rx) = oneshot::channel::<oneshot::Sender<()>>();
    state
        .terminal_watches
        .lock()
        .unwrap()
        .insert(window.clone(), cancel_tx);
    let watches = state.terminal_watches.clone();
    let routes = state.terminal_routes.clone();
    let task_window = window.clone();
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(FLUSH_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut pending = Vec::with_capacity(FLUSH_BYTES);
        let mut resync = initial_resync;
        let mut last_resync = std::time::Instant::now();
        let mut cancelled = None;

        loop {
            tokio::select! {
                ack = &mut cancel_rx => {
                    cancelled = ack.ok();
                    break;
                }
                frame = route_rx.recv() => match frame {
                    Ok(frame) => match frame.as_ref() {
                        RouteFrame::Lagged => resync = true,
                        RouteFrame::Chunk(chunk) => {
                            // Events queued before a repaint are already visible in it. A later
                            // generation or sequence gap means terminal-relative state is unsafe,
                            // so stop applying bytes until an authoritative repaint replaces it.
                            if chunk.cursor.generation == stream_cursor.generation
                                && chunk.cursor.sequence <= stream_cursor.sequence
                            {
                                continue;
                            }
                            let contiguous = chunk.cursor.generation == stream_cursor.generation
                                && chunk.cursor.sequence == stream_cursor.sequence + 1;
                            if !contiguous {
                                pending.clear();
                                resync = true;
                                continue;
                            }
                            stream_cursor = chunk.cursor;
                            let was_idle = pending.is_empty();
                            if !append_pending(&mut pending, &chunk.bytes) {
                                resync = true;
                            } else if (was_idle || pending.len() >= FLUSH_BYTES)
                                && !flush(&on_bytes, &mut pending)
                            {
                                // Leading edge: a chunk arriving on an idle pane (a keystroke
                                // echo) paints now instead of waiting out the 16ms ticker;
                                // chunks that arrive while data is already pending coalesce.
                                break;
                            }
                        }
                    },
                    Err(broadcast::error::RecvError::Lagged(_)) => resync = true,
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = ticker.tick() => {
                    if resync {
                        if last_resync.elapsed() < RESYNC_RETRY {
                            continue;
                        }
                        last_resync = std::time::Instant::now();
                        pending.clear();
                        let Some(repaint) =
                            capture_resync(&client, &on_bytes, lane_id, &task_window).await
                        else {
                            break;
                        };
                        stream_cursor = repaint.cursor;
                        // A continuously mutating pane may not yield a quiet checkpoint on the
                        // first pass. Render the latest complete frame now and verify it again on
                        // the next tick instead of mixing its bytes with an uncertain cursor.
                        resync = !repaint.stable;
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
        routes.lock().unwrap().remove(&window);
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

    use super::{MAX_PENDING, StreamCursor, append_pending, dimensions, event_chunk, resync_frame};

    #[test]
    fn resync_frame_reanchors_bare_newlines() {
        let frame = resync_frame("line1\nline2", true, Some((3, 4)));
        let text = String::from_utf8(frame).unwrap();
        assert!(text.starts_with("\x1b[?1049h\x1b[?25l\x1b[?7h\x1b[H\x1b[2J\x1b[3J"));
        assert!(text.contains("\x1b[1;1Hline1\x1b[2;1Hline2"));
        assert!(text.ends_with("\x1b[5;4H\x1b[?25h"));
    }

    #[test]
    fn resync_frame_restores_normal_screen_mode() {
        let frame = String::from_utf8(resync_frame("shell\nprompt", false, None)).unwrap();
        assert!(frame.starts_with("\x1b[?1049l\x1b[?25l\x1b[?7h\x1b[H\x1b[2J\x1b[3J"));
        assert!(frame.contains("shell\r\nprompt"));
    }

    #[test]
    fn event_chunk_decodes_and_names_the_window() {
        let bytes = b"\x1b[32mready\x1b[0m";
        // Missing generation/sequence: not routable.
        let event = Notification::new(
            "event.agent.bytes",
            json!({ "window": "lane-7", "data": STANDARD.encode(bytes) }),
        );
        assert!(event_chunk(&event).is_none());
        let event = Notification::new(
            "event.agent.bytes",
            json!({
                "window": "lane-7",
                "generation": 3,
                "sequence": 8,
                "data": STANDARD.encode(bytes)
            }),
        );
        let (window, chunk) = event_chunk(&event).unwrap();
        assert_eq!(window, "lane-7");
        assert_eq!(chunk.bytes, bytes);
        assert_eq!(
            chunk.cursor,
            StreamCursor {
                generation: 3,
                sequence: 8,
            }
        );
        // Non-bytes events route to the UI event channel, not a pane.
        let other = Notification::new("event.notification", json!({ "window": "lane-7" }));
        assert!(event_chunk(&other).is_none());
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
        let ack = dimensions(&json!({
            "cols": 120,
            "rows": 40,
            "generation": 7,
            "sequence": 11
        }));
        assert_eq!(ack.generation, Some(7));
        assert_eq!(ack.sequence, Some(11));
    }
}
