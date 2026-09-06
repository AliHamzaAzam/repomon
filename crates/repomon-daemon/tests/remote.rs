//! End-to-end for the remote WebSocket bridge: token gate, RPC round-trip, event push.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use repomon_core::agent::backend::SpawnSpec;
use repomon_core::{Config, Store, TmuxRuntime};
use repomon_daemon::bytes_stream::WatchEntry;
use repomon_daemon::conn::{ConnKind, ConnSession};
use repomon_daemon::{Ctx, pubsub, remote, rpc};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

/// Serve an already-bound ephemeral listener without a port-reuse gap; seed tokens in the context
/// before calling.
async fn serve(ctx: Arc<Ctx>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move { remote::serve_remote_on(ctx, listener).await });
    addr
}

/// As `serve`, but with an explicit (short) pre-upgrade handshake deadline so the deadline test is
/// fast and deterministic instead of waiting the 10s production default.
async fn serve_with_timeout(ctx: Arc<Ctx>, handshake_timeout: Duration) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        remote::serve_remote_on_with_timeout(ctx, listener, handshake_timeout).await
    });
    addr
}

async fn start_bridge(token: &str) -> (Arc<Ctx>, String) {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);

    ctx.remote_tokens
        .write()
        .unwrap()
        .push((token.to_string(), None));
    let addr = serve(ctx.clone()).await;
    (ctx, addr)
}

/// Read one JSON value from the socket (fails the test on a non-text or absent frame).
async fn recv_json(
    ws: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
) -> Value {
    match ws.next().await.unwrap().unwrap() {
        Message::Text(t) => serde_json::from_str(&t).unwrap(),
        m => panic!("unexpected frame: {m:?}"),
    }
}

#[tokio::test]
async fn bridge_round_trips_rpc_and_events_with_token() {
    let (ctx, addr) = start_bridge("sekrit-token").await;

    // Connect with the token in the query (the header path is equivalent).
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/?token=sekrit-token"))
        .await
        .expect("authorized connect");

    ws.send(Message::text(
        json!({"jsonrpc":"2.0","id":1,"method":"ping"}).to_string(),
    ))
    .await
    .unwrap();
    let resp: Value = match ws.next().await.unwrap().unwrap() {
        Message::Text(t) => serde_json::from_str(&t).unwrap(),
        m => panic!("unexpected frame: {m:?}"),
    };
    assert_eq!(resp["result"], json!("pong"));

    ws.send(Message::text(
        json!({"jsonrpc":"2.0","id":2,"method":"repo.list"}).to_string(),
    ))
    .await
    .unwrap();
    let resp: Value = match ws.next().await.unwrap().unwrap() {
        Message::Text(t) => serde_json::from_str(&t).unwrap(),
        m => panic!("unexpected frame: {m:?}"),
    };
    assert_eq!(resp["result"], json!([]));

    // Device registration for push round-trips (idempotent re-register, then unregister).
    for (id, method) in [
        (10, "push.register"),
        (11, "push.register"),
        (12, "push.unregister"),
    ] {
        ws.send(Message::text(
            json!({"jsonrpc":"2.0","id":id,"method":method,
                   "params":{"device_token":"feedcafe"}})
            .to_string(),
        ))
        .await
        .unwrap();
        let resp: Value = match ws.next().await.unwrap().unwrap() {
            Message::Text(t) => serde_json::from_str(&t).unwrap(),
            m => panic!("unexpected frame: {m:?}"),
        };
        assert!(resp["error"].is_null(), "{method} errored: {resp}");
    }

    ws.send(Message::text(
        json!({"jsonrpc":"2.0","id":3,"method":"subscribe"}).to_string(),
    ))
    .await
    .unwrap();
    let _sub_ack = ws.next().await.unwrap().unwrap();
    ctx.broadcast("event.test", json!({ "x": 1 }));
    let event: Value = match tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("event within 2s")
        .unwrap()
        .unwrap()
    {
        Message::Text(t) => serde_json::from_str(&t).unwrap(),
        m => panic!("unexpected frame: {m:?}"),
    };
    assert_eq!(event["method"], json!("event.test"));
    assert_eq!(event["params"]["x"], json!(1));
}

#[tokio::test]
async fn bridge_rejects_bad_or_missing_token_before_upgrade() {
    let (_ctx, addr) = start_bridge("right-token").await;

    let wrong = tokio_tungstenite::connect_async(format!("ws://{addr}/?token=wrong-token")).await;
    assert!(wrong.is_err(), "wrong token must not complete the upgrade");

    let missing = tokio_tungstenite::connect_async(format!("ws://{addr}/")).await;
    assert!(
        missing.is_err(),
        "missing token must not complete the upgrade"
    );
}

/// The RFC 6455 sample key and its companion `Sec-WebSocket-Accept` value (SHA1 of the key plus
/// the WS GUID, base64). Fixed so the byte-level tests can assert the exact response bytes.
const SAMPLE_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const SAMPLE_ACCEPT: &str = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";

/// Sends a raw handshake and returns its unnormalized response head with the open stream for
/// byte-level protocol assertions.
async fn raw_handshake(addr: &str, request: &str) -> (tokio::net::TcpStream, String) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await.unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    (stream, String::from_utf8_lossy(&buf).into_owned())
}

