//! Serves local framed JSON-RPC over Unix sockets or Windows pipes. Independent reading and event
//! forwarding keep slow RPCs from blocking terminal events; one writer serializes complete frames.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use repomon_core::protocol::{self, Request, Response, RpcError};
use repomon_core::transport::{self, Endpoint, IpcListener, IpcStream};
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::{Ctx, rpc};

/// Bounds silent-reader lifetime so abandoned connections cannot accumulate tasks.
const READ_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Bind the local IPC endpoint and serve until shutdown is requested. `socket_path` is the
/// socket file on unix and is interpreted as a named-pipe name on Windows (stale-file cleanup
/// and parent-dir creation happen inside `transport::listen`; pipes need neither).
pub async fn serve(ctx: Arc<Ctx>, socket_path: &Path) -> std::io::Result<()> {
    let endpoint = Endpoint::from_path(socket_path);
    let listener = transport::listen(&endpoint).await?;
    serve_listener(ctx, socket_path, listener).await
}

/// Serves an already-bound IPC listener until shutdown, removing its Unix socket file on exit.
pub async fn serve_listener(
    ctx: Arc<Ctx>,
    socket_path: &Path,
    listener: IpcListener,
) -> std::io::Result<()> {
    serve_with_watchdog(ctx, socket_path, listener, Duration::from_secs(600)).await
}

async fn serve_with_watchdog(
    ctx: Arc<Ctx>,
    socket_path: &Path,
    mut listener: IpcListener,
    interval: Duration,
) -> std::io::Result<()> {
    tracing::info!("listening on {}", socket_path.display());

    #[cfg(unix)]
    let mut identity = listener.bound_identity();
    let mut watchdog = tokio::time::interval(interval);
    watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut touched = std::time::Instant::now();
    loop {
        tokio::select! {
            _ = watchdog.tick() => {
                #[cfg(unix)]
                match rebind_if_lost(socket_path, &mut listener, &mut identity).await {
                    Ok(true) => ctx.broadcast("event.daemon.rebound", serde_json::json!({ "socket": socket_path })),
                    Ok(false) => {},
                    Err(error) => tracing::error!(%error, "daemon socket recovery failed; will retry"),
                }
                let backend = ctx.backend.clone();
                tokio::task::spawn_blocking(move || {
                    if let Err(error) = backend.maintain_socket() {
                        tracing::error!(%error, "tmux socket watchdog failed; fleet replacement remains blocked");
                    }
                });
                if touched.elapsed() >= Duration::from_secs(86400) {
                    if let Err(error) = repomon_core::agent::tmux_socket::touch_socket(socket_path) {
                        tracing::warn!(%error, "could not refresh daemon socket timestamps");
                    }
                    touched = std::time::Instant::now();
                }
            },
            _ = ctx.shutdown.notified() => break,
            accepted = listener.accept() => match accepted {
                Ok(stream) => {
                    let ctx = ctx.clone();
                    tokio::spawn(async move { handle_conn(ctx, stream).await });
                }
                Err(e) => tracing::warn!("accept error: {e}"),
            },
        }
    }

    // Remove the socket file so the next daemon start binds cleanly (pipes vanish on close).
    #[cfg(unix)]
    if socket_identity(socket_path).ok() == Some(identity) {
        let _ = std::fs::remove_file(socket_path);
    }
    Ok(())
}

#[cfg(unix)]
fn socket_identity(path: &Path) -> std::io::Result<(u64, u64)> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket() {
        return Err(std::io::Error::other("listener path is not a socket"));
    }
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(unix)]
async fn rebind_if_lost(
    path: &Path,
    listener: &mut IpcListener,
    identity: &mut (u64, u64),
) -> std::io::Result<bool> {
    if socket_identity(path).ok() == Some(*identity) {
        return Ok(false);
    }
    tracing::error!(path = %path.display(), "daemon listener path was removed or replaced; rebinding");
    // listen refuses to unlink a live replacement; established connections keep their streams.
    let replacement = transport::listen(&Endpoint::from_path(path)).await?;
    *identity = replacement.bound_identity();
    *listener = replacement;
    Ok(true)
}

