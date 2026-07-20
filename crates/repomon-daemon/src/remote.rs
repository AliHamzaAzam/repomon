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
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use repomon_core::protocol::{MAX_FRAME_BYTES, Request, Response, RpcError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast::error::RecvError;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::http;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};

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
        // orchestrator.watch gates the read-only pane stream (event.orchestrator.output) — the
        // repomind analog of the already-allowed agent.watch_bytes, and per-connection state
        // since the phone-loop work, so a phone toggling its view can never stop the TUI's
        // stream. orchestrator.resize stays blocked: an unmediated remote resize is exactly
        // what squeezed the TUI's view before agent.fit.
        | "orchestrator.watch"
        // benign metadata
        | "agent.pin" | "session.rename"
        // companion self-registration for push
        | "push.register" | "push.unregister"
    )
}

/// Bind the WebSocket bridge and serve until shutdown is requested. The set of valid tokens lives
/// in `ctx.remote_tokens` (seeded from the store's paired devices plus the legacy config token, and
/// refreshed on every pair/revoke), so no token is passed in here.
pub async fn serve_remote(ctx: Arc<Ctx>, bind: &str) -> std::io::Result<()> {
    let listener = TcpListener::bind(bind).await?;
    tracing::info!("remote bridge listening on ws://{bind}");
    serve_remote_on(ctx, listener).await
}

/// Serve the WebSocket bridge on an already-bound listener. Split out from `serve_remote` so tests
/// can bind an exclusive ephemeral port and hand the live listener in — with no bind-then-rebind
/// window for a concurrent test to race on.
pub async fn serve_remote_on(ctx: Arc<Ctx>, listener: TcpListener) -> std::io::Result<()> {
    serve_remote_on_with_timeout(ctx, listener, HANDSHAKE_TIMEOUT).await
}

