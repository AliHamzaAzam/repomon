//! The named-pipe control server (Windows only): accept loop, per-connection
//! request/response handling, and `subscribe_bytes` streaming (PROTOCOL.md §5).

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::broadcast;

use crate::codec::{FrameDecoder, encode_frame};
use crate::dacl::PipeSecurity;
use crate::dispatch::{Dispatcher, Effect};
use crate::protocol::StreamFrame;

/// Shared server context: the dispatcher, the PTY byte fan-out, and what to clean up on
/// `kill`.
pub struct ServerCtx {
    pub dispatcher: Mutex<Dispatcher>,
    pub bytes_tx: broadcast::Sender<Vec<u8>>,
    pub registry_path: std::path::PathBuf,
}

/// Creates a pipe with a per-user DACL, claiming the first instance exclusively to reject a
/// pre-bound name.
pub fn create_instance(
    pipe: &str,
    security: &PipeSecurity,
    first: bool,
) -> std::io::Result<NamedPipeServer> {
    let mut attrs = security.attributes();
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .create_with_security_attributes_raw(pipe, &mut attrs as *mut _ as *mut c_void)
    }
}

/// Accepts clients while keeping a spare pipe instance connectable, beginning with the instance
/// already registered by the caller.
pub async fn serve(
    pipe: String,
    security: PipeSecurity,
    first_instance: NamedPipeServer,
    ctx: Arc<ServerCtx>,
) -> anyhow::Result<()> {
    let mut instance = first_instance;
    loop {
        instance.connect().await?;
        let next = create_instance(&pipe, &security, false)?;
        let conn = std::mem::replace(&mut instance, next);
        let ctx = ctx.clone();
        tokio::spawn(async move {
            handle_conn(conn, ctx).await;
        });
    }
}

/// One client connection: request/response until disconnect, a corrupt frame, `kill`, or
/// an upgrade to stream mode.
async fn handle_conn(mut conn: NamedPipeServer, ctx: Arc<ServerCtx>) {
    let mut decoder = FrameDecoder::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match conn.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        decoder.extend(&buf[..n]);
        loop {
            let payload = match decoder.next_frame() {
                Ok(Some(p)) => p,
                Ok(None) => break,
                Err(_) => return, // corrupt peer: disconnect (PROTOCOL.md §4)
            };
            let (response, effect) = ctx
                .dispatcher
                .lock()
                .expect("dispatcher lock")
                .handle(&payload, epoch_now());
            if conn.write_all(&encode_frame(&response)).await.is_err() {
                return;
            }
            match effect {
                Effect::None => {}
                Effect::Shutdown => {
                    // kill: ok is on the wire; tear the window down like tmux kill-window.
                    let _ = conn.flush().await;
                    let _ = crate::registry::remove(&ctx.registry_path);
                    std::process::exit(0);
                }
                Effect::StartStream => {
                    stream_bytes(conn, ctx).await;
                    return;
                }
            }
        }
    }
}

/// Streams replay then live bytes, subscribing under the dispatcher lock so no output falls between
/// the snapshot and tail.
async fn stream_bytes(mut conn: NamedPipeServer, ctx: Arc<ServerCtx>) {
    let (replay, mut rx) = {
        let dispatcher = ctx.dispatcher.lock().expect("dispatcher lock");
        (dispatcher.replay(), ctx.bytes_tx.subscribe())
    };
    if send_stream_frame(&mut conn, &replay).await.is_err() {
        return;
    }
    loop {
        match rx.recv().await {
            Ok(chunk) => {
                if send_stream_frame(&mut conn, &chunk).await.is_err() {
                    return;
                }
            }
            // Lagged = we dropped chunks and the client's emulator would corrupt.
            // Disconnect; the client reconnects and gets a fresh replay.
            Err(broadcast::error::RecvError::Lagged(_))
            | Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

async fn send_stream_frame(conn: &mut NamedPipeServer, data: &[u8]) -> std::io::Result<()> {
    let frame = serde_json::to_vec(&StreamFrame::bytes(data)).expect("stream frame serializes");
    conn.write_all(&encode_frame(&frame)).await
}

/// Unix epoch seconds (`#{window_activity}` parity).
pub fn epoch_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
