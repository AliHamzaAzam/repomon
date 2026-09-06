//! Serves JSON-RPC over WebSocket text frames on a private interface, checking header or query
//! bearer tokens before upgrade and rejecting invalid tokens with HTTP 401.

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

/// Bounds concurrent accepted connections, including handshakes awaiting authentication.
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

/// Default-deny remote RPCs: permit explicit fleet control while keeping lifecycle, secrets,
/// credentials, host files, policy grants, and arbitrary commit-history reads local.
fn remote_method_allowed(method: &str) -> bool {
    matches!(
        method,
        "ping"

        | "repo.list" | "lane.list" | "lane.get"
        | "commit.today" | "commit.range" | "commit.search" | "commit.recent"
        | "agent.capture" | "agent.transcript" | "agent.transcript_page"
        | "usage.get" | "daemon.status"
        // Allow remote usage reads while keeping host mutations and probes local.
        | "usage.summary" | "usage.timeline" | "usage.sessions" | "usage.findings"
        | "usage.status" | "usage.rates" | "usage.models"
        // terminal-window *names* only ({lane_id, id} pairs) - open/close/target stay blocked
        | "terminal.list_all"

        | "subscribe" | "viewport.set"
        // agent.fit arbitrates shared pane size; unrestricted agent.resize remains local so remote
        // clients cannot displace a local viewport.
        | "agent.send_input" | "agent.signal" | "agent.key" | "agent.scroll"
        | "agent.target" | "agent.fit"
        | "agent.prompt" | "agent.answer" | "agent.watch_bytes"
        // agent.detect exposes selectable agents without granting access to config secrets.
        | "agent.spawn" | "agent.stop" | "agent.adopt" | "agent.detect"
        | "lane.create" | "lane.delete" | "lane.merge"
        | "lane.diff" | "lane.focus"
        // Remote clients may interact with a running orchestrator, but starting or stopping one
        // grants broader process and autonomy control.
        | "orchestrator.status" | "orchestrator.transcript"
        | "orchestrator.send_input" | "orchestrator.key"
        // The orchestrator watch is per connection; unmediated resize remains local.
        | "orchestrator.watch"
        // repomind home: read-only metadata about the home repo and its controller lane (where it
        // lives, which lane and window carry it, the controller cap). No file content and no
        // writes; every repomind RPC that touches the home's files stays local-only.
        | "repomind.status"

        | "agent.pin" | "session.rename"

        | "push.register" | "push.unregister"
        // Supervision observation and manual nudges do not grant standing approval authority;
        // policy changes remain local.
        | "supervision.get" | "supervision.audit" | "supervision.status" | "supervision.nudge"
    )
}

/// Serves the WebSocket bridge until shutdown, authenticating against the live paired-device and
/// shared-token cache.
pub async fn serve_remote(ctx: Arc<Ctx>, bind: &str) -> std::io::Result<()> {
    let listener = TcpListener::bind(bind).await?;
    tracing::info!("remote bridge listening on ws://{bind}");
    serve_remote_on(ctx, listener).await
}

/// Serve the WebSocket bridge on an already-bound listener. Split out from `serve_remote` so tests
/// can bind an exclusive ephemeral port and hand the live listener in - with no bind-then-rebind
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

/// Upper bound on the handshake request head we'll buffer before giving up - a WS upgrade request
/// is a few hundred bytes; anything past this is not a client we serve.
const MAX_HANDSHAKE_BYTES: usize = 16 * 1024;

/// Bound the entire pre-upgrade handshake so a partial request cannot occupy an unauthenticated
/// slot indefinitely.
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
    // viewport/focus/fit state. The guard drops it from `ctx.sessions` on every exit path below -
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

    // Every connection holds an event receiver, but only forwards once subscribed -
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
                    // Recheck tokens during forwarding so a revoked silent client cannot retain its
                    // event stream.
                    if !token_present(&ctx, &conn_token) {
                        break;
                    }
                    // Filter against this connection’s requested streams before awaiting delivery.
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

