//! The local-IPC JSON-RPC server (a Unix domain socket on unix, a named pipe on Windows —
//! see `repomon_core::transport`).
//!
//! Each connection runs three cooperating tasks: a reader (so `read_frame`, which isn't
//! cancel-safe, always runs to completion), an event forwarder that drains the event bus, and a
//! single writer that owns the write half — so responses and pushed notifications are serialized
//! (never interleave mid-frame) but a slow RPC dispatch can never stall event draining, which is
//! what used to overflow the broadcast and drop `event.agent.bytes` (terminal glitches).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use repomon_core::protocol::{self, Request, Response, RpcError};
use repomon_core::transport::{self, Endpoint, IpcStream};
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::{Ctx, rpc};

/// How long the reader will wait on a silent socket before tearing itself down. A half-open client
/// (gone away but still holding the fd) otherwise parks the reader task forever, leaking one task
/// per reconnect. Generous — well past the TUI's 1s `lane.list` poll, so a healthy idle client is
/// never dropped; the connection task simply re-accepts on the next request.
const READ_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Bind the local IPC endpoint and serve until shutdown is requested. `socket_path` is the
/// socket file on unix and is interpreted as a named-pipe name on Windows (stale-file cleanup
/// and parent-dir creation happen inside `transport::listen`; pipes need neither).
pub async fn serve(ctx: Arc<Ctx>, socket_path: &Path) -> std::io::Result<()> {
    let endpoint = Endpoint::from_path(socket_path);
    let mut listener = transport::listen(&endpoint).await?;
    tracing::info!("listening on {}", socket_path.display());

    loop {
        tokio::select! {
            _ = ctx.shutdown.notified() => break,
            accepted = listener.accept() => match accepted {
                Ok(stream) => {
                    let ctx = ctx.clone();
                    tokio::spawn(async move { handle_conn(ctx, stream).await });
                }
                Err(e) => tracing::warn!("accept error: {e}"),
            },
        }
    }

    // Remove the socket file so the next daemon start binds cleanly (pipes vanish on close).
    #[cfg(unix)]
    let _ = std::fs::remove_file(socket_path);
    Ok(())
}

async fn handle_conn(ctx: Arc<Ctx>, stream: IpcStream) {
    let (mut read_half, mut write_half) = tokio::io::split(stream);

    // Reader task: read_frame to completion, hand frames to the connection task.
    let (in_tx, mut in_rx) = mpsc::channel::<Vec<u8>>(128);
    let reader_ctx = ctx.clone();
    tokio::spawn(async move {
        // `read_frame` isn't cancel-safe, so we only ever drop it *between* whole frames: the
        // select races the next frame against shutdown and an idle ceiling, both of which can only
        // fire while we're parked waiting for the first byte of a frame, never mid-frame.
        // Stops on clean EOF, read error, the connection task dropping the receiver, daemon
        // shutdown, or a client that goes silent while holding the socket half-open (idle timeout)
        // — without which a wedged client would park this task forever, leaking one per reconnect.
        loop {
            let frame = tokio::select! {
                _ = reader_ctx.shutdown.notified() => break,
                read = protocol::read_frame(&mut read_half) => match read {
                    Ok(Some(frame)) => frame,
                    _ => break, // clean EOF or read error
                },
                _ = tokio::time::sleep(READ_IDLE_TIMEOUT) => break,
            };
            if in_tx.send(frame).await.is_err() {
                break;
            }
        }
    });

    // This connection's per-device session (viewport/focus/fit state). The guard drops it from
    // `ctx.sessions` on every exit path below.
    let sess = ctx.open_session(crate::conn::ConnKind::Local).await;
    let _session_guard = crate::conn::SessionGuard::new(ctx.clone(), sess.id);

    // A single writer task owns the write half; both the RPC responder and the event forwarder hand
    // it already-serialized frames over this channel, so writes never interleave mid-frame and
    // neither side blocks the other on a slow write.
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(1024);
    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if protocol::write_frame(&mut write_half, &frame).await.is_err() {
                break;
            }
        }
    });

    // Off until a `subscribe` request turns it on.
    let forwarding = Arc::new(AtomicBool::new(false));

    // Event-forwarder task: drains the event bus PROMPTLY, independent of RPC dispatch. In the old
    // single-select loop a slow `lane.list` overlay (or a large response write) stalled event
    // draining, overflowing the broadcast and silently dropping `event.agent.bytes` — the terminal
    // dropped-characters glitch. A dedicated drainer keeps the bus empty so nothing is lost.
    let forwarder = {
        let mut events = ctx.events.subscribe();
        let forwarding = forwarding.clone();
        let sess = sess.clone();
        let out_tx = out_tx.clone();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(value) => {
                        if !forwarding.load(Ordering::Relaxed) {
                            continue;
                        }
                        // Per-connection filtering: `event.agent.bytes` reaches only the connections
                        // that watch its window, `event.agent.output` only those whose viewport
                        // covers its lane/window; every other topic forwards unchanged.
                        let deliver = {
                            let watched = sess.watched_bytes.lock().unwrap();
                            let out = sess.output_filter.lock().unwrap();
                            crate::pubsub::deliver_to(&value, &watched, &out.0, &out.1)
                        };
                        if deliver {
                            if let Ok(bytes) = serde_json::to_vec::<Value>(&value) {
                                if out_tx.send(bytes).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Err(RecvError::Lagged(n)) => tracing::debug!("subscriber lagged {n} events"),
                    Err(RecvError::Closed) => break,
                }
            }
        })
    };

    // RPC loop: dispatch requests and hand responses to the writer. Event forwarding runs in its own
    // task above, so a slow dispatch here no longer stalls it.
    while let Some(frame) = in_rx.recv().await {
        let req: Request = match serde_json::from_slice(&frame) {
            Ok(r) => r,
            Err(e) => {
                let resp = Response::err(None, RpcError::new(-32700, format!("parse error: {e}")));
                if out_tx.send(serde_json::to_vec(&resp).unwrap_or_default()).await.is_err() {
                    break;
                }
                continue;
            }
        };
        // A local request means a UI is actively watching; refresh the heartbeat (or zero it on the
        // explicit `watcher.park`) so the notification engine knows when to take over desktop popups.
        if req.method == "watcher.park" {
            *ctx.local_watcher_seen.lock().await = None;
        } else {
            *ctx.local_watcher_seen.lock().await = Some(std::time::Instant::now());
        }
        if req.method == "subscribe" {
            forwarding.store(true, Ordering::Relaxed);
        }
        let id = req.id;
        let resp = match rpc::dispatch(&ctx, &sess, &req.method, req.params).await {
            Ok(value) => Response::ok(id, value),
            Err(err) => Response::err(id, err),
        };
        if out_tx.send(serde_json::to_vec(&resp).unwrap_or_default()).await.is_err() {
            break;
        }
    }

    // Client gone: drop our writer handle so the writer task ends, stop the forwarder, and let the
    // writer flush what it has queued.
    drop(out_tx);
    forwarder.abort();
    let _ = writer.await;
}
