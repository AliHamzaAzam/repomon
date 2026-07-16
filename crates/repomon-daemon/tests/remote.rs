//! End-to-end for the remote WebSocket bridge: token gate, RPC round-trip, event push.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use repomon_core::{Config, Store};
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

    // pair → {name, token, url}; seeds the auth cache.
    let pair = rpc::dispatch(&ctx, "remote.pair", Some(json!({ "name": "phone" })))
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
    let again = rpc::dispatch(&ctx, "remote.pair", Some(json!({ "name": "phone" })))
        .await
        .unwrap();
    assert_eq!(pair["token"], again["token"]);
    assert_eq!(ctx.remote_tokens.read().unwrap().len(), 1);

    // devices lists the device WITHOUT the token.
    let devices = rpc::dispatch(&ctx, "remote.devices", None).await.unwrap();
    let d0 = &devices.as_array().unwrap()[0];
    assert_eq!(d0["name"], json!("phone"));
    assert_eq!(d0["role"], json!("full"));
    assert!(d0.get("token").is_none(), "the listing never exposes the token");

    // revoke → {revoked:true}, and the auth cache empties.
    let rev = rpc::dispatch(&ctx, "remote.revoke", Some(json!({ "name": "phone" })))
        .await
        .unwrap();
    assert_eq!(rev["revoked"], json!(true));
    assert!(ctx.remote_tokens.read().unwrap().is_empty());

    // revoking again → {revoked:false}.
    let rev2 = rpc::dispatch(&ctx, "remote.revoke", Some(json!({ "name": "phone" })))
        .await
        .unwrap();
    assert_eq!(rev2["revoked"], json!(false));
}
