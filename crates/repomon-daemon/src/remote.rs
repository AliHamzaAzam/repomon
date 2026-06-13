//! The remote-access WebSocket server — how companion apps (iOS) reach the daemon.
//!
//! Speaks the exact same JSON-RPC protocol as the Unix socket (`socket.rs`), with WebSocket
//! text frames replacing the 4-byte length prefix: one frame = one envelope. Auth is a bearer
//! token checked **before** the WebSocket upgrade completes (`Authorization: Bearer …` header,
//! or `?token=…` for clients that can't set headers); a bad token is rejected with 401 and no
//! connection state. Bind this to a private address — typically the machine's Tailscale IP —
//! never the open internet.

use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use repomon_core::protocol::{Request, Response, RpcError};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast::error::RecvError;
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HsRequest, Response as HsResponse,
};
use tokio_tungstenite::tungstenite::Message;

use crate::{rpc, Ctx};

/// Bind the WebSocket bridge and serve until shutdown is requested.
pub async fn serve_remote(ctx: Arc<Ctx>, bind: &str, token: String) -> std::io::Result<()> {
    let listener = TcpListener::bind(bind).await?;
    tracing::info!("remote bridge listening on ws://{bind}");
    let token = Arc::new(token);

    loop {
        tokio::select! {
            _ = ctx.shutdown.notified() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, addr)) => {
                    let ctx = ctx.clone();
                    let token = token.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_conn(ctx, stream, &token).await {
                            tracing::debug!("remote conn {addr}: {e}");
                        }
                    });
                }
                Err(e) => tracing::warn!("remote accept error: {e}"),
            },
        }
    }
    Ok(())
}

// `result_large_err`: the auth closure's Err type (a full http::Response) is dictated by
// tungstenite's `Callback` contract — nothing to box here.
#[allow(clippy::result_large_err)]
async fn handle_conn(
    ctx: Arc<Ctx>,
    stream: TcpStream,
    token: &str,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    // Check the token during the handshake — an unauthorized client never completes the
    // upgrade and learns nothing but "401".
    let ws =
        tokio_tungstenite::accept_hdr_async(stream, |req: &HsRequest, resp| auth(req, resp, token))
            .await?;
    let (mut sink, mut source) = ws.split();

    // Every connection holds an event receiver, but only forwards once subscribed —
    // mirroring the Unix-socket connection loop.
    let mut events = ctx.events.subscribe();
    let mut forwarding = false;

    loop {
        tokio::select! {
            incoming = source.next() => {
                let msg = match incoming {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => return Err(e),
                    None => break,
                };
                let text = match msg {
                    Message::Text(t) => t,
                    Message::Close(_) => break,
                    // Ping/pong are answered by tungstenite itself; ignore binary frames.
                    _ => continue,
                };
                let req: Request = match serde_json::from_str(&text) {
                    Ok(r) => r,
                    Err(e) => {
                        let resp = Response::err(None, RpcError::new(-32700, format!("parse error: {e}")));
                        send_json(&mut sink, &resp).await?;
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
                send_json(&mut sink, &resp).await?;
            }
            event = events.recv() => match event {
                Ok(value) => {
                    if forwarding {
                        send_json(&mut sink, &value).await?;
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    tracing::debug!("remote subscriber lagged {n} events");
                }
                Err(RecvError::Closed) => break,
            },
        }
    }
    Ok(())
}

async fn send_json<S, T>(
    sink: &mut S,
    value: &T,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
    T: serde::Serialize,
{
    let text = serde_json::to_string(value).unwrap_or_default();
    sink.send(Message::text(text)).await
}

/// The handshake gatekeeper: pass the upgrade through when the token matches, else 401.
#[allow(clippy::result_large_err)] // the signature is tungstenite's Callback contract
fn auth(req: &HsRequest, resp: HsResponse, token: &str) -> Result<HsResponse, ErrorResponse> {
    if request_authorized(req, token) {
        Ok(resp)
    } else {
        let mut deny = ErrorResponse::new(Some("unauthorized".into()));
        *deny.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED;
        Err(deny)
    }
}

/// Whether the handshake request carries the right token: `Authorization: Bearer <token>` or a
/// `token=<token>` query parameter (for clients that can't set headers on a WS dial).
fn request_authorized(req: &HsRequest, token: &str) -> bool {
    let presented = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
        .or_else(|| {
            req.uri().query().and_then(|q| {
                q.split('&')
                    .find_map(|kv| kv.strip_prefix("token=").map(str::to_string))
            })
        });
    match presented {
        Some(p) => constant_time_eq(p.as_bytes(), token.as_bytes()),
        None => false,
    }
}

/// Compare two byte strings without early exit on the first mismatch (timing side channel).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_basics() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secrex"));
        assert!(!constant_time_eq(b"secret", b"secret1"));
        assert!(constant_time_eq(b"", b""));
    }
}