async fn handle_conn(ctx: Arc<Ctx>, stream: IpcStream) {
    let (mut read_half, mut write_half) = tokio::io::split(stream);

    // Reader task: read_frame to completion, hand frames to the connection task.
    let (in_tx, mut in_rx) = mpsc::channel::<Vec<u8>>(128);
    let reader_ctx = ctx.clone();
    tokio::spawn(async move {
        // Cancellation may interrupt a partial frame, so shutdown or timeout must abandon this
        // reader permanently rather than resume parsing the same stream.
        loop {
            let frame = tokio::select! {
                _ = reader_ctx.shutdown.notified() => break,
                read = protocol::read_frame(&mut read_half) => match read {
                    Ok(Some(frame)) => frame,
                    _ => break, // clean EOF or read error
                },
                _ = tokio::time::sleep(READ_IDLE_TIMEOUT) => break,
            };
            if in_tx.send(frame).await.is_err() {
                break;
            }
        }
    });

    // This connection's per-device session (viewport/focus/fit state). The guard drops it from
    // `ctx.sessions` on every exit path below.
    let sess = ctx.open_session(crate::conn::ConnKind::Local).await;
    let _session_guard = crate::conn::SessionGuard::new(ctx.clone(), sess.id);

    // A single writer task owns the write half; both the RPC responder and the event forwarder hand
    // it already-serialized frames over this channel, so writes never interleave mid-frame and
    // neither side blocks the other on a slow write.
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(1024);
    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if protocol::write_frame(&mut write_half, &frame)
                .await
                .is_err()
            {
                break;
            }
        }
    });

    let forwarding = Arc::new(AtomicBool::new(false));

    // Drain events independently of RPC dispatch so slow overlays or response writes do not
    // overflow the broadcast buffer.
    let forwarder = {
        let mut events = ctx.events.subscribe();
        let forwarding = forwarding.clone();
        let sess = sess.clone();
        let out_tx = out_tx.clone();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(value) => {
                        if !forwarding.load(Ordering::Relaxed) {
                            continue;
                        }
                        // Per-connection filtering: `event.agent.bytes` reaches only the connections
                        // that watch its window, `event.agent.output` only those whose viewport
                        // covers its lane/window; every other topic forwards unchanged.
                        let deliver = {
                            let watched = sess.watched_bytes.lock().unwrap();
                            let out = sess.output_filter.lock().unwrap();
                            crate::transcript::deliver_to(&value, sess.id)
                                && crate::pubsub::deliver_to(&value, &watched, &out.0, &out.1)
                        };
                        if deliver {
                            if let Ok(bytes) = serde_json::to_vec::<Value>(&value) {
                                if out_tx.send(bytes).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Err(RecvError::Lagged(n)) => tracing::debug!("subscriber lagged {n} events"),
                    Err(RecvError::Closed) => break,
                }
            }
        })
    };

    // Independent reads may overtake other requests. Everything else, including input,
    // viewport changes and watch/unwatch, executes in wire order. Clients match replies by ID.
    let mut requests = tokio::task::JoinSet::new();
    // One tail per ordering key. Each carries its predecessor's method and duration so a stalled
    // successor can name what held its chain.
    let mut ordered_tails: std::collections::HashMap<
        OrderKey,
        tokio::sync::oneshot::Receiver<(String, f64)>,
    > = std::collections::HashMap::new();
    while let Some(frame) = in_rx.recv().await {
        while requests.try_join_next().is_some() {}
        if requests.len() >= 128 {
            requests.join_next().await;
        }
        let req: Request = match serde_json::from_slice(&frame) {
            Ok(r) => r,
            Err(e) => {
                let resp = Response::err(None, RpcError::new(-32700, format!("parse error: {e}")));
                if out_tx
                    .send(serde_json::to_vec(&resp).unwrap_or_default())
                    .await
                    .is_err()
                {
                    break;
                }
                continue;
            }
        };
        // A local request means a UI is actively watching; refresh the heartbeat (or zero it on the
        // explicit `watcher.park`) so the notification engine knows when to take over desktop popups.
        if req.method == "watcher.park" {
            *ctx.local_watcher_seen.lock().await = None;
        } else {
            *ctx.local_watcher_seen.lock().await = Some(std::time::Instant::now());
        }
        if req.method == "subscribe" {
            forwarding.store(true, Ordering::Relaxed);
        }
        // The oneshot carries the predecessor's identity so a stalled successor can name what
        // held the chain; see `chat_open_trace::ordered_wait`.
        let (previous, done) = if independent_read(&req.method) {
            (None, None)
        } else {
            let (done, next) = tokio::sync::oneshot::channel::<(String, f64)>();
            let key = ordering_key(&req.method, &req.params);
            (ordered_tails.insert(key, next), Some(done))
        };
        let ctx = ctx.clone();
        let sess = sess.clone();
        let out_tx = out_tx.clone();
        requests.spawn(async move {
            let queued = std::time::Instant::now();
            if let Some(previous) = previous {
                if let Ok((ahead, ahead_ms)) = previous.await {
                    crate::chat_open_trace::ordered_wait(
                        &req.method,
                        queued.elapsed().as_secs_f64() * 1000.0,
                        &ahead,
                        ahead_ms,
                    );
                }
            }
            let method = req.method.clone();
            let own = std::time::Instant::now();
            // Report this request while it is still the chain head. Aborted the moment dispatch
            // returns, so only a request that outlives its own ceiling ever writes a line.
            let watchdog = {
                let method = method.clone();
                let conn = sess.id;
                tokio::spawn(async move {
                    for ms in CHAIN_HEAD_ALARMS {
                        tokio::time::sleep_until(
                            (own + std::time::Duration::from_millis(ms)).into(),
                        )
                        .await;
                        crate::chat_open_trace::chain_head(&method, ms, conn);
                    }
                })
            };
            let resp = match rpc::dispatch(&ctx, &sess, &req.method, req.params).await {
                Ok(value) => Response::ok(req.id, value),
                Err(err) => Response::err(req.id, err),
            };
            watchdog.abort();
            let ran = own.elapsed().as_secs_f64() * 1000.0;
            let _ = out_tx
                .send(serde_json::to_vec(&resp).unwrap_or_default())
                .await;
            // Releasing the successor also hands it this request's identity. Dropping the sender
            // without sending still releases it, which is what happens on a panic.
            if let Some(done) = done {
                let _ = done.send((method, ran));
            }
        });
    }
    requests.abort_all();
    while requests.join_next().await.is_some() {}

    // Client gone: drop our writer handle so the writer task ends, stop the forwarder, and let the
    // writer flush what it has queued.
    drop(out_tx);
    forwarder.abort();
    let _ = writer.await;
}

