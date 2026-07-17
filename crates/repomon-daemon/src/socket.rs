//! The Unix-domain-socket JSON-RPC server.
//!
//! Each connection runs a dedicated reader task (so `read_frame`, which isn't
//! cancel-safe, always runs to completion) that feeds incoming frames over an mpsc. The
//! connection task then `select!`s those requests against the event bus while exclusively
//! owning the write half — responses and pushed notifications never interleave mid-frame.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use repomon_core::protocol::{self, Request, Response, RpcError};
use serde_json::Value;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::{Ctx, rpc};

/// How long the reader will wait on a silent socket before tearing itself down. A half-open client
/// (gone away but still holding the fd) otherwise parks the reader task forever, leaking one task
/// per reconnect. Generous — well past the TUI's 1s `lane.list` poll, so a healthy idle client is
/// never dropped; the connection task simply re-accepts on the next request.
const READ_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Bind the socket and serve until shutdown is requested.
pub async fn serve(ctx: Arc<Ctx>, socket_path: &Path) -> std::io::Result<()> {
    if let Some(parent) = socket_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Clear a stale socket from a previous run.
    if socket_path.exists() {
        let _ = std::fs::remove_file(socket_path);
    }
    let listener = UnixListener::bind(socket_path)?;
    tracing::info!("listening on {}", socket_path.display());

    loop {
        tokio::select! {
            _ = ctx.shutdown.notified() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _addr)) => {
                    let ctx = ctx.clone();
                    tokio::spawn(async move { handle_conn(ctx, stream).await });
                }
                Err(e) => tracing::warn!("accept error: {e}"),
            },
        }
    }

    let _ = std::fs::remove_file(socket_path);
    Ok(())
}

async fn handle_conn(ctx: Arc<Ctx>, stream: UnixStream) {
    let (mut read_half, mut write_half) = stream.into_split();

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
    // `ctx.sessions` on every exit path below (each `break`, plus a panic).
    let sess = ctx.open_session(crate::conn::ConnKind::Local).await;
    let _session_guard = crate::conn::SessionGuard::new(ctx.clone(), sess.id);

    // Every connection holds an event receiver, but only forwards once subscribed.
    let mut events = ctx.events.subscribe();
    let mut forwarding = false;

    loop {
        tokio::select! {
            incoming = in_rx.recv() => {
                let Some(frame) = incoming else { break };
                let req: Request = match serde_json::from_slice(&frame) {
                    Ok(r) => r,
                    Err(e) => {
                        let resp = Response::err(None, RpcError::new(-32700, format!("parse error: {e}")));
                        if protocol::write_message(&mut write_half, &resp).await.is_err() {
                            break;
                        }
                        continue;
                    }
                };
                // A local request means the TUI is actively watching; its 1s lane.list refresh
                // keeps this fresh and stops the instant the TUI parks in an attach or closes —
                // which is how the notification engine knows to take over desktop popups.
                // `watcher.park` is the explicit "I'm parking now" signal: zero the heartbeat so
                // the daemon takes over on its very next tick instead of waiting out LOCAL_TTL.
                if req.method == "watcher.park" {
                    *ctx.local_watcher_seen.lock().await = None;
                } else {
                    *ctx.local_watcher_seen.lock().await = Some(std::time::Instant::now());
                }
                if req.method == "subscribe" {
                    forwarding = true;
                }
                let id = req.id;
                let resp = match rpc::dispatch(&ctx, &sess, &req.method, req.params).await {
                    Ok(value) => Response::ok(id, value),
                    Err(err) => Response::err(id, err),
                };
                if protocol::write_message(&mut write_half, &resp).await.is_err() {
                    break;
                }
            }
            event = events.recv() => match event {
                Ok(value) => {
                    // Per-connection filtering: `event.agent.bytes` reaches only the connections
                    // that watch its window (the pipe is shared and broadcast to all); every other
                    // topic forwards unchanged. Sync std-Mutex read — no await added on this path.
                    let deliver = {
                        let watched = sess.watched_bytes.lock().unwrap();
                        crate::pubsub::deliver_to(&value, &watched)
                    };
                    if forwarding && deliver {
                        if let Ok(bytes) = serde_json::to_vec::<Value>(&value) {
                            if protocol::write_frame(&mut write_half, &bytes).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    tracing::debug!("subscriber lagged {n} events");
                }
                Err(RecvError::Closed) => break,
            },
        }
    }
}