/// As `serve_remote_on`, but with an explicit pre-upgrade handshake deadline. Exposed so tests can
/// drive a short deadline deterministically; production always uses `HANDSHAKE_TIMEOUT`.
pub async fn serve_remote_on_with_timeout(
    ctx: Arc<Ctx>,
    listener: TcpListener,
    handshake_timeout: Duration,
) -> std::io::Result<()> {
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
                    tokio::spawn(async move {
                        let _guard = guard;
                        if let Err(e) = handle_conn(ctx, stream, handshake_timeout).await {
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

/// How often a live connection re-stamps its device's `last_seen_at` (throttled, per connection).
const LAST_SEEN_THROTTLE: Duration = Duration::from_secs(60);

/// Upper bound on the handshake request head we'll buffer before giving up — a WS upgrade request
/// is a few hundred bytes; anything past this is not a client we serve.
const MAX_HANDSHAKE_BYTES: usize = 16 * 1024;

/// Deadline for the entire pre-upgrade handshake (head read through the 101 write). Auth precedes
/// the upgrade, so a peer that connects and then dribbles or stays silent would otherwise hold its
/// `MAX_REMOTE_CONNS` slot forever without ever authenticating; the deadline drops it so a burst of
/// idle connections can't starve the cap.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

async fn handle_conn(
    ctx: Arc<Ctx>,
    mut stream: TcpStream,
    handshake_timeout: Duration,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    // Bound the whole pre-upgrade handshake (read + auth + 101). On elapse we return, dropping the
    // socket and decrementing the connection guard, so a silent/dribbling peer can't hold its slot.
    let identity = match tokio::time::timeout(handshake_timeout, negotiate(&ctx, &mut stream)).await
    {
        Ok(Ok(Some(identity))) => identity,
        // A refusal (400/401/426) was already written, or a malformed/oversized/EOF handshake, or
        // the deadline elapsed: in every case just drop the connection quietly.
        Ok(Ok(None)) | Ok(Err(_)) | Err(_) => return Ok(()),
    };
    let (conn_token, device_name) = identity;

    // Hand the post-handshake socket to tungstenite for the framed protocol, keeping the same
    // MAX_FRAME_BYTES bounds the previous accept path applied.
    let ws = WebSocketStream::from_raw_socket(stream, Role::Server, Some(remote_ws_config())).await;
    let (mut sink, mut source) = ws.split();

    // This connection's per-device session, carrying its identity (device name) and its own
    // viewport/focus/fit state. The guard drops it from `ctx.sessions` on every exit path below —
    // each `break`, every `?` early return, and a panic.
    let sess = ctx
        .open_session(crate::conn::ConnKind::Remote {
            device: device_name.clone(),
        })
        .await;
    let _session_guard = crate::conn::SessionGuard::new(ctx.clone(), sess.id);

    // Stamp last-seen once on connect for a named device, then at most once per minute below.
    let mut last_seen_stamp = Instant::now();
    if let Some(name) = &device_name {
        let _ = ctx.store.remote_device_seen(name).await;
    }

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
                // Live revocation: if this connection's token has been revoked since the
                // handshake (dropped from the auth cache), refuse this request and close.
                if !token_present(&ctx, &conn_token) {
                    let resp = Response::err(id, RpcError::new(-32000, "device revoked"));
                    send_json(&mut sink, &resp).await?;
                    break;
                }
                // Throttled last-seen refresh for named devices (at most once a minute).
                if let Some(name) = &device_name {
                    if last_seen_stamp.elapsed() >= LAST_SEEN_THROTTLE {
                        last_seen_stamp = Instant::now();
                        let _ = ctx.store.remote_device_seen(name).await;
                    }
                }
                let resp = if remote_method_allowed(&req.method) {
                    if req.method == "subscribe" {
                        forwarding = true;
                    }
                    match rpc::dispatch(&ctx, &sess, &req.method, req.params).await {
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
                    // Passive revocation: a device that stays silent still holds a live event
                    // receiver, so the request-arm's revocation check never runs for it. Re-check
                    // the token on every forward and drop the connection the moment it's gone —
                    // otherwise a revoked-but-quiet device keeps receiving event.agent.bytes/output
                    // forever. Sync std RwLock read, no await held.
                    if !token_present(&ctx, &conn_token) {
                        break;
                    }
                    // Per-connection filtering: `event.agent.bytes` reaches only the connections
                    // that watch its window, and `event.agent.output` only the connections whose
                    // viewport covers its lane/window (the bus broadcasts both to every subscriber);
                    // every other topic forwards unchanged. Sync std-Mutex reads, dropped before the
                    // await.
                    let deliver = {
                        let watched = sess.watched_bytes.lock().unwrap();
                        let out = sess.output_filter.lock().unwrap();
                        crate::pubsub::deliver_to(&value, &watched, &out.0, &out.1)
                    };
                    if forwarding && deliver {
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

/// Run the entire pre-upgrade handshake: read the bounded request head, authenticate (constant
/// time, BEFORE any upgrade), validate the WebSocket upgrade, and write the Title-Case 101. Returns
/// `Ok(Some(identity))` on success; `Ok(None)` when the client was cleanly refused (a 400/401/426
/// was written here); `Err` on an I/O error, malformed/oversized head, or EOF. The caller wraps
/// this in a deadline and drops the socket for any non-`Some` outcome.
///
/// Hand-rolled (rather than tokio-tungstenite's server path) so the 101 uses Title-Case header
/// names (`Connection`, `Upgrade`, `Sec-WebSocket-Accept`): tungstenite serializes them through the
/// `http` crate's `HeaderMap`, which canonicalizes names to lowercase, and iOS 27's CFNetwork
/// rejects that lowercase 101 outright ("bad response from the server").
async fn negotiate(
    ctx: &Arc<Ctx>,
    stream: &mut TcpStream,
) -> std::io::Result<Option<(String, Option<String>)>> {
    let head = read_handshake_head(stream).await?;
    let Some(req) = parse_request(&head) else {
        write_simple_response(stream, 400, "Bad Request", &[]).await?;
        return Ok(None);
    };

    // Constant-time token check, before the upgrade. The matching entry's identity (device name,
    // `None` for the legacy shared token) and the token itself are captured for the session.
    let Some(identity) = authorize(&req, ctx) else {
        // 401 with Title-Case headers, then terminate the connection.
        write_simple_response(stream, 401, "Unauthorized", &[]).await?;
        return Ok(None);
    };

    // Validate the WebSocket upgrade and derive the accept key. A malformed upgrade never reaches
    // this authenticated client's session.
    let accept = match ws_upgrade(&req) {
        WsUpgrade::Accept(accept) => accept,
        // RFC 6455 §4.4: on an unsupported version, answer 426 and advertise the version we speak
        // so a mismatched client can retry rather than guess.
        WsUpgrade::BadVersion => {
            write_simple_response(
                stream,
                426,
                "Upgrade Required",
                &[("Sec-WebSocket-Version", "13")],
            )
            .await?;
            return Ok(None);
        }
        WsUpgrade::Malformed => {
            write_simple_response(stream, 400, "Bad Request", &[]).await?;
            return Ok(None);
        }
    };

    // Emit the Title-Case 101. `permessage-deflate` is deliberately NOT negotiated: we omit
    // `Sec-WebSocket-Extensions` entirely, so no compression is agreed (matching the reference
    // server iOS 27 accepts).
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Connection: Upgrade\r\n\
         Upgrade: websocket\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\
         \r\n"
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    Ok(Some(identity))
}

/// Read the HTTP request head (up to and including the terminating CRLFCRLF) from a freshly
/// accepted socket, bounded by `MAX_HANDSHAKE_BYTES`. A WebSocket client sends nothing before the
/// 101, so a well-behaved peer never writes bytes past the head; if it does, that's a protocol
/// violation and we error (there's no way to feed a tail to `from_raw_socket`, and tungstenite's
/// own server rejects junk-after-request identically).
async fn read_handshake_head(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "eof during handshake",
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(end) = find_headers_end(&buf) {
            if end != buf.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "junk after handshake request",
                ));
            }
            return Ok(buf);
        }
        if buf.len() > MAX_HANDSHAKE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "handshake request head too large",
            ));
        }
    }
}

/// Index just past the `\r\n\r\n` that ends an HTTP head, if present.
fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

/// Parse a raw HTTP request head into an `http::Request<()>` so the existing token-auth path can
/// read its headers and query. Returns `None` for anything that isn't a complete `GET` request.
fn parse_request(head: &[u8]) -> Option<http::Request<()>> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut parsed = httparse::Request::new(&mut headers);
    match parsed.parse(head) {
        Ok(httparse::Status::Complete(_)) => {}
        _ => return None,
    }
    // WebSocket upgrades are always GET; reject anything else outright.
    if !parsed.method.is_some_and(|m| m.eq_ignore_ascii_case("GET")) {
        return None;
    }
    let mut builder = http::Request::builder()
        .method(http::Method::GET)
        .uri(parsed.path?);
    for h in parsed.headers.iter() {
        builder = builder.header(h.name, h.value);
    }
    builder.body(()).ok()
}