/// Build a WS upgrade request for the bridge with the given token and any extra header lines.
fn handshake_request(token: &str, extra_headers: &str) -> String {
    format!(
        "GET /?token={token} HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {SAMPLE_KEY}\r\n\
         Sec-WebSocket-Version: 13\r\n\
         {extra_headers}\r\n"
    )
}

/// CFNetwork (iOS 27) rejects a lowercase 101; the bridge must emit Title-Case response header
/// names. Assert the raw response bytes carry `Connection:`, `Upgrade:`, and `Sec-WebSocket-Accept:`
/// with exactly this casing and the correct accept value, and NOT their lowercase forms.
#[tokio::test]
async fn handshake_response_uses_title_case_header_names() {
    let (_ctx, addr) = start_bridge("sekrit-token").await;
    let (_stream, resp) = raw_handshake(&addr, &handshake_request("sekrit-token", "")).await;

    assert!(
        resp.starts_with("HTTP/1.1 101 "),
        "expected a 101 status line, got: {resp:?}"
    );
    assert!(
        resp.contains("\r\nConnection: Upgrade\r\n"),
        "response must carry Title-Case `Connection: Upgrade`: {resp:?}"
    );
    assert!(
        resp.contains("\r\nUpgrade: websocket\r\n"),
        "response must carry Title-Case `Upgrade: websocket`: {resp:?}"
    );
    assert!(
        resp.contains(&format!("\r\nSec-WebSocket-Accept: {SAMPLE_ACCEPT}\r\n")),
        "response must carry Title-Case `Sec-WebSocket-Accept` with the correct value: {resp:?}"
    );
    // The lowercase serialization iOS 27 rejects must be gone.
    assert!(
        !resp.contains("\r\nconnection:") && !resp.contains("\r\nupgrade:"),
        "no lowercase handshake header names may remain: {resp:?}"
    );
    assert!(
        !resp.contains("\r\nsec-websocket-accept:"),
        "no lowercase sec-websocket-accept may remain: {resp:?}"
    );
}

/// A client offering `permessage-deflate` still gets a 101, and the bridge negotiates NO
/// compression: it omits `Sec-WebSocket-Extensions` from the response entirely (matching the
/// accepted reference server). We do NOT implement deflate.
#[tokio::test]
async fn handshake_does_not_negotiate_permessage_deflate() {
    let (_ctx, addr) = start_bridge("sekrit-token").await;
    let (_stream, resp) = raw_handshake(
        &addr,
        &handshake_request(
            "sekrit-token",
            "Sec-WebSocket-Extensions: permessage-deflate\r\n",
        ),
    )
    .await;

    assert!(
        resp.starts_with("HTTP/1.1 101 "),
        "a deflate-offering client still upgrades: {resp:?}"
    );
    assert!(
        !resp
            .to_ascii_lowercase()
            .contains("sec-websocket-extensions"),
        "the bridge must omit Sec-WebSocket-Extensions (no compression agreed): {resp:?}"
    );
}

/// A bad token is refused with a 401 BEFORE the upgrade, and the connection is terminated (the
/// server does not leave a half-open upgraded socket for an unauthorized client).
#[tokio::test]
async fn handshake_bad_token_gets_401_and_closes() {
    let (_ctx, addr) = start_bridge("right-token").await;
    let (mut stream, resp) = raw_handshake(&addr, &handshake_request("wrong-token", "")).await;

    assert!(
        resp.starts_with("HTTP/1.1 401 "),
        "a bad token must be refused with 401 before the upgrade: {resp:?}"
    );
    assert!(
        !resp.starts_with("HTTP/1.1 101"),
        "an unauthorized client must never see a 101"
    );
    // The server terminates the connection after the 401: the next read is EOF.
    let mut tail = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut tail))
        .await
        .expect("the server closes the socket within 2s")
        .expect("read after 401");
    assert_eq!(n, 0, "the connection must be closed after a 401");
}

/// An authenticated client offering an unsupported `Sec-WebSocket-Version` gets a 426 that
/// advertises the version the bridge speaks (RFC 6455 §4.4), not a bare 400.
#[tokio::test]
async fn handshake_bad_version_gets_426_with_supported_version() {
    let (_ctx, addr) = start_bridge("sekrit-token").await;
    let request = format!(
        "GET /?token=sekrit-token HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {SAMPLE_KEY}\r\n\
         Sec-WebSocket-Version: 8\r\n\
         \r\n"
    );
    let (_stream, resp) = raw_handshake(&addr, &request).await;
    assert!(
        resp.starts_with("HTTP/1.1 426 "),
        "an unsupported version must get 426 Upgrade Required: {resp:?}"
    );
    assert!(
        resp.contains("\r\nSec-WebSocket-Version: 13\r\n"),
        "the 426 must advertise the supported version: {resp:?}"
    );
}