/// What a request is ordered against. The chain used to be the whole connection, so a fleet mail
/// was a predecessor of every keystroke in every pane; the desktop holds one connection, which is
/// how one slow unrelated request froze all twelve panes at once.
///
/// The key is the narrowest thing a later request could observe the effect through.
#[derive(PartialEq, Eq, Hash, Clone, Debug)]
enum OrderKey {
    /// Steering or watching one pane. Two sends to the same window stay ordered; sends to
    /// different windows do not need to be, and never did.
    Window(String),
    /// This connection's own viewport, focus and subscription state. Claims must apply in the
    /// order the client made them, and `agent.fit` arbitrates on those claims, so it shares the
    /// key rather than being scoped to the window it resizes.
    Session,
    /// Everything else ordered: fleet and store mutations, and the reads that must see them.
    /// Kept as one chain rather than split per method, because a create and the call that acts on
    /// what it created are routinely different methods.
    Fleet,
}

/// Methods that steer or watch an existing pane. Window lifecycle (`agent.spawn`, `agent.adopt`,
/// `agent.stop`) is deliberately absent: fleet-wide readers observe those, so they stay on
/// `Fleet`.
const PANE_SCOPED: [&str; 10] = [
    "agent.send_input",
    "agent.answer",
    "agent.key",
    "agent.signal",
    "agent.scroll",
    "agent.resize",
    "agent.target",
    "agent.watch_bytes",
    "agent.prompt",
    "agent.transcript_watch",
];

