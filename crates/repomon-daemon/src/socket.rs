//! Serves local framed JSON-RPC over Unix sockets or Windows pipes. Independent reading and event
//! forwarding keep slow RPCs from blocking terminal events; one writer serializes complete frames.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use repomon_core::protocol::{self, Request, Response, RpcError};
use repomon_core::transport::{self, Endpoint, IpcListener, IpcStream};
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::{Ctx, rpc};

/// Bounds silent-reader lifetime so abandoned connections cannot accumulate tasks.
const READ_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Bind the local IPC endpoint and serve until shutdown is requested. `socket_path` is the
/// socket file on unix and is interpreted as a named-pipe name on Windows (stale-file cleanup
/// and parent-dir creation happen inside `transport::listen`; pipes need neither).
pub async fn serve(ctx: Arc<Ctx>, socket_path: &Path) -> std::io::Result<()> {
    let endpoint = Endpoint::from_path(socket_path);
    let listener = transport::listen(&endpoint).await?;
    serve_listener(ctx, socket_path, listener).await
}

/// Serves an already-bound IPC listener until shutdown, removing its Unix socket file on exit.
pub async fn serve_listener(
    ctx: Arc<Ctx>,
    socket_path: &Path,
    listener: IpcListener,
) -> std::io::Result<()> {
    serve_with_watchdog(ctx, socket_path, listener, Duration::from_secs(600)).await
}