/// The outcome of validating a WebSocket upgrade request.
enum WsUpgrade {
    /// A valid upgrade; carries the `Sec-WebSocket-Accept` value for the client's key.
    Accept(String),
    /// A websocket upgrade whose `Sec-WebSocket-Version` isn't the 13 we speak (→ 426).
    BadVersion,
    /// Not a well-formed websocket upgrade at all: missing Upgrade/Connection/Key (→ 400).
    Malformed,
}

/// Validate the WebSocket upgrade request (case-insensitively, per RFC 9110). Mirrors the checks
/// tungstenite's server performs before it would have produced a 101, but distinguishes a version
/// mismatch (answerable with 426) from an otherwise malformed upgrade.
fn ws_upgrade(req: &http::Request<()>) -> WsUpgrade {
    let headers = req.headers();
    let upgrade_ok = headers
        .get("Upgrade")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    let connection_ok = headers
        .get("Connection")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split([' ', ','])
                .any(|p| p.trim().eq_ignore_ascii_case("Upgrade"))
        });
    if !(upgrade_ok && connection_ok) {
        return WsUpgrade::Malformed;
    }
    if headers
        .get("Sec-WebSocket-Version")
        .is_none_or(|v| v != "13")
    {
        return WsUpgrade::BadVersion;
    }
    match headers.get("Sec-WebSocket-Key") {
        Some(key) => WsUpgrade::Accept(derive_accept_key(key.as_bytes())),
        None => WsUpgrade::Malformed,
    }
}