/// A partial handshake must time out and release its slot for a later client.
#[tokio::test]
async fn handshake_deadline_drops_a_dribbling_client_and_frees_the_slot() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    ctx.remote_tokens
        .write()
        .unwrap()
        .push(("tok".to_string(), None));
    let addr = serve_with_timeout(ctx.clone(), Duration::from_millis(200)).await;

    // Connect and send only a partial request head (no terminating CRLFCRLF), then go quiet.
    let mut dribble = tokio::net::TcpStream::connect(&addr).await.unwrap();
    dribble
        .write_all(b"GET /?token=tok HTTP/1.1\r\nHost: 127.0.0.1\r\n")
        .await
        .unwrap();
    dribble.flush().await.unwrap();

    // The server closes it once the deadline elapses: the next read returns EOF (0 bytes), well
    // within a generous ceiling that still catches a never-closing socket.
    let mut buf = [0u8; 8];
    let n = tokio::time::timeout(Duration::from_secs(5), dribble.read(&mut buf))
        .await
        .expect("the server drops a dribbling client within the deadline")
        .expect("read after the server closes");
    assert_eq!(
        n, 0,
        "a client that never completes the handshake is disconnected by the deadline"
    );

    // A subsequent legit handshake still succeeds - the dribbling client's slot was freed.
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/?token=tok"))
        .await
        .expect("a legit client connects after the dribbling one is reaped");
    ws.send(Message::text(
        json!({"jsonrpc":"2.0","id":1,"method":"ping"}).to_string(),
    ))
    .await
    .unwrap();
    assert_eq!(recv_json(&mut ws).await["result"], json!("pong"));
}

#[tokio::test]
async fn bridge_authenticates_a_named_device_and_stamps_last_seen() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    // Pair a device, seed its token into the auth cache with the device name.
    let dev = ctx.store.remote_device_pair("phone").await.unwrap();
    ctx.remote_tokens
        .write()
        .unwrap()
        .push((dev.token.clone(), Some("phone".into())));
    let addr = serve(ctx.clone()).await;

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/?token={}", dev.token))
        .await
        .expect("named-device token authorizes");
    ws.send(Message::text(
        json!({"jsonrpc":"2.0","id":1,"method":"ping"}).to_string(),
    ))
    .await
    .unwrap();
    assert_eq!(recv_json(&mut ws).await["result"], json!("pong"));

    // The handshake stamps last_seen_at for the named device (poll - it happens in the handler).
    let mut stamped = false;
    for _ in 0..100 {
        let d = &ctx.store.remote_device_list().await.unwrap()[0];
        if d.last_seen_at.is_some() {
            stamped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        stamped,
        "a named device's last_seen_at is stamped on connect"
    );
}

#[tokio::test]
async fn bridge_kicks_a_revoked_token_mid_session() {
    let (ctx, addr) = start_bridge("live-token").await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/?token=live-token"))
        .await
        .expect("authorized connect");

    ws.send(Message::text(
        json!({"jsonrpc":"2.0","id":1,"method":"ping"}).to_string(),
    ))
    .await
    .unwrap();
    assert_eq!(recv_json(&mut ws).await["result"], json!("pong"));

    // Revoke: drop the token from the auth cache (what `remote.revoke` does via refresh).
    ctx.remote_tokens.write().unwrap().clear();

    // The next request is refused with -32000 "device revoked", then the socket closes.
    ws.send(Message::text(
        json!({"jsonrpc":"2.0","id":2,"method":"ping"}).to_string(),
    ))
    .await
    .unwrap();
    let resp = recv_json(&mut ws).await;
    assert_eq!(resp["error"]["code"], json!(-32000));
    assert_eq!(resp["error"]["message"], json!("device revoked"));

    let closed = matches!(
        ws.next().await,
        None | Some(Ok(Message::Close(_))) | Some(Err(_))
    );
    assert!(
        closed,
        "the bridge closes the socket after a revoked request"
    );
}

/// Recheck revocation while forwarding events because a passive subscriber may never send another
/// request.
#[tokio::test]
async fn bridge_stops_events_to_a_silently_revoked_device() {
    let (ctx, addr) = start_bridge("live-token").await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/?token=live-token"))
        .await
        .expect("authorized connect");

    // Subscribe so the connection forwards events; drain the ack. (A byte-watch would be seeded the
    // same way as the other bridge tests, but a plain topic exercises the same forward arm.)
    ws.send(Message::text(
        json!({"jsonrpc":"2.0","id":1,"method":"subscribe"}).to_string(),
    ))
    .await
    .unwrap();
    let _ack = recv_json(&mut ws).await;

    // A pre-revocation broadcast is delivered - the stream is live.
    ctx.broadcast("event.test", json!({ "n": 1 }));
    let ev = recv_json(&mut ws).await;
    assert_eq!(ev["method"], json!("event.test"));
    assert_eq!(ev["params"]["n"], json!(1));

    // Revoke by clearing the auth cache (what `remote.revoke` does via refresh). The device sends
    // NO further request, so only the event-forward arm can notice.
    ctx.remote_tokens.write().unwrap().clear();

    // The next event must NOT reach the device: the forward arm re-checks the token, finds it gone,
    // and closes the socket. The client sees a close (or EOF/err), never the event frame.
    ctx.broadcast("event.test", json!({ "n": 2 }));
    let closed = matches!(
        tokio::time::timeout(Duration::from_secs(2), ws.next())
            .await
            .expect("the bridge acts within 2s"),
        None | Some(Ok(Message::Close(_))) | Some(Err(_))
    );
    assert!(
        closed,
        "a silently-revoked device stops receiving events and the socket closes"
    );
}

