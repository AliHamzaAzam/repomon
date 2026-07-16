//! The remote-access WebSocket server — how companion apps (iOS) reach the daemon.
//!
//! Speaks the exact same JSON-RPC protocol as the Unix socket (`socket.rs`), with WebSocket
//! text frames replacing the 4-byte length prefix: one frame = one envelope. Auth is a bearer
//! token checked **before** the WebSocket upgrade completes (`Authorization: Bearer …` header,
//! or `?token=…` for clients that can't set headers); a bad token is rejected with 401 and no
//! connection state. Bind this to a private address — typically the machine's Tailscale IP —
//! never the open internet.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::{SinkExt, StreamExt};
use repomon_core::protocol::{MAX_FRAME_BYTES, Request, Response, RpcError};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast::error::RecvError;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HsRequest, Response as HsResponse,
};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::{Ctx, rpc};

/// Max concurrent remote connections — a coarse DoS backstop (auth precedes the upgrade, so this
/// only bounds authenticated clients/reconnect churn).
const MAX_REMOTE_CONNS: usize = 64;

/// WebSocket frame/message limits for the bridge. Matches the Unix socket's `MAX_FRAME_BYTES` so a
/// large `agent.capture` isn't truncated, but is set explicitly rather than left to tungstenite's
/// default (which gave the remote path no stated bound).
fn remote_ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES))
}

/// Decrements the live-connection counter when a handler task ends.
struct ConnGuard(Arc<AtomicUsize>);
impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Methods the remote WebSocket bridge may invoke. **Default-deny**: anything not listed here
/// (including any future RPC) is rejected over the network, so the bridge can't be used to reach
/// past what's listed. Paired devices get full fleet control: read the fleet, drive existing
/// agents, and now spawn/stop/adopt agents and create/delete/merge lanes too. Still blocked:
/// daemon lifecycle (`daemon.shutdown`), config/secrets (`config.get` can carry the remote token,
/// `config.set`), host terminal + filesystem access (`terminal.open/close/target`, `fs.browse`),
/// and credential minting (`remote.*`, local-only). The local Unix socket is unaffected.
fn remote_method_allowed(method: &str) -> bool {
    matches!(
        method,
        // health check
        "ping"
        // reads
        | "repo.list" | "lane.list" | "lane.get"
        | "commit.today" | "commit.range" | "commit.search" | "commit.recent"
        | "agent.capture" | "agent.transcript"
        | "usage.get" | "daemon.status"
        // terminal-window *names* only ({lane_id, id} pairs) — open/close/target stay blocked
        | "terminal.list_all"
        // event stream + per-client streaming hint
        | "subscribe" | "viewport.set"
        // drive an existing agent. agent.prompt is a read (fresh pane capture parsed for the
        // on-screen dialog); agent.answer is strictly safer than the already-allowed blind
        // agent.key — it re-captures and verifies the dialog before steering; agent.watch_bytes
        // is a read-only byte stream of a pane the client could already agent.capture.
        // agent.fit is the ONLY remote door to pane sizing: it reflows the shared pane to the
        // caller's grid only while no live TUI viewport owns the window, and always answers
        // with the authoritative grid. The blind agent.resize stays local-only — an
        // unconditional remote resize is exactly what squeezed the TUI's mediated view.
        | "agent.send_input" | "agent.signal" | "agent.key" | "agent.scroll"
        | "agent.target" | "agent.fit"
        | "agent.prompt" | "agent.answer" | "agent.watch_bytes"
        // full fleet control: spawn/stop/adopt an agent, and manage the lanes they run in.
        // agent.detect is a read (the spawn sheet's agent picker) — it's the only remote door to
        // the configured agent list, since config.get stays blocked. lane.create's `path` param
        // is optional, so no fs.browse is needed; the repo picker is the already-allowed
        // repo.list.
        | "agent.spawn" | "agent.stop" | "agent.adopt" | "agent.detect"
        | "lane.create" | "lane.delete" | "lane.merge"
        | "lane.diff" | "lane.focus"
        // repomind orchestrator: read (status/transcript) + interact (send_input/key) are safe like
        // the agent equivalents above. start/stop spawn/kill the orchestrator's claude — a remote
        // process-spawn with caller-chosen autonomy/max_agents/prompt, so strictly higher privilege
        // than the already-allowed agent.spawn (which targets a known, already-configured agent
        // rather than an arbitrary claude invocation). Remote tokens may chat with a running
        // repomind but cannot start or stop it.
        | "orchestrator.status" | "orchestrator.transcript"
        | "orchestrator.send_input" | "orchestrator.key"
        // benign metadata
        | "agent.pin" | "session.rename"
        // companion self-registration for push
        | "push.register" | "push.unregister"
    )
}

