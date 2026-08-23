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
const CHANNEL_BYTES: u8 = 0;
const CHANNEL_GRID: u8 = 1;

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

#[derive(Debug, PartialEq, Eq)]
pub struct GridChange {
    cursor: StreamCursor,
    cols: u16,
    rows: u16,
}

#[derive(Debug, PartialEq, Eq)]
pub struct StreamClosed {
    generation: u64,
}

impl StreamClosed {
    fn matches(&self, cursor: StreamCursor) -> bool {
        self.generation == cursor.generation
    }
}

/// One routed item on a window's terminal channel: ordered bytes/grid, target closure, or notice
/// that the upstream daemon subscription lagged (every receiver must resync — chunks were dropped).
#[derive(Debug, PartialEq, Eq)]
pub enum RouteFrame {
    Chunk(ByteChunk),
    Grid(GridChange),
    Closed(StreamClosed),
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
    let bytes = STANDARD.decode(encoded).ok()?;
    log_term_trace("DAEMON_CHUNK", window, Some(sequence), &bytes);
    Some((
        window.to_string(),
        ByteChunk {
            cursor: StreamCursor {
                generation,
                sequence,
            },
            bytes,
        },
    ))
}

fn event_grid(event: &Notification) -> Option<(String, GridChange)> {
    if event.method != "event.agent.grid" {
        return None;
    }
    let window = event.params.get("window").and_then(Value::as_str)?;
    let generation = event.params.get("generation").and_then(Value::as_u64)?;
    let sequence = event.params.get("sequence").and_then(Value::as_u64)?;
    let cols = event.params.get("cols").and_then(Value::as_u64)? as u16;
    let rows = event.params.get("rows").and_then(Value::as_u64)? as u16;
    Some((
        window.to_string(),
        GridChange {
            cursor: StreamCursor {
                generation,
                sequence,
            },
            cols,
            rows,
        },
    ))
}

