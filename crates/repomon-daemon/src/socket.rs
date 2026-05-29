//! The Unix-domain-socket JSON-RPC server.
//!
//! Each connection runs a dedicated reader task (so `read_frame`, which isn't
//! cancel-safe, always runs to completion) that feeds incoming frames over an mpsc. The
//! connection task then `select!`s those requests against the event bus while exclusively
//! owning the write half — responses and pushed notifications never interleave mid-frame.

use std::path::Path;
use std::sync::Arc;

use repomon_core::protocol::{self, Request, Response, RpcError};
use serde_json::Value;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::{rpc, Ctx};

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
    tokio::spawn(async move {
        // Stops on clean EOF, read error, or when the connection task drops the receiver.
        while let Ok(Some(frame)) = protocol::read_frame(&mut read_half).await {
            if in_tx.send(frame).await.is_err() {
                break;
            }
        }
    });

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
                if req.method == "subscribe" {
                    forwarding = true;
                }
                let id = req.id;
                let resp = match rpc::dispatch(&ctx, &req.method, req.params).await {
                    Ok(value) => Response::ok(id, value),
                    Err(err) => Response::err(id, err),
                };
                if protocol::write_message(&mut write_half, &resp).await.is_err() {
                    break;
                }
            }
            event = events.recv() => match event {
                Ok(value) => {
                    if forwarding {
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