#[tokio::test]
async fn remote_pair_list_revoke_round_trip_over_dispatch() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    // A session is required by the dispatch signature; these local-only RPCs don't touch it.
    let sess = ctx.open_session(ConnKind::Local).await;

    let pair = rpc::dispatch(&ctx, &sess, "remote.pair", Some(json!({ "name": "phone" })))
        .await
        .unwrap();
    assert_eq!(pair["name"], json!("phone"));
    let token = pair["token"].as_str().unwrap();
    assert!(token.len() >= 32);
    // The fragment is exclusively the bearer token; device metadata must precede it in query
    // parameters.
    let url = pair["url"].as_str().unwrap();
    assert_eq!(url, &format!("repomon://?name=phone#{token}"));
    assert_eq!(ctx.remote_tokens.read().unwrap().len(), 1);

    // re-pair the same name is idempotent (same token, no second cache entry).
    let again = rpc::dispatch(&ctx, &sess, "remote.pair", Some(json!({ "name": "phone" })))
        .await
        .unwrap();
    assert_eq!(pair["token"], again["token"]);
    assert_eq!(ctx.remote_tokens.read().unwrap().len(), 1);

    let devices = rpc::dispatch(&ctx, &sess, "remote.devices", None)
        .await
        .unwrap();
    let d0 = &devices.as_array().unwrap()[0];
    assert_eq!(d0["name"], json!("phone"));
    assert_eq!(d0["role"], json!("full"));
    assert!(
        d0.get("token").is_none(),
        "the listing never exposes the token"
    );

    let rev = rpc::dispatch(
        &ctx,
        &sess,
        "remote.revoke",
        Some(json!({ "name": "phone" })),
    )
    .await
    .unwrap();
    assert_eq!(rev["revoked"], json!(true));
    assert!(ctx.remote_tokens.read().unwrap().is_empty());

    let rev2 = rpc::dispatch(
        &ctx,
        &sess,
        "remote.revoke",
        Some(json!({ "name": "phone" })),
    )
    .await
    .unwrap();
    assert_eq!(rev2["revoked"], json!(false));
}

/// Concurrent pair and revoke must leave the auth cache consistent with the store, never restoring
/// a revoked token.
#[tokio::test]
async fn concurrent_pair_and_revoke_leave_the_cache_consistent() {
    use std::collections::HashSet;

    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sess = ctx.open_session(ConnKind::Local).await;

    rpc::dispatch(&ctx, &sess, "remote.pair", Some(json!({ "name": "a" })))
        .await
        .unwrap();

    let (c1, s1) = (ctx.clone(), sess.clone());
    let (c2, s2) = (ctx.clone(), sess.clone());
    let pair = tokio::spawn(async move {
        rpc::dispatch(&c1, &s1, "remote.pair", Some(json!({ "name": "b" })))
            .await
            .unwrap();
    });
    let revoke = tokio::spawn(async move {
        rpc::dispatch(&c2, &s2, "remote.revoke", Some(json!({ "name": "a" })))
            .await
            .unwrap();
    });
    pair.await.unwrap();
    revoke.await.unwrap();

    // The auth cache must equal the store's live device set: "b" present, "a" absent.
    let store_names: HashSet<String> = ctx
        .store
        .remote_device_list()
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.name)
        .collect();
    let cache_names: HashSet<String> = ctx
        .remote_tokens
        .read()
        .unwrap()
        .iter()
        .filter_map(|(_, n)| n.clone())
        .collect();
    assert_eq!(
        cache_names, store_names,
        "auth cache must mirror the store after concurrent pair+revoke"
    );
    assert!(
        !cache_names.contains("a"),
        "a revoked device must never survive in the cache"
    );
    assert!(
        cache_names.contains("b"),
        "the concurrently paired device is present"
    );
}

/// Run a git command in `dir`, asserting success (test setup for a real repo to branch from).
fn git(dir: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?} failed");
}

/// Remote lane creation ignores caller-supplied filesystem paths while local callers retain that
/// capability.
#[tokio::test]
async fn remote_lane_create_ignores_caller_path() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);

    // A real repo with one commit on main (lane.create branches a worktree off it).
    let repo_dir = tempfile::tempdir().unwrap();
    git(repo_dir.path(), &["init", "-b", "main"]);
    std::fs::write(repo_dir.path().join("README.md"), "hi\n").unwrap();
    git(repo_dir.path(), &["add", "."]);
    git(repo_dir.path(), &["commit", "-m", "init"]);
    let repo = ctx.registry.add(repo_dir.path()).await.unwrap();

    // A paired device tries to pin the worktree to an attacker-chosen host path.
    let sess = ctx
        .open_session(ConnKind::Remote {
            device: Some("phone".into()),
        })
        .await;
    let outside = tempfile::tempdir().unwrap();
    // Canonicalize temporary paths for macOS comparisons, then strip the Windows verbatim prefix
    // that git cannot create directories under.
    let outside_base = std::fs::canonicalize(outside.path()).unwrap();
    #[cfg(windows)]
    let outside_base = PathBuf::from(
        outside_base
            .to_string_lossy()
            .strip_prefix(r"\\?\")
            .map(str::to_owned)
            .unwrap_or_else(|| outside_base.to_string_lossy().into_owned()),
    );
    let evil_path = outside_base.join("pwned");
    let lane = rpc::dispatch(
        &ctx,
        &sess,
        "lane.create",
        Some(json!({
            "repo_id": repo.id,
            "branch": "feat/x",
            "source_branch": "main",
            "path": evil_path.to_string_lossy(),
        })),
    )
    .await
    .expect("remote lane.create still succeeds (path is stripped, not rejected)");

    let created = PathBuf::from(lane["worktree"]["path"].as_str().unwrap());
    assert_ne!(
        created, evil_path,
        "remote lane.create must not honor the caller-supplied path"
    );
    assert!(
        !evil_path.exists(),
        "nothing may be created at the attacker path"
    );

    // Sanity: a LOCAL caller's path IS honored - the strip is remote-only.
    let local = ctx.open_session(ConnKind::Local).await;
    let local_path = outside_base.join("local-ok");
    let lane2 = rpc::dispatch(
        &ctx,
        &local,
        "lane.create",
        Some(json!({
            "repo_id": repo.id,
            "branch": "feat/y",
            "source_branch": "main",
            "path": local_path.to_string_lossy(),
        })),
    )
    .await
    .expect("local lane.create honors the path");
    assert_eq!(
        PathBuf::from(lane2["worktree"]["path"].as_str().unwrap()),
        local_path,
        "a local caller keeps full control of the worktree path"
    );
}