/// The per-connection state claims, ordered against each other only.
///
/// `agent.fit` is here rather than on its window because it reads the focus and fit windows that
/// `viewport.set` writes: `fit_allowed` compares the caller's `focus_at` against every other
/// session's, so a fit that overtakes the claim granting it ownership is refused and the client
/// repaints at the old grid. Scoping the release to the windows a claim names would still miss a
/// claim that drops one. `agent.resize` needs none of this; it arbitrates nothing.
const SESSION_SCOPED: [&str; 4] = ["viewport.set", "subscribe", "watcher.park", "agent.fit"];

fn ordering_key(method: &str, params: &Option<serde_json::Value>) -> OrderKey {
    if SESSION_SCOPED.contains(&method) {
        return OrderKey::Session;
    }
    if !PANE_SCOPED.contains(&method) {
        return OrderKey::Fleet;
    }
    let window = params.as_ref().and_then(|p| {
        p.get("window")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                p.get("lane_id")
                    .and_then(serde_json::Value::as_u64)
                    .map(|lane| format!("lane-{lane}"))
            })
    });
    // A pane method that names no pane cannot be scoped to one, so it keeps the broad chain.
    window.map_or(OrderKey::Fleet, OrderKey::Window)
}

/// When a still-running ordered request is reported. The last is past the client's own 15 s
/// ceiling, so a request that blows through it is named even though its caller has already gone.
const CHAIN_HEAD_ALARMS: [u64; 3] = [1_000, 5_000, 15_000];

/// A request joins the connection's FIFO chain if and only if a later request on that same
/// connection could observe its effect. That is the whole rule, and it has two consequences.
///
/// Mutations stay ordered, including the ones whose semantics *are* the order: input sends and
/// dialog answers must arrive as they were typed, two viewport claims must apply in the order the
/// client made them, and `subscribe` must be live before the events it is meant to carry.
/// Correctness beats latency there.
///
/// A correctly sized capture is not one of those orderings and never was. `viewport.set` resizes
/// nothing; it records this session's claims and wakes the capture loop. The only RPC that
/// resizes is `agent.fit`, which stays ordered, and every `agent.capture` reply carries the
/// authoritative `size` the capture was taken at, so a client renders the grid it was given
/// rather than one it assumed. A capture that overtakes a resize returns an older frame
/// correctly labelled, which is not the stale-size repaint that `fix/terminal-repaint-race`
/// addressed.
///
/// Pure reads do not, and neither do reads that write only their own memoisation cache, because
/// nothing can observe that cache except the same read. Every method added below was either
/// measured holding the chain on the operator's machine or is polled per pane on every tick:
/// `daemon.status` at 16.8 s and `repo.pull_requests` at 7.7 s were the two worst, and behind them
/// `agent.prompt` waited 11.5 s while every pane stalled at once.
///
/// New methods still default to ordered. Adding one here means showing it cannot be observed out
/// of order.
fn independent_read(method: &str) -> bool {
    matches!(
        method,
        "ping"
            | "agent.detect"
            | "lane.list"
            | "agent.transcript_page"
            | "agent.command_catalog"
            | "agent.input_history"
            // Polled per pane on every tick, and self describing: the reply carries the size it
            // was taken at. `agent.prompt` deliberately stays ordered, see below.
            | "agent.capture"
            // Measured holding the chain on the operator's machine; all pure reads.
            | "daemon.status"
            | "repo.pull_requests"
            | "repomind.status"
            | "repo.list"
            | "config.get"
            | "usage.get"
            | "usage.summary"
            | "terminal.list_all"
            | "lane.headline"
            | "file.index"
    )
}