/// Authenticate before upgrading; emit Title-Case response headers because iOS CFNetwork rejects
/// the lowercase form produced by HeaderMap.
async fn negotiate(
    ctx: &Arc<Ctx>,
    stream: &mut TcpStream,
) -> std::io::Result<Option<(String, Option<String>)>> {
    let head = read_handshake_head(stream).await?;
    let Some(req) = parse_request(&head) else {
        write_simple_response(stream, 400, "Bad Request", &[]).await?;
        return Ok(None);
    };

    // Authenticate before upgrading and retain the matched token and device identity for revocation
    // checks.
    let Some(identity) = authorize(&req, ctx) else {
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

/// Read a bounded request head and reject trailing bytes because from_raw_socket cannot consume an
/// already-read tail.
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

/// Returns the matched token and optional device name from the auth cache, or None for an
/// unauthorized handshake.
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

/// Whether a token is still in the auth cache - the live-revocation check on each request.
fn token_present(ctx: &Ctx, token: &str) -> bool {
    ctx.remote_tokens
        .read()
        .unwrap()
        .iter()
        .any(|(t, _)| constant_time_eq(token.as_bytes(), t.as_bytes()))
}

/// Reads the bearer header or raw query token without percent-decoding because minted tokens are
/// URL-safe.
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
    fn remote_allowlist_permits_the_ledger_reads_and_blocks_its_host_side_actions() {
        for m in [
            "usage.summary",
            "usage.timeline",
            "usage.sessions",
            "usage.findings",
            "usage.status",
            "usage.rates",
            "usage.models",
        ] {
            assert!(remote_method_allowed(m), "{m} must be allowed");
        }
        for m in [
            "usage.refresh",
            "usage.ingest_now",
            "usage.export",
            "usage.refresh_rates",
        ] {
            assert!(!remote_method_allowed(m), "{m} must be blocked");
        }
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
            "agent.transcript_page",
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
            "repomind.status",
        ] {
            assert!(remote_method_allowed(m), "{m} should be allowed");
        }
        // Daemon lifecycle, config/secrets, host terminal + filesystem access, and credential
        // minting stay blocked over the bridge even under full fleet control.
        for m in [
            "agent.resize",
            // Keep custom-agent configuration local because it controls which binaries later spawns
            // execute.
            "agent.add",
            "agent.remove",
            "agent.set_default",
            "repo.add",
            "repo.remove",
            "repo.set_hidden",
            "repo.discover",
            "config.get",
            "config.set",
            // Repository notes remain local because their contents enter worker prompts.
            "repo.notes.get",
            "repo.notes.set",
            // Journal writes influence recaps and reads expose the complete action history; both
            // remain local.
            "journal.append",
            "journal.query",
            // Keep playbook approval, unattended schedules, and automatic-approval policy local
            // because they grant future execution authority.
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
            "playbook.reject",
            "playbook.delete",
            // Fleet mail requires a local registered sending identity and contains internal agent
            // coordination.
            "message.send",
            "message.inbox",
            "message.mark_read",
            "message.list",
            "message.force_send",
            "message.delete",
            "terminal.open",
            "terminal.close",
            "terminal.target",
            "fs.browse",
            // File access remains local, including writes that overwrite host files.
            "file.list",
            "file.read",
            "file.read_raw",
            "file.write",
            "file.index",
            "file.create",
            "file.rename",
            "file.delete",
            "file.search",
            "file.diff_base",
            // Caller-chosen commit IDs expose arbitrary repository history, beyond the allowed
            // current lane diff.
            "commit.show",
            "daemon.shutdown",
            // system.doctor is intentionally local-only (machine health / dependency check of the host)
            "system.doctor",
            "watcher.park",
            "orchestrator.resize",
            "orchestrator.start",
            "orchestrator.stop",
            // Repomind exports and boot regeneration write home files and remain local.
            "repomind.export",
            "repomind.boot",
            // repomind.instruct types into a controller pane holding the full fleet catalog:
            // broader authority than the bridge's agent.send_input on one worker.
            "repomind.instruct",
            // supervision.set grants standing auto-approval authority - strictly local-only.
            "supervision.set",
            // Credential minting must remain local.
            "remote.pair",
            "remote.devices",
            "remote.revoke",
            "some.future.method",
        ] {
            assert!(!remote_method_allowed(m), "{m} must be blocked");
        }
    }

    #[test]
    fn supervision_set_is_not_remote_reachable() {
        assert!(!remote_method_allowed("supervision.set"));
        assert!(remote_method_allowed("supervision.get"));
        assert!(remote_method_allowed("supervision.audit"));
        assert!(remote_method_allowed("supervision.status"));
        assert!(remote_method_allowed("supervision.nudge"));
    }
}