/// Find the live session a named remote device connected as (polls; the session registers just
/// after the handshake). Correlating by device name avoids depending on connection-id ordering.
async fn session_for_device(ctx: &Ctx, device: &str) -> Arc<ConnSession> {
    for _ in 0..200 {
        {
            let sessions = ctx.sessions.lock().await;
            for s in sessions.values() {
                if matches!(&s.kind, ConnKind::Remote { device: Some(d) } if d == device) {
                    return s.clone();
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no live session for device {device}");
}

/// Verify per-connection byte filtering over the bridge with seeded watches, avoiding a live
/// backend dependency.
#[tokio::test]
async fn bytes_events_are_delivered_per_connection() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let phone = ctx.store.remote_device_pair("phone").await.unwrap();
    let ipad = ctx.store.remote_device_pair("ipad").await.unwrap();
    {
        let mut toks = ctx.remote_tokens.write().unwrap();
        toks.push((phone.token.clone(), Some("phone".into())));
        toks.push((ipad.token.clone(), Some("ipad".into())));
    }
    let addr = serve(ctx.clone()).await;

    let (mut ws_p, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/?token={}", phone.token))
            .await
            .expect("phone connects");
    let (mut ws_i, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/?token={}", ipad.token))
            .await
            .expect("ipad connects");

    session_for_device(&ctx, "phone")
        .await
        .watched_bytes
        .lock()
        .unwrap()
        .insert("lane-1".to_string());
    session_for_device(&ctx, "ipad")
        .await
        .watched_bytes
        .lock()
        .unwrap()
        .insert("lane-2".to_string());

    // Subscribe both and drain the subscribe acks (forwarding is on once the ack returns).
    for (ws, id) in [(&mut ws_p, 1u64), (&mut ws_i, 2u64)] {
        ws.send(Message::text(
            json!({"jsonrpc":"2.0","id":id,"method":"subscribe"}).to_string(),
        ))
        .await
        .unwrap();
        let ack = recv_json(ws).await;
        assert_eq!(ack["id"], json!(id));
    }

    // Bytes for each window, then a non-bytes topic that must reach everyone.
    ctx.broadcast(
        "event.agent.bytes",
        json!({ "lane_id": 1, "window": "lane-1", "data": "QQ==" }),
    );
    ctx.broadcast(
        "event.agent.bytes",
        json!({ "lane_id": 2, "window": "lane-2", "data": "Qg==" }),
    );
    ctx.broadcast("event.repo.changed", json!({ "hello": true }));

    // The phone sees ONLY lane-1's bytes (lane-2's are filtered out), then the shared event.
    let p1 = recv_json(&mut ws_p).await;
    assert_eq!(p1["method"], json!("event.agent.bytes"));
    assert_eq!(p1["params"]["window"], json!("lane-1"));
    let p2 = recv_json(&mut ws_p).await;
    assert_eq!(p2["method"], json!("event.repo.changed"));

    let i1 = recv_json(&mut ws_i).await;
    assert_eq!(i1["method"], json!("event.agent.bytes"));
    assert_eq!(i1["params"]["window"], json!("lane-2"));
    let i2 = recv_json(&mut ws_i).await;
    assert_eq!(i2["method"], json!("event.repo.changed"));
}

/// Verify requested viewport delivery, no output before a viewport claim, and unchanged delivery of
/// other topics.
#[tokio::test]
async fn output_events_are_delivered_per_connection() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let phone = ctx.store.remote_device_pair("phone").await.unwrap();
    let ipad = ctx.store.remote_device_pair("ipad").await.unwrap();
    let laptop = ctx.store.remote_device_pair("laptop").await.unwrap();
    {
        let mut toks = ctx.remote_tokens.write().unwrap();
        toks.push((phone.token.clone(), Some("phone".into())));
        toks.push((ipad.token.clone(), Some("ipad".into())));
        toks.push((laptop.token.clone(), Some("laptop".into())));
    }
    let addr = serve(ctx.clone()).await;

    let (mut ws_p, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/?token={}", phone.token))
            .await
            .expect("phone connects");
    let (mut ws_i, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/?token={}", ipad.token))
            .await
            .expect("ipad connects");
    let (mut ws_l, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/?token={}", laptop.token))
            .await
            .expect("laptop connects");

    // Phone's viewport covers lane 1, iPad's covers lane 2; laptop never asserts a viewport, so its
    // `output_filter` stays at its empty session-creation zero-state.
    session_for_device(&ctx, "phone")
        .await
        .output_filter
        .lock()
        .unwrap()
        .0
        .insert(1);
    session_for_device(&ctx, "ipad")
        .await
        .output_filter
        .lock()
        .unwrap()
        .0
        .insert(2);

    // Subscribe all three and drain the acks (forwarding is on once the ack returns).
    for (ws, id) in [(&mut ws_p, 1u64), (&mut ws_i, 2u64), (&mut ws_l, 3u64)] {
        ws.send(Message::text(
            json!({"jsonrpc":"2.0","id":id,"method":"subscribe"}).to_string(),
        ))
        .await
        .unwrap();
        let ack = recv_json(ws).await;
        assert_eq!(ack["id"], json!(id));
    }

    // Output for each lane, then a non-output topic that must reach everyone.
    ctx.broadcast(
        "event.agent.output",
        json!({ "lane_id": 1, "window": "lane-1", "content": "one" }),
    );
    ctx.broadcast(
        "event.agent.output",
        json!({ "lane_id": 2, "window": "lane-2", "content": "two" }),
    );
    ctx.broadcast("event.repo.changed", json!({ "hello": true }));

    // The phone sees ONLY lane 1's output (lane 2's is filtered out), then the shared event.
    let p1 = recv_json(&mut ws_p).await;
    assert_eq!(p1["method"], json!("event.agent.output"));
    assert_eq!(p1["params"]["lane_id"], json!(1));
    let p2 = recv_json(&mut ws_p).await;
    assert_eq!(p2["method"], json!("event.repo.changed"));

    let i1 = recv_json(&mut ws_i).await;
    assert_eq!(i1["method"], json!("event.agent.output"));
    assert_eq!(i1["params"]["lane_id"], json!(2));
    let i2 = recv_json(&mut ws_i).await;
    assert_eq!(i2["method"], json!("event.repo.changed"));

    // The laptop, with no viewport, sees NEITHER output event - its first frame is the shared event.
    let l1 = recv_json(&mut ws_l).await;
    assert_eq!(l1["method"], json!("event.repo.changed"));
}

/// A connection's byte watches die with it: `close_session` runs `unwatch_all`, which drops the
/// windows the connection solely watched and releases it from shared ones (which survive). Observed
/// directly on the shared registry.
#[tokio::test]
async fn close_session_releases_only_this_connections_watches() {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let a = ctx.open_session(ConnKind::Local).await;
    let b = ctx
        .open_session(ConnKind::Remote {
            device: Some("phone".into()),
        })
        .await;

    {
        let mut map = ctx.bytes_watches.lock().await;

        map.insert(
            "lane-1".to_string(),
            WatchEntry {
                lane: 1,
                refs: [a.id, b.id].into_iter().collect(),
                generation: 0,
                sequence: Arc::new(AtomicU64::new(0)),
                grid: None,
            },
        );

        map.insert(
            "lane-2".to_string(),
            WatchEntry {
                lane: 2,
                refs: [a.id].into_iter().collect(),
                generation: 1,
                sequence: Arc::new(AtomicU64::new(0)),
                grid: None,
            },
        );
    }

    ctx.close_session(a.id).await;

    let map = ctx.bytes_watches.lock().await;
    assert!(
        !map.contains_key("lane-2"),
        "A's solo window is stopped when A disconnects"
    );
    let shared = map
        .get("lane-1")
        .expect("B still watches the shared window");
    assert_eq!(
        shared.refs.iter().copied().collect::<Vec<_>>(),
        vec![b.id],
        "A is released from the shared window; B's ref remains"
    );
}

/// A fresh competing focus claim must deny fit, while an uncontested window can resize.
#[tokio::test]
async fn fit_arbitrates_between_two_remote_sessions() {
    if !TmuxRuntime::available() {
        eprintln!("tmux not available; skipping fit arbitration test");
        return;
    }
    let session = format!("repomon-fit-it-{}", std::process::id());
    let config = Config {
        tmux_session: session.clone(),
        ..Default::default()
    };
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);
    let mut events = ctx.events.subscribe();

    let cwd = std::env::temp_dir();
    ctx.backend
        .spawn_named(
            "lane-2",
            &SpawnSpec::new("sh -c 'while :; do printf x; sleep 0.05; done'", &cwd),
        )
        .expect("spawn uncontested window");

    let a = ctx
        .open_session(ConnKind::Remote {
            device: Some("a".into()),
        })
        .await;
    let b = ctx
        .open_session(ConnKind::Remote {
            device: Some("b".into()),
        })
        .await;

    // B "types" - dispatch stamps B's last_interaction before the handler runs, so the (absent
    // "lane-1" window) tmux error is irrelevant to the arbitration under test.
    let _ = rpc::dispatch(
        &ctx,
        &b,
        "agent.send_input",
        Some(json!({ "lane_id": 1, "window": "lane-1", "text": "x" })),
    )
    .await;

    *b.viewport_focus.lock().await = Some((1, "lane-1".to_string()));
    *b.viewport_focus_at.lock().await = Some(std::time::Instant::now());

    // A fits B's fresh-focus, more-recently-driven window → denied (last-interaction-wins).
    let denied = rpc::dispatch(
        &ctx,
        &a,
        "agent.fit",
        Some(json!({ "lane_id": 1, "window": "lane-1", "cols": 100, "rows": 30 })),
    )
    .await
    .unwrap();
    assert_eq!(
        denied["applied"],
        json!(false),
        "A must not resize a window B freshly owns and drove"
    );
    assert!(
        matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ),
        "a denied fit must not announce a grid change"
    );

    let applied = rpc::dispatch(
        &ctx,
        &a,
        "agent.fit",
        Some(json!({ "lane_id": 2, "window": "lane-2", "cols": 100, "rows": 30 })),
    )
    .await
    .unwrap();
    assert_eq!(
        applied["applied"],
        json!(true),
        "A resizes a window nobody else owns"
    );
    let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
        .await
        .expect("grid-change event timeout")
        .expect("grid-change event");
    assert_eq!(event["method"], json!(pubsub::topic::AGENT_GRID));
    assert_eq!(
        event["params"],
        json!({ "lane_id": 2, "window": "lane-2", "cols": 100, "rows": 30 })
    );

    rpc::dispatch(
        &ctx,
        &a,
        "agent.resize",
        Some(json!({ "lane_id": 2, "window": "lane-2", "cols": 101, "rows": 31 })),
    )
    .await
    .unwrap();
    let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
        .await
        .expect("resize grid-change event timeout")
        .expect("resize grid-change event");
    assert_eq!(event["method"], json!(pubsub::topic::AGENT_GRID));
    assert_eq!(
        event["params"],
        json!({ "lane_id": 2, "window": "lane-2", "cols": 101, "rows": 31 })
    );

    // A raw tmux attach/direct resize bypasses both mediated RPCs. The control-mode byte stream
    // must announce its layout change in the same generation/sequence as pane output.
    rpc::dispatch(
        &ctx,
        &a,
        "agent.watch_bytes",
        Some(json!({ "lane_id": 2, "window": "lane-2", "on": true })),
    )
    .await
    .unwrap();
    ctx.backend.resize_named("lane-2", 111, 32).unwrap();
    let external_event = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.expect("event bus remains open");
            if event["method"] == json!(pubsub::topic::AGENT_GRID)
                && event["params"]["cols"] == json!(111)
            {
                break event;
            }
        }
    })
    .await
    .expect("external grid-change event timeout");
    assert_eq!(external_event["params"]["lane_id"], json!(2));
    assert_eq!(external_event["params"]["window"], json!("lane-2"));
    assert_eq!(external_event["params"]["cols"], json!(111));
    assert_eq!(external_event["params"]["rows"], json!(32));
    assert!(external_event["params"]["generation"].is_u64());
    assert!(external_event["params"]["sequence"].is_u64());

    let _ = std::process::Command::new(repomon_core::agent::tmux_program())
        .args(["-L", &session, "kill-server"])
        .output();
}

/// Verify lane-wide watch release preserves other lanes and purges dead registry names so name
/// reuse cannot deliver unrequested bytes.
#[tokio::test]
async fn watch_bytes_off_without_window_releases_only_that_lanes_watches() {
    if !TmuxRuntime::available() {
        eprintln!("tmux not available; skipping watch_bytes handler test");
        return;
    }
    let session = format!("repomon-bytes-it-{}", std::process::id());
    let config = Config {
        tmux_session: session.clone(),
        ..Default::default()
    };
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);

    // Real windows to pipe: two on lane 1 (default-named and a second slot), one on lane 2.
    let cwd = std::env::temp_dir();
    for w in ["lane-1", "lane-1-2", "lane-2"] {
        ctx.backend
            .spawn_named(w, &SpawnSpec::new("sleep 30", &cwd))
            .expect("spawn window");
    }

    let sess = ctx.open_session(ConnKind::Local).await;
    let second_viewer = ctx.open_session(ConnKind::Local).await;

    // Watch both lane-1 windows and the lane-2 window through the real handler.
    for (lane, window) in [(1, "lane-1"), (1, "lane-1-2"), (2, "lane-2")] {
        let ack = rpc::dispatch(
            &ctx,
            &sess,
            "agent.watch_bytes",
            Some(json!({ "lane_id": lane, "window": window, "on": true })),
        )
        .await
        .unwrap_or_else(|e| panic!("watch {window} errored: {e:?}"));
        assert!(ack.get("cols").is_some(), "ack shape carries dims: {ack}");
        assert!(
            ack["generation"].as_u64().is_some(),
            "ack carries a stream generation: {ack}"
        );
        assert_eq!(
            ack["sequence"],
            json!(0),
            "a new pipe starts before its first emitted byte"
        );
    }
    {
        let watched = sess.watched_bytes.lock().unwrap();
        assert_eq!(watched.len(), 3, "on:true records each window: {watched:?}");
    }

    // A second viewer joins lane-2's existing generation instead of replacing its stream.
    let second_ack = rpc::dispatch(
        &ctx,
        &second_viewer,
        "agent.watch_bytes",
        Some(json!({ "lane_id": 2, "window": "lane-2", "on": true })),
    )
    .await
    .expect("second viewer joins lane-2");
    assert!(second_ack["generation"].is_u64());
    {
        let map = ctx.bytes_watches.lock().await;
        assert_eq!(map.len(), 3);
        assert_eq!(map["lane-1"].lane, 1);
        assert_eq!(map["lane-1-2"].lane, 1);
        assert_eq!(map["lane-2"].lane, 2);
        for w in ["lane-1", "lane-1-2", "lane-2"] {
            assert!(map[w].refs.contains(&sess.id), "{w} holds this conn's ref");
        }
        assert!(map["lane-2"].refs.contains(&second_viewer.id));
    }

    // A stale name: watched by the session, but its registry entry already died (EOF-cleaned).
    sess.watched_bytes
        .lock()
        .unwrap()
        .insert("lane-ghost".to_string());

    // The TUI's stop shape: no window. Releases BOTH lane-1 windows (lane matched by entry field,
    // so the non-default lane-1-2 is found too), leaves lane-2 alone, and purges the dead name.
    rpc::dispatch(
        &ctx,
        &sess,
        "agent.watch_bytes",
        Some(json!({ "lane_id": 1, "on": false })),
    )
    .await
    .unwrap();

    {
        let map = ctx.bytes_watches.lock().await;
        assert!(
            !map.contains_key("lane-1"),
            "lane 1's default window released"
        );
        assert!(
            !map.contains_key("lane-1-2"),
            "lane 1's second window released"
        );
        let survivor = map.get("lane-2").expect("lane 2's watch survives");
        assert!(survivor.refs.contains(&sess.id));
        assert!(survivor.refs.contains(&second_viewer.id));
    }
    {
        let watched = sess.watched_bytes.lock().unwrap();
        assert_eq!(
            watched.iter().cloned().collect::<Vec<_>>(),
            vec!["lane-2".to_string()],
            "watched_bytes reflects the release, including the purged stale name"
        );
    }

    // The shared control stream must remain live until its last viewer releases it.
    rpc::dispatch(
        &ctx,
        &sess,
        "agent.watch_bytes",
        Some(json!({ "lane_id": 2, "window": "lane-2", "on": false })),
    )
    .await
    .unwrap();
    assert!(ctx.bytes_watches.lock().await.contains_key("lane-2"));
    rpc::dispatch(
        &ctx,
        &second_viewer,
        "agent.watch_bytes",
        Some(json!({ "lane_id": 2, "window": "lane-2", "on": false })),
    )
    .await
    .unwrap();
    assert!(!ctx.bytes_watches.lock().await.contains_key("lane-2"));

    let _ = std::process::Command::new(repomon_core::agent::tmux_program())
        .args(["-L", &session, "kill-server"])
        .output();
}

/// Terminate a watched stream when its target dies even if sibling windows keep the session-scoped
/// control client alive.
#[tokio::test]
async fn watched_window_death_closes_stream_while_sibling_survives() {
    if !TmuxRuntime::available() {
        eprintln!("tmux not available; skipping watch close test");
        return;
    }
    let session = format!("repomon-bytes-close-it-{}", std::process::id());
    let config = Config {
        tmux_session: session.clone(),
        ..Default::default()
    };
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, config, None);
    let cwd = std::env::temp_dir();
    for window in ["lane-1", "lane-2"] {
        ctx.backend
            .spawn_named(window, &SpawnSpec::new("sleep 30", &cwd))
            .expect("spawn window");
    }

    let sess = ctx.open_session(ConnKind::Local).await;
    let mut events = ctx.events.subscribe();
    let ack = rpc::dispatch(
        &ctx,
        &sess,
        "agent.watch_bytes",
        Some(json!({ "lane_id": 1, "window": "lane-1", "on": true })),
    )
    .await
    .expect("start byte watch");
    let generation = ack["generation"].as_u64().expect("stream generation");

    ctx.backend
        .kill_named("lane-1")
        .expect("kill watched window");
    let closed = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let event = events.recv().await.expect("event bus remains open");
            if event["method"] == json!(pubsub::topic::AGENT_STREAM_CLOSED) {
                break event;
            }
        }
    })
    .await
    .expect("stream-close event within 3s");
    assert_eq!(closed["params"]["window"], json!("lane-1"));
    assert_eq!(closed["params"]["generation"], json!(generation));
    assert!(!ctx.bytes_watches.lock().await.contains_key("lane-1"));
    assert!(
        ctx.backend
            .list_windows()
            .unwrap()
            .iter()
            .any(|window| window == "lane-2"),
        "sibling window should keep the tmux session alive"
    );

    let clients = std::process::Command::new(repomon_core::agent::tmux_program())
        .args([
            "-L",
            &session,
            "list-clients",
            "-F",
            "#{client_control_mode}",
        ])
        .output()
        .expect("list tmux clients");
    assert!(
        clients.stdout.is_empty(),
        "target death leaked a control client: {:?}",
        String::from_utf8_lossy(&clients.stdout)
    );

    // The client consumes the close event, then releases its per-connection watch bookkeeping.
    rpc::dispatch(
        &ctx,
        &sess,
        "agent.watch_bytes",
        Some(json!({ "lane_id": 1, "window": "lane-1", "on": false })),
    )
    .await
    .expect("release closed watch");
    assert!(!sess.watched_bytes.lock().unwrap().contains("lane-1"));

    let _ = std::process::Command::new(repomon_core::agent::tmux_program())
        .args(["-L", &session, "kill-server"])
        .output();
}
