//! End-to-end for the remote WebSocket bridge: token gate, RPC round-trip, event push.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use repomon_core::{Config, Store, TmuxRuntime};
use repomon_daemon::bytes_stream::WatchEntry;
use repomon_daemon::conn::{ConnKind, ConnSession};
use repomon_daemon::{Ctx, remote, rpc};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

/// A free localhost port (bind :0, read it back, release).
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Serve a prepared `ctx` on a fresh localhost port and wait for the listener. Tokens must already
/// be seeded into `ctx.remote_tokens` (that is now the auth source, not a serve_remote argument).
async fn serve(ctx: Arc<Ctx>) -> String {
    let addr = format!("127.0.0.1:{}", free_port());
    {
        let ctx = ctx.clone();
        let addr = addr.clone();
        tokio::spawn(async move { remote::serve_remote(ctx, &addr).await });
    }
    // Wait for the listener to come up (connect attempts, not sleeps).
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    addr
}

async fn start_bridge(token: &str) -> (Arc<Ctx>, String) {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    // Seed a shared token (device name None) — the legacy config-token path.
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

    // ping → pong.
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

    // A real method works over the bridge.
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

    // subscribe, then a broadcast arrives as an event frame.
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

    // The handshake stamps last_seen_at for the named device (poll — it happens in the handler).
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

    // A request works while the token is live.
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
    // Server closed the connection after the error.
    let closed = matches!(
        ws.next().await,
        None | Some(Ok(Message::Close(_))) | Some(Err(_))
    );
    assert!(
        closed,
        "the bridge closes the socket after a revoked request"
    );
}

/// Passive revocation: a device that subscribes and watches, then is revoked while sending NO
/// further requests, must stop receiving events. The request-arm revocation check never fires for
/// a silent device, so the event-forward arm has to re-check the token itself and close the socket
/// within one event delivery.
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

    // A pre-revocation broadcast is delivered — the stream is live.
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

    // pair → {name, token, url}; seeds the auth cache.
    let pair = rpc::dispatch(&ctx, &sess, "remote.pair", Some(json!({ "name": "phone" })))
        .await
        .unwrap();
    assert_eq!(pair["name"], json!("phone"));
    assert!(pair["token"].as_str().unwrap().len() >= 32);
    assert!(pair["url"].as_str().unwrap().starts_with("repomon://"));
    assert!(
        pair["url"].as_str().unwrap().contains("&name=phone"),
        "the pairing url carries the device name"
    );
    assert_eq!(ctx.remote_tokens.read().unwrap().len(), 1);

    // re-pair the same name is idempotent (same token, no second cache entry).
    let again = rpc::dispatch(&ctx, &sess, "remote.pair", Some(json!({ "name": "phone" })))
        .await
        .unwrap();
    assert_eq!(pair["token"], again["token"]);
    assert_eq!(ctx.remote_tokens.read().unwrap().len(), 1);

    // devices lists the device WITHOUT the token.
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

    // revoke → {revoked:true}, and the auth cache empties.
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

    // revoking again → {revoked:false}.
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

/// Auth-cache refresh race (Finding 4): a `remote.pair` and a `remote.revoke` running at once must
/// leave the cache consistent with the store. `refresh_remote_tokens` is read-then-write, so an
/// unserialized pair could rebuild from a pre-revoke snapshot and resurrect the revoked token. With
/// the mutate lock serializing each mutate+refresh, the final cache always mirrors the store — the
/// revoked device is gone and the paired one is present, regardless of interleaving.
#[tokio::test]
async fn concurrent_pair_and_revoke_leave_the_cache_consistent() {
    use std::collections::HashSet;

    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sess = ctx.open_session(ConnKind::Local).await;

    // Start with device "a" paired and in the cache.
    rpc::dispatch(&ctx, &sess, "remote.pair", Some(json!({ "name": "a" })))
        .await
        .unwrap();

    // Concurrently pair "b" and revoke "a".
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

/// Defense-in-depth (Finding 5): a remote `lane.create` must NOT honor a caller-supplied `path` —
/// the bridge withholds `fs.browse`, so a paired device has no legitimate way to have chosen one.
/// The daemon strips it and derives the template worktree location; a LOCAL caller is unaffected.
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
    // Canonicalize the base: the daemon returns canonical worktree paths, and on macOS the tempdir
    // lives under a `/private` symlink, so a raw join wouldn't compare equal.
    let outside_base = std::fs::canonicalize(outside.path()).unwrap();
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

    // Sanity: a LOCAL caller's path IS honored — the strip is remote-only.
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

/// Two connections, each byte-watching a DIFFERENT window, receive ONLY their own window's
/// `event.agent.bytes` — while every non-bytes topic reaches both. This is the per-connection
/// delivery filter at the forwarding loop, exercised end to end over the real bridge. (The pipe
/// machinery itself is unit-tested; we seed each session's `watched_bytes` directly here so the
/// test needs no live tmux.)
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

    // Each connection watches a different window.
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

    // The iPad sees ONLY lane-2's bytes, then the shared event.
    let i1 = recv_json(&mut ws_i).await;
    assert_eq!(i1["method"], json!("event.agent.bytes"));
    assert_eq!(i1["params"]["window"], json!("lane-2"));
    let i2 = recv_json(&mut ws_i).await;
    assert_eq!(i2["method"], json!("event.repo.changed"));
}