async fn serve_with_watchdog(
    ctx: Arc<Ctx>,
    socket_path: &Path,
    mut listener: IpcListener,
    interval: Duration,
) -> std::io::Result<()> {
    tracing::info!("listening on {}", socket_path.display());

    #[cfg(unix)]
    let mut identity = listener.bound_identity();
    let mut watchdog = tokio::time::interval(interval);
    watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut touched = std::time::Instant::now();
    loop {
        tokio::select! {
            _ = watchdog.tick() => {
                #[cfg(unix)]
                match rebind_if_lost(socket_path, &mut listener, &mut identity).await {
                    Ok(true) => ctx.broadcast("event.daemon.rebound", serde_json::json!({ "socket": socket_path })),
                    Ok(false) => {},
                    Err(error) => tracing::error!(%error, "daemon socket recovery failed; will retry"),
                }
                let backend = ctx.backend.clone();
                tokio::task::spawn_blocking(move || {
                    if let Err(error) = backend.maintain_socket() {
                        tracing::error!(%error, "tmux socket watchdog failed; fleet replacement remains blocked");
                    }
                });
                if touched.elapsed() >= Duration::from_secs(86400) {
                    if let Err(error) = repomon_core::agent::tmux_socket::touch_socket(socket_path) {
                        tracing::warn!(%error, "could not refresh daemon socket timestamps");
                    }
                    touched = std::time::Instant::now();
                }
            },
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
    if socket_identity(socket_path).ok() == Some(identity) {
        let _ = std::fs::remove_file(socket_path);
    }
    Ok(())
}

#[cfg(unix)]
fn socket_identity(path: &Path) -> std::io::Result<(u64, u64)> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket() {
        return Err(std::io::Error::other("listener path is not a socket"));
    }
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(unix)]
async fn rebind_if_lost(
    path: &Path,
    listener: &mut IpcListener,
    identity: &mut (u64, u64),
) -> std::io::Result<bool> {
    if socket_identity(path).ok() == Some(*identity) {
        return Ok(false);
    }
    tracing::error!(path = %path.display(), "daemon listener path was removed or replaced; rebinding");
    // listen refuses to unlink a live replacement; established connections keep their streams.
    let replacement = transport::listen(&Endpoint::from_path(path)).await?;
    *identity = replacement.bound_identity();
    *listener = replacement;
    Ok(true)
}

async fn handle_conn(ctx: Arc<Ctx>, stream: IpcStream) {
    let (mut read_half, mut write_half) = tokio::io::split(stream);

    // Reader task: read_frame to completion, hand frames to the connection task.
    let (in_tx, mut in_rx) = mpsc::channel::<Vec<u8>>(128);
    let reader_ctx = ctx.clone();
    tokio::spawn(async move {
        // Cancellation may interrupt a partial frame, so shutdown or timeout must abandon this
        // reader permanently rather than resume parsing the same stream.
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
            if protocol::write_frame(&mut write_half, &frame)
                .await
                .is_err()
            {
                break;
            }
        }
    });

    let forwarding = Arc::new(AtomicBool::new(false));

    // Drain events independently of RPC dispatch so slow overlays or response writes do not
    // overflow the broadcast buffer.
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
                            crate::transcript::deliver_to(&value, sess.id)
                                && crate::pubsub::deliver_to(&value, &watched, &out.0, &out.1)
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

    while let Some(frame) = in_rx.recv().await {
        let req: Request = match serde_json::from_slice(&frame) {
            Ok(r) => r,
            Err(e) => {
                let resp = Response::err(None, RpcError::new(-32700, format!("parse error: {e}")));
                if out_tx
                    .send(serde_json::to_vec(&resp).unwrap_or_default())
                    .await
                    .is_err()
                {
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
        if out_tx
            .send(serde_json::to_vec(&resp).unwrap_or_default())
            .await
            .is_err()
        {
            break;
        }
    }

    // Client gone: drop our writer handle so the writer task ends, stop the forwarder, and let the
    // writer flush what it has queued.
    drop(out_tx);
    forwarder.abort();
    let _ = writer.await;
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn running_daemon_rebinds_and_broadcasts_without_dropping_existing_stream() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.sock");
        let mut listener = transport::listen(&Endpoint::from_path(&path))
            .await
            .unwrap();
        // Accepted streams belong to connections, not the listener, and must survive its replacement.
        let mut old_client = tokio::net::UnixStream::connect(&path).await.unwrap();
        let mut old_server = listener.accept().await.unwrap();
        let config = repomon_core::Config {
            tmux_session: format!("i1-watchdog-{}", std::process::id()),
            ..Default::default()
        };
        let ctx = Ctx::new_with_paths(
            repomon_core::Store::open_in_memory().unwrap(),
            config,
            None,
            dir.path().join("config"),
            dir.path().join("notes"),
        );
        let mut events = ctx.events.subscribe();
        // Unlink even before serving begins to prove we use the original bound identity.
        std::fs::remove_file(&path).unwrap();
        let task = tokio::spawn({
            let ctx = ctx.clone();
            let path = path.clone();
            async move { serve_with_watchdog(ctx, &path, listener, Duration::from_millis(10)).await }
        });
        let event = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event["method"], "event.daemon.rebound");
        assert_eq!(event["params"]["socket"], path.to_string_lossy().as_ref());
        let _new_client = tokio::net::UnixStream::connect(&path).await.unwrap();
        old_client.write_all(b"alive").await.unwrap();
        let mut data = [0; 5];
        old_server.read_exact(&mut data).await.unwrap();
        assert_eq!(&data, b"alive");
        ctx.shutdown.notify_waiters();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn watchdog_recovers_unlinked_listener_and_preserves_live_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.sock");
        let mut listener = transport::listen(&Endpoint::from_path(&path))
            .await
            .unwrap();
        let mut identity = socket_identity(&path).unwrap();
        let original = identity;
        std::fs::remove_file(&path).unwrap();
        assert!(
            rebind_if_lost(&path, &mut listener, &mut identity)
                .await
                .unwrap()
        );
        assert_ne!(identity, original);
        let _client = tokio::net::UnixStream::connect(&path).await.unwrap();
        listener.accept().await.unwrap();
        assert!(
            !rebind_if_lost(&path, &mut listener, &mut identity)
                .await
                .unwrap()
        );
        std::fs::remove_file(&path).unwrap();
        let other = tokio::net::UnixListener::bind(&path).unwrap();
        let other_identity = socket_identity(&path).unwrap();
        assert!(
            rebind_if_lost(&path, &mut listener, &mut identity)
                .await
                .is_err()
        );
        assert_eq!(socket_identity(&path).unwrap(), other_identity);
        drop(other);
    }
}