/// Write a minimal Title-Case HTTP response (used for the 400/401/426 pre-upgrade refusals) with
/// any extra headers, and signal connection close. Best-effort: the caller drops the socket right
/// after.
async fn write_simple_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    extra_headers: &[(&str, &str)],
) -> std::io::Result<()> {
    let body = reason;
    let mut response = format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\n");
    for (name, value) in extra_headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str(&format!(
        "Content-Type: text/plain\r\nContent-Length: {len}\r\n\r\n{body}",
        len = body.len(),
    ));
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    stream.shutdown().await
}

/// Match the handshake's presented token against the auth cache. On a hit, returns
/// `Some((token, device_name))` — `device_name` is `None` for the legacy shared config token — so
/// the connection learns the identity it authenticated as. `None` means no valid token (→ 401).
fn authorize(req: &http::Request<()>, ctx: &Ctx) -> Option<(String, Option<String>)> {
    let presented = presented_token(req)?;
    let tokens = ctx.remote_tokens.read().unwrap();
    for (tok, name) in tokens.iter() {
        if constant_time_eq(presented.as_bytes(), tok.as_bytes()) {
            return Some((presented, name.clone()));
        }
    }
    None
}

/// Whether a token is still in the auth cache — the live-revocation check on each request.
fn token_present(ctx: &Ctx, token: &str) -> bool {
    ctx.remote_tokens
        .read()
        .unwrap()
        .iter()
        .any(|(t, _)| constant_time_eq(token.as_bytes(), t.as_bytes()))
}

/// The token a handshake request carries: `Authorization: Bearer <token>` or a `token=<token>`
/// query parameter (for clients that can't set headers on a WS dial). The query value is taken
/// verbatim (no percent-decoding): minted tokens are URL-safe by construction, so a raw `%` never
/// appears in a legitimate token and decoding would only widen the input we accept.
fn presented_token(req: &http::Request<()>) -> Option<String> {
    req.headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
        .or_else(|| {
            req.uri().query().and_then(|q| {
                q.split('&')
                    .find_map(|kv| kv.strip_prefix("token=").map(str::to_string))
            })
        })
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
            "orchestrator.watch",
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
            // per-repo notes stay local-only for now: the write side injects text into every
            // future worker prompt for that repo, too much leverage for the bridge until a
            // deliberate Phase 6 decision allowlists it.
            "repo.notes.get",
            "repo.notes.set",
            // the orchestration journal is local-only like the notes it complements: append is
            // an unauthenticated write channel into every future recap, and query exposes the
            // full action history — neither belongs on the bridge without a deliberate decision.
            "journal.append",
            "journal.query",
            // playbooks stay local-only: save is a write channel into future orchestrator
            // prompts (post-approval), and approve is the human gate itself — neither belongs
            // on the bridge.
            // standing-run schedules mint unattended orchestrator processes — strictly
            // local-only.
            // approval policy shapes what the daemon auto-approves — the definition of a
            // permission bypass. Strictly local-only.
            "approval.record",
            "approval.allow",
            "approval.remove",
            "approval.list",
            "schedule.add",
            "schedule.list",
            "schedule.remove",
            "playbook.save",
            "playbook.search",
            "playbook.list",
            "playbook.approve",
            "playbook.delete",
            "terminal.open",
            "terminal.close",
            "terminal.target",
            "fs.browse",
            "daemon.shutdown",
            "watcher.park",
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