/// Sibling of `bytes_events_are_delivered_per_connection` for A5: two connections with DIFFERENT
/// viewports each receive ONLY their own lane's `event.agent.output`, a third connection that never
/// asserted a viewport receives NONE, and every connection still gets a non-output topic. This is
/// the per-connection output filter at the forwarding loop, end to end over the real bridge. We seed
/// each session's `output_filter` snapshot directly (the same snapshot `viewport.set` writes) so the
/// test needs no live tmux.
///
/// The no-viewport case is the whole point of A5: TODAY's shipping iPhone app never calls
/// `viewport.set` and never consumes `event.agent.output` (it polls `agent.capture`), so filtering
/// it to nothing wastes none of its bandwidth and drops nothing it relies on.
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

    // The iPad sees ONLY lane 2's output, then the shared event.
    let i1 = recv_json(&mut ws_i).await;
    assert_eq!(i1["method"], json!("event.agent.output"));
    assert_eq!(i1["params"]["lane_id"], json!(2));
    let i2 = recv_json(&mut ws_i).await;
    assert_eq!(i2["method"], json!("event.repo.changed"));

    // The laptop, with no viewport, sees NEITHER output event — its first frame is the shared event.
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
        // Shared window watched by both A and B.
        map.insert(
            "lane-1".to_string(),
            WatchEntry {
                lane: 1,
                fifo: std::env::temp_dir().join("repomon-test-a4-1.fifo"),
                refs: [a.id, b.id].into_iter().collect(),
                generation: 0,
            },
        );
        // A window only A watches.
        map.insert(
            "lane-2".to_string(),
            WatchEntry {
                lane: 2,
                fifo: std::env::temp_dir().join("repomon-test-a4-2.fifo"),
                refs: [a.id].into_iter().collect(),
                generation: 1,
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

/// `agent.fit` arbitration through real dispatch (covers the A3 wiring: interaction stamping +
/// cross-session snapshots). Session B drives an agent (stamping its `last_interaction`) and holds
/// a fresh focus on a window; session A's fit on that window is denied, while A's fit on an
/// uncontested window applies. tmux-gated (the apply path resizes a real pane).
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

    // A real, uncontested window for the apply case.
    let cwd = std::env::temp_dir();
    ctx.tmux
        .spawn_named("lane-2", &cwd, "sleep 30")
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

    // B "types" — dispatch stamps B's last_interaction before the handler runs, so the (absent
    // "lane-1" window) tmux error is irrelevant to the arbitration under test.
    let _ = rpc::dispatch(
        &ctx,
        &b,
        "agent.send_input",
        Some(json!({ "lane_id": 1, "window": "lane-1", "text": "x" })),
    )
    .await;
    // ...and holds a fresh focus beat on the contested window.
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

    // A fits the uncontested real window → applied.
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

    let _ = std::process::Command::new("tmux")
        .args(["-L", &session, "kill-server"])
        .output();
}

/// The `agent.watch_bytes` handler through real dispatch: `on:true` starts real pipes and records
/// the windows in the session; `{lane_id, on:false}` with NO window (the TUI's stop path) releases
/// exactly this session's watches on that lane — matched by the WatchEntry.lane field, so a
/// non-default window is found too — while another lane's watch survives. Also covers the
/// stale-name purge: a watched name whose registry entry already died is dropped from
/// `watched_bytes` so later window-name reuse can't deliver unrequested bytes. tmux-gated (the
/// on:true path runs mkfifo + pipe-pane against real windows).
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
        ctx.tmux
            .spawn_named(w, &cwd, "sleep 30")
            .expect("spawn window");
    }

    let sess = ctx.open_session(ConnKind::Local).await;

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
    }
    {
        let watched = sess.watched_bytes.lock().unwrap();
        assert_eq!(watched.len(), 3, "on:true records each window: {watched:?}");
    }
    {
        let map = ctx.bytes_watches.lock().await;
        assert_eq!(map.len(), 3);
        assert_eq!(map["lane-1"].lane, 1);
        assert_eq!(map["lane-1-2"].lane, 1);
        assert_eq!(map["lane-2"].lane, 2);
        for w in ["lane-1", "lane-1-2", "lane-2"] {
            assert!(map[w].refs.contains(&sess.id), "{w} holds this conn's ref");
        }
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
    }
    {
        let watched = sess.watched_bytes.lock().unwrap();
        assert_eq!(
            watched.iter().cloned().collect::<Vec<_>>(),
            vec!["lane-2".to_string()],
            "watched_bytes reflects the release, including the purged stale name"
        );
    }

    let _ = std::process::Command::new("tmux")
        .args(["-L", &session, "kill-server"])
        .output();
}
