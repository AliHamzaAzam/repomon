//! Regression test for the "daemon connection closed" bug.
//!
//! The daemon reaps idle client connections after 120s (see `repomon-daemon` socket.rs). A
//! `DaemonClient` must transparently reconnect on its next `call` instead of failing every
//! RPC forever once its connection is dropped — the failure that bricked the MCP bridge's
//! action tools (`read_agent`, `send_to_agent`, …) while subscription-fed reads kept working.

use std::time::Duration;

use repomon_core::client::DaemonClient;
use repomon_core::protocol::{Notification, Request, Response, read_frame, write_message};
use serde_json::json;
use tokio::net::{UnixListener, UnixStream};

/// Answer every request frame on `stream` with `Response::ok(id, "pong")` until the peer closes.
async fn serve_pings(stream: UnixStream) {
    let (mut rd, mut wr) = stream.into_split();
    while let Ok(Some(frame)) = read_frame(&mut rd).await {
        if let Ok(req) = serde_json::from_slice::<Request>(&frame) {
            let resp = Response::ok(req.id, json!("pong"));
            if write_message(&mut wr, &resp).await.is_err() {
                break;
            }
        }
    }
}

#[tokio::test]
async fn call_reconnects_after_daemon_drops_connection() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("daemon.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    // Mock daemon: serve exactly one ping on the first connection, then drop it (the 120s reap).
    // A correct client reconnects, and the second connection is served normally.
    tokio::spawn(async move {
        let (s1, _) = listener.accept().await.unwrap();
        {
            let (mut rd, mut wr) = s1.into_split();
            if let Ok(Some(frame)) = read_frame(&mut rd).await {
                let req: Request = serde_json::from_slice(&frame).unwrap();
                let _ = write_message(&mut wr, &Response::ok(req.id, json!("pong"))).await;
            }
            // s1 is dropped here -> the client's connection closes, simulating the reap.
        }
        let (s2, _) = listener.accept().await.unwrap();
        serve_pings(s2).await;
    });

    let client = DaemonClient::connect(&sock).await.unwrap();

    // First call rides the original connection.
    let r1 = client.call("ping", None).await.expect("first call");
    assert_eq!(r1, json!("pong"));

    // Let the client observe the dropped connection.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The daemon reaped the connection. The client must reconnect transparently. Before the fix
    // this fails with "daemon connection closed" or hangs on the dead connection until timeout.
    let r2 = tokio::time::timeout(Duration::from_secs(5), client.call("ping", None))
        .await
        .expect("call must not hang on a dead connection")
        .expect("second call should reconnect and succeed");
    assert_eq!(r2, json!("pong"));
}

/// Answer every request by id, and if the request is `subscribe`, also push one
/// `event.test` notification — standing in for the daemon's per-connection `forwarding` flag
/// (only connections that sent `subscribe` get events; see `repomon-daemon` socket.rs).
async fn serve_and_notify_on_subscribe(stream: UnixStream) {
    let (mut rd, mut wr) = stream.into_split();
    while let Ok(Some(frame)) = read_frame(&mut rd).await {
        let Ok(req) = serde_json::from_slice::<Request>(&frame) else {
            continue;
        };
        if write_message(&mut wr, &Response::ok(req.id, json!("ok")))
            .await
            .is_err()
        {
            break;
        }
        if req.method == "subscribe" {
            let note = Notification::new("event.test", json!({}));
            if write_message(&mut wr, &note).await.is_err() {
                break;
            }
        }
    }
}

#[tokio::test]
async fn subscribe_survives_reconnect() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("daemon.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    // Mock daemon: connection 1 acks the `subscribe` call (like the real daemon) and is then
    // dropped, simulating the 120s reap. Connection 2 is where the client must replay
    // `subscribe` on its own — the daemon only forwards events on connections that asked.
    tokio::spawn(async move {
        let (s1, _) = listener.accept().await.unwrap();
        {
            let (mut rd, mut wr) = s1.into_split();
            if let Ok(Some(frame)) = read_frame(&mut rd).await {
                let req: Request = serde_json::from_slice(&frame).unwrap();
                assert_eq!(req.method, "subscribe");
                let _ = write_message(&mut wr, &Response::ok(req.id, json!("ok"))).await;
            }
            // s1 is dropped here -> the client's connection closes, simulating the reap.
        }
        let (s2, _) = listener.accept().await.unwrap();
        serve_and_notify_on_subscribe(s2).await;
    });

    let client = DaemonClient::connect(&sock).await.unwrap();

    // Subscribe before the drop, and hold onto the receiver across the reconnect below.
    let mut events = client.subscribe();
    client
        .call("subscribe", None)
        .await
        .expect("subscribe call");

    // Let the client observe the dropped connection.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Any call triggers a transparent reconnect, which must replay `subscribe` on the new
    // connection so the daemon resumes forwarding events to it.
    client
        .call("ping", None)
        .await
        .expect("call after reconnect");

    // The original receiver (obtained before the drop) must still see events after reconnect.
    let note = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("subscribe must survive reconnect: no notification within 5s")
        .expect("broadcast channel should not be closed");
    assert_eq!(note.method, "event.test");
}