/// Bind the WebSocket bridge and serve until shutdown is requested.
pub async fn serve_remote(ctx: Arc<Ctx>, bind: &str, token: String) -> std::io::Result<()> {
    let listener = TcpListener::bind(bind).await?;
    tracing::info!("remote bridge listening on ws://{bind}");
    let token = Arc::new(token);
    let conns = Arc::new(AtomicUsize::new(0));

    loop {
        tokio::select! {
            _ = ctx.shutdown.notified() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, addr)) => {
                    // Reserve a slot; over the cap we drop the connection (guard decrements).
                    let guard = ConnGuard(conns.clone());
                    if conns.fetch_add(1, Ordering::Relaxed) >= MAX_REMOTE_CONNS {
                        tracing::warn!("remote connection cap reached, dropping {addr}");
                        continue; // `guard` drops here, undoing the increment
                    }
                    let ctx = ctx.clone();
                    let token = token.clone();
                    tokio::spawn(async move {
                        let _guard = guard;
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
    let ws = tokio_tungstenite::accept_hdr_async_with_config(
        stream,
        |req: &HsRequest, resp| auth(req, resp, token),
        Some(remote_ws_config()),
    )
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
                let id = req.id;
                let resp = if remote_method_allowed(&req.method) {
                    if req.method == "subscribe" {
                        forwarding = true;
                    }
                    match rpc::dispatch(&ctx, &req.method, req.params).await {
                        Ok(value) => Response::ok(id, value),
                        Err(err) => Response::err(id, err),
                    }
                } else {
                    // Default-deny: host-management RPCs aren't reachable over the network.
                    tracing::warn!("remote bridge rejected method {:?}", req.method);
                    Response::err(
                        id,
                        RpcError::new(
                            -32601,
                            format!("method '{}' not permitted over remote bridge", req.method),
                        ),
                    )
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

    #[test]
    fn remote_allowlist_permits_full_fleet_control() {
        // Read, interact with, and now manage the fleet: spawn/stop/adopt agents, create/delete/
        // merge lanes, plus the companion's own push registration.
        for m in [
            "ping",
            "repo.list",
            "lane.list",
            "lane.get",
            "lane.create",
            "lane.delete",
            "lane.merge",
            "lane.diff",
            "lane.focus",
            "commit.recent",
            "agent.capture",
            "agent.transcript",
            "agent.prompt",
            "agent.answer",
            "agent.watch_bytes",
            "agent.spawn",
            "agent.stop",
            "agent.adopt",
            "agent.detect",
            "terminal.list_all",
            "agent.send_input",
            "agent.signal",
            "agent.key",
            "agent.scroll",
            "agent.target",
            "agent.fit",
            "agent.pin",
            "subscribe",
            "viewport.set",
            "usage.get",
            "daemon.status",
            "push.register",
            "push.unregister",
            "session.rename",
            "orchestrator.status",
            "orchestrator.transcript",
            "orchestrator.send_input",
            "orchestrator.key",
        ] {
            assert!(remote_method_allowed(m), "{m} should be allowed");
        }
        // Daemon lifecycle, config/secrets, host terminal + filesystem access, and credential
        // minting stay blocked over the bridge even under full fleet control.
        for m in [
            "agent.resize",
            "agent.add",
            "agent.remove",
            "agent.set_default",
            "repo.add",
            "repo.remove",
            "repo.discover",
            "config.get",
            "config.set",
            "terminal.open",
            "terminal.close",
            "terminal.target",
            "fs.browse",
            "daemon.shutdown",
            "watcher.park",
            "orchestrator.watch",
            "orchestrator.resize",
            "orchestrator.start",
            "orchestrator.stop",
            // upcoming local-only credential-minting RPCs (task A2) — must never be reachable
            // over the remote bridge.
            "remote.pair",
            "remote.devices",
            "remote.revoke",
            "some.future.method",
        ] {
            assert!(!remote_method_allowed(m), "{m} must be blocked");
        }
    }
}