fn event_stream_closed(event: &Notification) -> Option<(String, StreamClosed)> {
    if event.method != "event.agent.stream_closed" {
        return None;
    }
    let window = event.params.get("window").and_then(Value::as_str)?;
    let generation = event.params.get("generation").and_then(Value::as_u64)?;
    Some((window.to_string(), StreamClosed { generation }))
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
            client
                .call("subscribe", None)
                .await
                .map_err(map_call_error)?;
            tauri::async_runtime::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(event) => {
                            if let Some((window, chunk)) = event_chunk(&event) {
                                let tx = routes.lock().unwrap().get(&window).cloned();
                                if let Some(tx) = tx {
                                    let _ = tx.send(Arc::new(RouteFrame::Chunk(chunk)));
                                }
                            } else if let Some((window, grid)) = event_grid(&event) {
                                let tx = routes.lock().unwrap().get(&window).cloned();
                                if let Some(tx) = tx {
                                    let _ = tx.send(Arc::new(RouteFrame::Grid(grid)));
                                }
                                // Non-terminal consumers still receive the additive grid event.
                                let _ = ui_events.send(event);
                            } else if let Some((window, closed)) = event_stream_closed(&event) {
                                let tx = routes.lock().unwrap().get(&window).cloned();
                                if let Some(tx) = tx {
                                    let _ = tx.send(Arc::new(RouteFrame::Closed(closed)));
                                }
                                let _ = ui_events.send(event);
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

fn log_term_trace(tag: &str, window: &str, seq: Option<u64>, bytes: &[u8]) {
    use std::io::Write;
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/repomon-terminal-trace.log")
    else {
        return;
    };
    let preview: String = bytes
        .iter()
        .take(128)
        .map(|&b| match b {
            0x1b => "\\e".to_string(),
            b'\r' => "\\r".to_string(),
            b'\n' => "\\n".to_string(),
            b'\t' => "\\t".to_string(),
            32..=126 => (b as char).to_string(),
            _ => format!("\\x{:02x}", b),
        })
        .collect();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let _ = writeln!(
        file,
        "[{ts}] [{tag}] win={window} seq={seq:?} len={} preview={preview}",
        bytes.len()
    );
}

fn flush(channel: &Channel<InvokeResponseBody>, pending: &mut Vec<u8>) -> bool {
    if pending.is_empty() {
        return true;
    }
    // Swap in a pre-sized buffer: `mem::take` would leave capacity 0 and re-grow every tick.
    let bytes = std::mem::replace(pending, Vec::with_capacity(FLUSH_BYTES));
    log_term_trace("FLUSH_TO_TAURI", "term", None, &bytes);
    send_bytes(channel, bytes)
}

fn send_bytes(channel: &Channel<InvokeResponseBody>, bytes: Vec<u8>) -> bool {
    let mut frame = Vec::with_capacity(bytes.len() + 1);
    frame.push(CHANNEL_BYTES);
    frame.extend_from_slice(&bytes);
    channel.send(InvokeResponseBody::Raw(frame)).is_ok()
}

fn send_grid(channel: &Channel<InvokeResponseBody>, cols: u16, rows: u16) -> bool {
    let mut frame = Vec::with_capacity(5);
    frame.push(CHANNEL_GRID);
    frame.extend_from_slice(&cols.to_be_bytes());
    frame.extend_from_slice(&rows.to_be_bytes());
    channel.send(InvokeResponseBody::Raw(frame)).is_ok()
}

/// Assemble a resync repaint: clear the visible screen and position each captured row explicitly.
/// Explicit row positioning ensures that wrapped lines or grid-edge newlines never shift the
/// viewport or desynchronize cursor coordinates.
fn resync_frame(content: &str, alternate: bool, cursor: Option<(u16, u16)>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(content.len() + 64);
    bytes.extend_from_slice(if alternate {
        b"\x1b[?1049h"
    } else {
        b"\x1b[?1049l"
    });
    bytes.extend_from_slice(b"\x1b[?25l\x1b[?7h\x1b[H\x1b[2J");
    for (row, line) in content.lines().enumerate() {
        bytes.extend_from_slice(format!("\x1b[{};1H\x1b[2K", row + 1).as_bytes());
        bytes.extend_from_slice(line.as_bytes());
    }
    if let Some((col, row)) = cursor {
        bytes.extend_from_slice(format!("\x1b[{};{}H\x1b[?25h", row + 1, col + 1).as_bytes());
    }
    bytes
}

struct Resync {
    cursor: StreamCursor,
}

/// Total time `capture_resync` will spend waiting for a *stable* capture before falling back to
/// the best-effort one it already has. A freshly spawned TUI (e.g. codex booting its own screen)
/// can legitimately stay unstable well past the old ~200ms budget; failing the whole watch over
/// that is worse than briefly painting a torn frame, since the live byte stream self-heals it
/// within a frame or two.
const RESYNC_DEADLINE: Duration = Duration::from_millis(2500);
/// Backoff between resync poll attempts: starts fast for the common already-stable case, backs
/// off so a genuinely slow-to-settle pane isn't hammered with capture calls for 2.5s straight.
const RESYNC_POLL_START: Duration = Duration::from_millis(20);
const RESYNC_POLL_MAX: Duration = Duration::from_millis(200);

struct ParsedResync {
    cols: u16,
    rows: u16,
    frame_bytes: Vec<u8>,
    cursor: StreamCursor,
}

/// Parse one `agent.capture` response into repaint-ready fields, whether or not it reported
/// `stable`. Pure and channel-free so it is unit-testable without a live Tauri IPC channel.
fn parse_resync(value: &Value) -> Option<ParsedResync> {
    let content = value.get("content").and_then(Value::as_str)?;
    let alternate = value.get("alternate").and_then(Value::as_bool)?;
    let cols = value.get("cols").and_then(Value::as_u64)? as u16;
    let rows = value.get("rows").and_then(Value::as_u64)? as u16;
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
    let frame_bytes = resync_frame(content, alternate, cursor);
    Some(ParsedResync {
        cols,
        rows,
        frame_bytes,
        cursor: repaint_cursor,
    })
}

/// Build a `Resync` from one `agent.capture` response, whether or not it reported `stable`.
fn resync_from_capture(
    channel: &Channel<InvokeResponseBody>,
    window: &str,
    value: &Value,
) -> Option<Resync> {
    let parsed = parse_resync(value)?;
    log_term_trace(
        "RESYNC_FRAME",
        window,
        Some(parsed.cursor.sequence),
        &parsed.frame_bytes,
    );
    if !send_grid(channel, parsed.cols, parsed.rows) || !send_bytes(channel, parsed.frame_bytes) {
        return None;
    }
    Some(Resync {
        cursor: parsed.cursor,
    })
}

async fn capture_resync(
    client: &DaemonClient,
    channel: &Channel<InvokeResponseBody>,
    lane_id: i64,
    window: &str,
) -> Option<Resync> {
    let start = tokio::time::Instant::now();
    let mut poll_delay = RESYNC_POLL_START;
    let mut last_unstable: Option<Value> = None;
    loop {
        let capture = client
            .call(
                "agent.capture",
                Some(json!({
                    "lane_id": lane_id,
                    "window": window,
                    "include_state": true
                })),
            )
            .await;
        match capture {
            Ok(value) if value.get("stable").and_then(Value::as_bool) == Some(true) => {
                return resync_from_capture(channel, window, &value);
            }
            Ok(value) => last_unstable = Some(value),
            // A transient RPC error is retried within the same deadline rather than aborting the
            // whole watch on the first hiccup.
            Err(_) => {}
        }
        if start.elapsed() >= RESYNC_DEADLINE {
            // Best-effort: an unstable-but-parseable capture beats a dead pane. Its cursor still
            // carries a valid generation/sequence, so the live stream picks up from it correctly.
            return last_unstable
                .as_ref()
                .and_then(|value| resync_from_capture(channel, window, value));
        }
        tokio::time::sleep(poll_delay).await;
        poll_delay = (poll_delay * 2).min(RESYNC_POLL_MAX);
    }
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
    let value = match client
        .call(
            "agent.watch_bytes",
            Some(json!({ "lane_id": lane_id, "window": window, "on": true })),
        )
        .await
    {
        Ok(value) => value,
        Err(error) => {
            state.terminal_routes.lock().unwrap().remove(&window);
            return Err(map_call_error(error));
        }
    };
    let ack = dimensions(&value);
    if ack.generation.is_none() || ack.sequence.is_none() {
        let _ = client
            .call(
                "agent.watch_bytes",
                Some(json!({ "lane_id": lane_id, "window": window, "on": false })),
            )
            .await;
        state.terminal_routes.lock().unwrap().remove(&window);
        return Err(RpcFailure {
            code: -32012,
            message: "The running daemon is too old for reliable terminal rendering. Restart the Repomon daemon, then reopen this agent.".into(),
            data: None,
        });
    }

    // Start the stream first, then capture a sequenced checkpoint and ignore every queued chunk
    // already represented by that repaint. Unix needs this because control-mode output is future-only;
    // Windows uses the same contract so a raced first replay frame cannot leave the pane blank.
    let Some(repaint) = capture_resync(&client, &on_bytes, lane_id, &window).await else {
        let _ = client
            .call(
                "agent.watch_bytes",
                Some(json!({ "lane_id": lane_id, "window": window, "on": false })),
            )
            .await;
        state.terminal_routes.lock().unwrap().remove(&window);
        return Err(RpcFailure {
            code: -32000,
            message: "could not establish an authoritative terminal repaint".into(),
            data: None,
        });
    };
    let mut stream_cursor = repaint.cursor;
    let initial_resync = false;

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
                        RouteFrame::Closed(closed) => {
                            if closed.matches(stream_cursor) {
                                break;
                            }
                        }
                        RouteFrame::Grid(grid) => {
                            let contiguous = grid.cursor.generation == stream_cursor.generation
                                && grid.cursor.sequence == stream_cursor.sequence + 1;
                            if !contiguous {
                                pending.clear();
                                resync = true;
                                continue;
                            }
                            if !flush(&on_bytes, &mut pending)
                                || !send_grid(&on_bytes, grid.cols, grid.rows)
                            {
                                break;
                            }
                            stream_cursor = grid.cursor;
                            resync = false;
                        }
                        RouteFrame::Chunk(chunk) => {
                            // Events queued before a repaint are already visible in it. A later
                            // generation or sequence gap means terminal-relative state is unsafe,
                            // so stop applying bytes until an authoritative repaint replaces it.
                            if chunk.cursor.generation == stream_cursor.generation
                                && chunk.cursor.sequence <= stream_cursor.sequence
                            {
                                log_term_trace(
                                    "DROPPED_STALE_CHUNK",
                                    &task_window,
                                    Some(chunk.cursor.sequence),
                                    &chunk.bytes,
                                );
                                continue;
                            }
                            let contiguous = chunk.cursor.generation == stream_cursor.generation
                                && chunk.cursor.sequence == stream_cursor.sequence + 1;
                            if !contiguous {
                                pending.clear();
                                resync = true;
                                continue;
                            }
                            resync = false;
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
                        resync = false;
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

    use super::{
        MAX_PENDING, StreamCursor, append_pending, dimensions, event_chunk, event_grid,
        event_stream_closed, parse_resync, resync_frame,
    };

    #[test]
    fn parse_resync_accepts_an_unstable_capture() {
        // The best-effort fallback path: a capture that never reported `stable: true` must still
        // parse into a paintable frame with a valid cursor, since it's the only data a booting
        // agent's pane has given us before the resync deadline runs out.
        let value = json!({
            "stable": false,
            "content": "booting…",
            "alternate": false,
            "cols": 80,
            "rows": 24,
            "cursor": { "col": 2, "row": 0 },
            "generation": 5,
            "sequence": 12,
        });
        let parsed = parse_resync(&value).expect("unstable capture should still parse");
        assert_eq!(parsed.cols, 80);
        assert_eq!(parsed.rows, 24);
        assert_eq!(
            parsed.cursor,
            StreamCursor {
                generation: 5,
                sequence: 12,
            }
        );
        assert!(!parsed.frame_bytes.is_empty());
    }

    #[test]
    fn parse_resync_rejects_a_capture_missing_the_repaint_cursor() {
        // Without generation/sequence there is nothing for the live byte stream to reconcile
        // against, so this must stay None rather than emit an unroutable frame.
        let value = json!({
            "stable": false,
            "content": "line",
            "alternate": false,
            "cols": 80,
            "rows": 24,
        });
        assert!(parse_resync(&value).is_none());
    }

    #[test]
    fn resync_frame_positions_rows_explicitly() {
        let frame = resync_frame("line1\nline2", true, Some((3, 4)));
        let text = String::from_utf8(frame).unwrap();
        assert!(text.starts_with("\x1b[?1049h\x1b[?25l\x1b[?7h\x1b[H\x1b[2J"));
        assert!(text.contains("\x1b[1;1H\x1b[2Kline1\x1b[2;1H\x1b[2Kline2"));
        assert!(text.ends_with("\x1b[5;4H\x1b[?25h"));
    }

    #[test]
    fn resync_frame_restores_normal_screen_mode() {
        let frame = String::from_utf8(resync_frame("shell\nprompt", false, None)).unwrap();
        assert!(frame.starts_with("\x1b[?1049l\x1b[?25l\x1b[?7h\x1b[H\x1b[2J"));
        assert!(frame.contains("\x1b[1;1H\x1b[2Kshell\x1b[2;1H\x1b[2Kprompt"));
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
    fn event_grid_requires_an_ordered_stream_cursor() {
        let ordered = Notification::new(
            "event.agent.grid",
            json!({
                "window": "lane-7",
                "generation": 3,
                "sequence": 9,
                "cols": 120,
                "rows": 40
            }),
        );
        let (window, grid) = event_grid(&ordered).unwrap();
        assert_eq!(window, "lane-7");
        assert_eq!(
            grid.cursor,
            StreamCursor {
                generation: 3,
                sequence: 9
            }
        );
        assert_eq!((grid.cols, grid.rows), (120, 40));

        let unsequenced = Notification::new(
            "event.agent.grid",
            json!({ "window": "lane-7", "cols": 120, "rows": 40 }),
        );
        assert!(event_grid(&unsequenced).is_none());
    }

    #[test]
    fn stream_close_names_the_window_and_generation() {
        let closed = Notification::new(
            "event.agent.stream_closed",
            json!({ "window": "lane-7", "generation": 3 }),
        );
        let (window, closed) = event_stream_closed(&closed).unwrap();
        assert_eq!(window, "lane-7");
        assert_eq!(closed.generation, 3);
        assert!(closed.matches(StreamCursor {
            generation: 3,
            sequence: 99,
        }));
        assert!(!closed.matches(StreamCursor {
            generation: 4,
            sequence: 0,
        }));

        let missing_generation =
            Notification::new("event.agent.stream_closed", json!({ "window": "lane-7" }));
        assert!(event_stream_closed(&missing_generation).is_none());
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