// `agent.prompt` is a pure read and was measured waiting 11.5 s, but it is not here. Answering a
// dialog clears it from the pane only when the agent repaints, so a poll can return a
// just-answered dialog and flash the composer back to blocked. Ordering does not close that
// window, which is why this is not a correctness argument: `agent.answer` returns once its keys
// are sent, not once the pane has repainted. What ordering does is stop the poll starting before
// the keys are sent at all, which is the only part of the window this side of the wire controls.
// With the two slow methods fixed the chain is short, so keeping it ordered costs little and
// strictly narrows a window the operator has already complained about.

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn ordering_keys_separate_panes_sessions_and_the_fleet() {
        let win = |w: &str| Some(serde_json::json!({ "lane_id": 7, "window": w }));
        let lane_only = Some(serde_json::json!({ "lane_id": 7 }));

        // Two sends to one window share a key, so they stay in the order they were typed.
        assert_eq!(
            ordering_key("agent.send_input", &win("lane-7")),
            ordering_key("agent.key", &win("lane-7"))
        );
        // Sends to different windows do not, which is the freeze: one slow pane held the rest.
        assert_ne!(
            ordering_key("agent.send_input", &win("lane-7")),
            ordering_key("agent.send_input", &win("lane-9"))
        );
        // A mail send and a lane read share nothing with any pane.
        assert_eq!(ordering_key("message.send", &None), OrderKey::Fleet);
        assert_ne!(
            ordering_key("message.send", &None),
            ordering_key("agent.send_input", &win("lane-7"))
        );
        // Viewport claims are ordered against each other, and against the fits that arbitrate on
        // them: `agent.fit` reads the focus and fit windows `viewport.set` writes, so a fit must
        // not overtake the claim that grants it ownership. Fits to different windows share the key
        // for the same reason, since the claim they read is per connection, not per window.
        assert_eq!(
            ordering_key("viewport.set", &win("lane-7")),
            OrderKey::Session
        );
        assert_eq!(
            ordering_key("viewport.set", &win("lane-7")),
            ordering_key("agent.fit", &win("lane-7"))
        );
        assert_eq!(
            ordering_key("agent.fit", &win("lane-7")),
            ordering_key("agent.fit", &win("lane-9"))
        );
        // `agent.resize` arbitrates on nothing, so it stays scoped to its own pane.
        assert_eq!(
            ordering_key("agent.resize", &win("lane-7")),
            OrderKey::Window("lane-7".into())
        );
        // Window lifecycle stays on the broad chain: fleet readers observe it.
        assert_eq!(ordering_key("agent.spawn", &win("lane-7")), OrderKey::Fleet);
        assert_eq!(ordering_key("agent.stop", &win("lane-7")), OrderKey::Fleet);
        // A pane method naming only a lane still resolves to that lane's window.
        assert_eq!(
            ordering_key("agent.send_input", &lane_only),
            OrderKey::Window("lane-7".into())
        );
        // A pane method naming no pane at all cannot be narrowed.
        assert_eq!(ordering_key("agent.send_input", &None), OrderKey::Fleet);
    }

    #[tokio::test]
    async fn a_slow_request_does_not_delay_an_ordered_request_with_a_different_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys.sock");
        let mut listener = transport::listen(&Endpoint::from_path(&path))
            .await
            .unwrap();
        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();
        let server = listener.accept().await.unwrap();
        let ctx = Ctx::new_with_paths(
            repomon_core::Store::open_in_memory().unwrap(),
            repomon_core::Config::default(),
            None,
            dir.path().join("config"),
            dir.path().join("notes"),
        );
        // `lane.get` cannot finish while this is held; it is on the Fleet chain.
        let blocked = ctx.overlay_flight.lock().await;
        let task = tokio::spawn(handle_conn(ctx.clone(), server));
        protocol::write_message(
            &mut client,
            &Request::new(1, "lane.get", Some(serde_json::json!({"lane_id": 1}))),
        )
        .await
        .unwrap();
        // A session-scoped request behind it, on its own key.
        protocol::write_message(
            &mut client,
            &Request::new(
                2,
                "agent.fit",
                Some(serde_json::json!({"lane_id": 7, "window": "lane-7"})),
            ),
        )
        .await
        .unwrap();
        let frame = tokio::time::timeout(Duration::from_secs(2), protocol::read_frame(&mut client))
            .await
            .expect("pane request queued behind an unrelated Fleet request")
            .unwrap()
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&frame).unwrap();
        assert_eq!(value["id"], 2, "the pane request must answer first");
        drop(blocked);
        task.abort();
    }

    #[tokio::test]
    async fn slow_overlay_does_not_delay_ping_on_the_same_connection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("concurrent.sock");
        let mut listener = transport::listen(&Endpoint::from_path(&path))
            .await
            .unwrap();
        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();
        let server = listener.accept().await.unwrap();
        let ctx = Ctx::new_with_paths(
            repomon_core::Store::open_in_memory().unwrap(),
            repomon_core::Config::default(),
            None,
            dir.path().join("config"),
            dir.path().join("notes"),
        );
        // lane.list cannot finish while this lock is held. The test releases it only after ping.
        let blocked = ctx.overlay_flight.lock().await;
        let task = tokio::spawn(handle_conn(ctx.clone(), server));
        protocol::write_message(&mut client, &Request::new(1, "lane.list", None))
            .await
            .unwrap();
        protocol::write_message(&mut client, &Request::new(2, "ping", None))
            .await
            .unwrap();
        let frame = tokio::time::timeout(Duration::from_secs(2), protocol::read_frame(&mut client))
            .await
            .expect("ping queued behind overlay")
            .unwrap()
            .unwrap();
        let response: Response = serde_json::from_slice(&frame).unwrap();
        assert_eq!(response.id, Some(2));
        assert_eq!(response.result, Some(serde_json::json!("pong")));
        drop(blocked);
        let frame = tokio::time::timeout(Duration::from_secs(5), protocol::read_frame(&mut client))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Response>(&frame).unwrap().id,
            Some(1)
        );
        drop(client);
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn two_input_sends_to_one_window_still_arrive_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inorder.sock");
        let mut listener = transport::listen(&Endpoint::from_path(&path))
            .await
            .unwrap();
        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();
        let server = listener.accept().await.unwrap();
        let ctx = Ctx::new_with_paths(
            repomon_core::Store::open_in_memory().unwrap(),
            repomon_core::Config::default(),
            None,
            dir.path().join("config"),
            dir.path().join("notes"),
        );
        let task = tokio::spawn(handle_conn(ctx.clone(), server));
        for (id, text) in [(1, "first"), (2, "second")] {
            protocol::write_message(
                &mut client,
                &Request::new(
                    id,
                    "agent.send_input",
                    Some(serde_json::json!({
                        "lane_id": 7, "window": "lane-7", "text": text, "enter": true
                    })),
                ),
            )
            .await
            .unwrap();
        }
        // Same window, same key: the replies must come back in the order they were sent, whatever
        // each one answers.
        for expected in 1..=2 {
            let frame =
                tokio::time::timeout(Duration::from_secs(5), protocol::read_frame(&mut client))
                    .await
                    .expect("input send answered")
                    .unwrap()
                    .unwrap();
            assert_eq!(
                serde_json::from_slice::<Response>(&frame).unwrap().id,
                Some(expected)
            );
        }
        task.abort();
    }

    #[tokio::test]
    async fn ordered_mutations_keep_wire_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ordered.sock");
        let mut listener = transport::listen(&Endpoint::from_path(&path))
            .await
            .unwrap();
        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();
        let server = listener.accept().await.unwrap();
        let ctx = Ctx::new_with_paths(
            repomon_core::Store::open_in_memory().unwrap(),
            repomon_core::Config::default(),
            None,
            dir.path().join("config"),
            dir.path().join("notes"),
        );
        let blocked = ctx.config.write().await;
        let task = tokio::spawn(handle_conn(ctx.clone(), server));
        // A pending configuration write must hold later requests on the same ordering key, while
        // ping bypasses it. Both mutations are `Fleet`, so this is the chain that still exists.
        for (id, name) in [(1, "qa-ordered-one"), (2, "qa-ordered-two")] {
            protocol::write_message(
                &mut client,
                &Request::new(
                    id,
                    "agent.add",
                    Some(serde_json::json!({"name": name, "command": "echo"})),
                ),
            )
            .await
            .unwrap();
        }
        protocol::write_message(&mut client, &Request::new(3, "ping", None))
            .await
            .unwrap();
        let frame = tokio::time::timeout(Duration::from_secs(2), protocol::read_frame(&mut client))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Response>(&frame).unwrap().id,
            Some(3)
        );
        drop(blocked);
        for id in 1..=2 {
            let frame =
                tokio::time::timeout(Duration::from_secs(2), protocol::read_frame(&mut client))
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
            let mut response: Response = serde_json::from_slice(&frame).unwrap();
            while response.id.is_none() {
                let frame = protocol::read_frame(&mut client).await.unwrap().unwrap();
                response = serde_json::from_slice(&frame).unwrap();
            }
            assert_eq!(response.id, Some(id));
            assert!(response.error.is_none());
        }
        for method in [
            "agent.send_input",
            "agent.key",
            "agent.spawn",
            "agent.stop",
            "viewport.set",
            "agent.transcript_watch",
        ] {
            assert!(!independent_read(method), "{method}");
        }
        drop(client);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn running_daemon_rebinds_and_broadcasts_without_dropping_existing_stream() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.sock");
        let mut listener = transport::listen(&Endpoint::from_path(&path))
            .await
            .unwrap();
        // Accepted streams belong to connections, not the listener, and must survive its replacement.
        let mut old_client = tokio::net::UnixStream::connect(&path).await.unwrap();
        let mut old_server = listener.accept().await.unwrap();
        let config = repomon_core::Config {
            tmux_session: format!("i1-watchdog-{}", std::process::id()),
            ..Default::default()
        };
        let ctx = Ctx::new_with_paths(
            repomon_core::Store::open_in_memory().unwrap(),
            config,
            None,
            dir.path().join("config"),
            dir.path().join("notes"),
        );
        let mut events = ctx.events.subscribe();
        // Unlink even before serving begins to prove we use the original bound identity.
        std::fs::remove_file(&path).unwrap();
        let task = tokio::spawn({
            let ctx = ctx.clone();
            let path = path.clone();
            async move { serve_with_watchdog(ctx, &path, listener, Duration::from_millis(10)).await }
        });
        let event = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event["method"], "event.daemon.rebound");
        assert_eq!(event["params"]["socket"], path.to_string_lossy().as_ref());
        let _new_client = tokio::net::UnixStream::connect(&path).await.unwrap();
        old_client.write_all(b"alive").await.unwrap();
        let mut data = [0; 5];
        old_server.read_exact(&mut data).await.unwrap();
        assert_eq!(&data, b"alive");
        ctx.shutdown.notify_waiters();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn watchdog_recovers_unlinked_listener_and_preserves_live_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.sock");
        let mut listener = transport::listen(&Endpoint::from_path(&path))
            .await
            .unwrap();
        let mut identity = socket_identity(&path).unwrap();
        let original = identity;
        std::fs::remove_file(&path).unwrap();
        assert!(
            rebind_if_lost(&path, &mut listener, &mut identity)
                .await
                .unwrap()
        );
        assert_ne!(identity, original);
        let _client = tokio::net::UnixStream::connect(&path).await.unwrap();
        listener.accept().await.unwrap();
        assert!(
            !rebind_if_lost(&path, &mut listener, &mut identity)
                .await
                .unwrap()
        );
        std::fs::remove_file(&path).unwrap();
        let other = tokio::net::UnixListener::bind(&path).unwrap();
        let other_identity = socket_identity(&path).unwrap();
        assert!(
            rebind_if_lost(&path, &mut listener, &mut identity)
                .await
                .is_err()
        );
        assert_eq!(socket_identity(&path).unwrap(), other_identity);
        drop(other);
    }
}
