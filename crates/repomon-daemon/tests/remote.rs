//! End-to-end for the remote WebSocket bridge: token gate, RPC round-trip, event push.

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
async fn recv_json(ws: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin)) -> Value {
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

    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/?token={}", dev.token))
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
    assert!(stamped, "a named device's last_seen_at is stamped on connect");
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
    assert!(closed, "the bridge closes the socket after a revoked request");
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
    let devices = rpc::dispatch(&ctx, &sess, "remote.devices", None).await.unwrap();
    let d0 = &devices.as_array().unwrap()[0];
    assert_eq!(d0["name"], json!("phone"));
    assert_eq!(d0["role"], json!("full"));
    assert!(d0.get("token").is_none(), "the listing never exposes the token");

    // revoke → {revoked:true}, and the auth cache empties.
    let rev = rpc::dispatch(&ctx, &sess, "remote.revoke", Some(json!({ "name": "phone" })))
        .await
        .unwrap();
    assert_eq!(rev["revoked"], json!(true));
    assert!(ctx.remote_tokens.read().unwrap().is_empty());

    // revoking again → {revoked:false}.
    let rev2 = rpc::dispatch(&ctx, &sess, "remote.revoke", Some(json!({ "name": "phone" })))
        .await
        .unwrap();
    assert_eq!(rev2["revoked"], json!(false));
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

    let (mut ws_p, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/?token={}", phone.token))
        .await
        .expect("phone connects");
    let (mut ws_i, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/?token={}", ipad.token))
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
    let shared = map.get("lane-1").expect("B still watches the shared window");
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
