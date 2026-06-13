//! End-to-end for the remote WebSocket bridge: token gate, RPC round-trip, event push.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use repomon_core::{Config, Store};
use repomon_daemon::{remote, Ctx};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

/// A free localhost port (bind :0, read it back, release).
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn start_bridge(token: &str) -> (Arc<Ctx>, String) {
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let addr = format!("127.0.0.1:{}", free_port());
    {
        let ctx = ctx.clone();
        let addr = addr.clone();
        let token = token.to_string();
        tokio::spawn(async move { remote::serve_remote(ctx, &addr, token).await });
    }
    // Wait for the listener to come up (connect attempts, not sleeps).
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    (ctx, addr)
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
