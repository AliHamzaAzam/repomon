//! Async client for the daemon's framed JSON-RPC socket.
//!
//! A reader task demultiplexes incoming frames: responses (with an `id`) resolve the
//! matching pending call; notifications (`event.*`) fan out on a broadcast channel.
//!
//! This is the single shared client used by every out-of-process consumer of the daemon —
//! the TUI, the headless CLI, and the MCP server (`repomond mcp`) that backs the repomind
//! orchestrator. Keeping one implementation means the wire framing, timeout, and event
//! demuxing behave identically everywhere.
//!
//! ## Self-healing connection
//!
//! The daemon reaps idle client connections after 120s (`repomon-daemon` socket.rs). A naive
//! one-shot connection therefore goes permanently dead the first time it's left idle, and every
//! subsequent RPC fails with "daemon connection closed" — which is exactly how the MCP bridge's
//! action tools (`read_agent`, `send_to_agent`, …) silently bricked while subscription-fed reads
//! kept working. To prevent that, this client:
//!   * marks itself disconnected (and fails in-flight calls fast) when the reader sees the socket
//!     close, then transparently **reconnects + retries once** on the next `call`, and
//!   * sends a lightweight keepalive `ping` well under the 120s reaper so a healthy idle client is
//!     never dropped in the first place.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::net::UnixStream;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::protocol::{Notification, Request, Response, read_frame, write_frame};

/// Keepalive cadence. Must stay comfortably under the daemon's 120s idle-connection reaper so an
/// otherwise-silent client (e.g. the MCP bridge, which mostly just receives events) is never reaped.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(60);

/// Per-RPC ceiling. A timeout means the daemon stopped responding mid-call.
const CALL_TIMEOUT: Duration = Duration::from_secs(15);

type Pending = Mutex<HashMap<u64, oneshot::Sender<Response>>>;

/// A connected daemon client. Cheap to clone; all clones share one connection that reconnects
/// transparently underneath them.
#[derive(Clone)]
pub struct DaemonClient {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    pending: Pending,
    next_id: AtomicU64,
    events_tx: broadcast::Sender<Notification>,
    /// Outbound frame channel for the *current* connection; swapped on every (re)connect.
    out_tx: Mutex<mpsc::Sender<Vec<u8>>>,
    /// Cleared by the reader task the moment the socket closes, so `call` knows to reconnect.
    connected: AtomicBool,
    /// Serializes reconnects so a burst of concurrent calls opens exactly one new socket.
    reconnecting: tokio::sync::Mutex<()>,
    /// Params of the last successful `subscribe` call, if any. `reconnect` replays it on the new
    /// connection — the daemon only forwards events to connections that asked for them.
    subscribe_params: Mutex<Option<Option<Value>>>,
    /// Bumped every time `spawn_io` starts a new connection. A reader task captures its own
    /// epoch at spawn and only runs disconnect cleanup if it's still current, so a stale reader
    /// from a superseded connection can't clobber a healthy reconnect.
    epoch: AtomicU64,
}

impl DaemonClient {
    /// Connect to the daemon socket and start the reader/writer + keepalive tasks.
    pub async fn connect(path: &Path) -> Result<Self> {
        Self::connect_inner(path, Some(KEEPALIVE_INTERVAL)).await
    }

    async fn connect_inner(path: &Path, keepalive: Option<Duration>) -> Result<Self> {
        let stream = UnixStream::connect(path)
            .await
            .with_context(|| format!("connecting to daemon at {}", path.display()))?;

        let (events_tx, _rx) = broadcast::channel(256);
        // Placeholder sender, immediately replaced by `spawn_io`.
        let (placeholder, _) = mpsc::channel(1);
        let inner = Arc::new(Inner {
            path: path.to_path_buf(),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            events_tx,
            out_tx: Mutex::new(placeholder),
            connected: AtomicBool::new(false),
            reconnecting: tokio::sync::Mutex::new(()),
            subscribe_params: Mutex::new(None),
            epoch: AtomicU64::new(0),
        });
        inner.spawn_io(stream);

        if let Some(interval) = keepalive {
            tokio::spawn(keepalive_loop(Arc::downgrade(&inner), interval));
        }

        Ok(DaemonClient { inner })
    }

    /// Call a method and return the raw result value. Reconnects once if the connection was reaped.
    pub async fn call(&self, method: &str, params: Option<Value>) -> Result<Value> {
        // Two attempts: the second runs only after a reconnect, so a reaped connection heals
        // instead of failing the caller.
        for _attempt in 0..2 {
            if !self.inner.connected.load(Ordering::SeqCst) {
                self.inner.reconnect().await?;
            }

            let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = oneshot::channel();
            self.inner.pending.lock().unwrap().insert(id, tx);

            let req = Request::new(id, method, params.clone());
            let bytes = serde_json::to_vec(&req)?;

            // Clone the sender out of the lock so we never hold a sync mutex across `.await`.
            let out = self.inner.out_tx.lock().unwrap().clone();
            if out.send(bytes).await.is_err() {
                // Writer task is gone -> connection is dead.
                self.inner.pending.lock().unwrap().remove(&id);
                self.inner.connected.store(false, Ordering::SeqCst);
                continue; // attempt == 1 falls through to the error below
            }

            match tokio::time::timeout(CALL_TIMEOUT, rx).await {
                Ok(Ok(resp)) => {
                    if let Some(err) = resp.error {
                        return Err(anyhow!("{} (code {})", err.message, err.code));
                    }
                    if method == "subscribe" {
                        *self.inner.subscribe_params.lock().unwrap() = Some(params.clone());
                    }
                    return Ok(resp.result.unwrap_or(Value::Null));
                }
                // Sender dropped: the reader cleared pending because the socket closed.
                Ok(Err(_)) => {
                    self.inner.connected.store(false, Ordering::SeqCst);
                    continue;
                }
                Err(_) => {
                    self.inner.pending.lock().unwrap().remove(&id);
                    tracing::warn!("RPC {method} timed out after {CALL_TIMEOUT:?}");
                    return Err(anyhow!("request '{method}' timed out"));
                }
            }
        }
        Err(anyhow!("daemon connection closed"))
    }

    /// Call a method and deserialize the result.
    pub async fn call_typed<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<T> {
        let value = self.call(method, params).await?;
        serde_json::from_value(value).map_err(|e| anyhow!("decoding {method} result: {e}"))
    }

    /// Subscribe to the daemon's event stream (after sending a `subscribe` request). The
    /// subscription survives reconnects: a new reader task feeds the same broadcast channel.
    pub fn subscribe(&self) -> broadcast::Receiver<Notification> {
        self.inner.events_tx.subscribe()
    }
}

impl Inner {
    /// Spawn the reader + writer tasks for `stream`, install its outbound channel, and mark the
    /// connection live. Reused for the initial connect and every reconnect.
    fn spawn_io(self: &Arc<Self>, stream: UnixStream) {
        let (mut rd, mut wr) = stream.into_split();
        let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(128);
        *self.out_tx.lock().unwrap() = out_tx;
        self.connected.store(true, Ordering::SeqCst);
        // Each (re)connect bumps the epoch; the reader task below captures it so it can tell
        // whether it's still the current connection by the time its loop exits.
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;

        // Writer: drains queued frames to the socket.
        tokio::spawn(async move {
            while let Some(frame) = out_rx.recv().await {
                if write_frame(&mut wr, &frame).await.is_err() {
                    break;
                }
            }
        });

        // Reader: demuxes responses and events; on close, marks disconnected and fails in-flight
        // calls so they retry/reconnect instead of hanging to the timeout.
        let inner = self.clone();
        tokio::spawn(async move {
            while let Ok(Some(frame)) = read_frame(&mut rd).await {
                if let Ok(resp) = serde_json::from_slice::<Response>(&frame) {
                    if let Some(id) = resp.id {
                        if let Some(tx) = inner.pending.lock().unwrap().remove(&id) {
                            let _ = tx.send(resp);
                        }
                        continue;
                    }
                }
                if let Ok(note) = serde_json::from_slice::<Notification>(&frame) {
                    if note.method.starts_with("event.") {
                        let _ = inner.events_tx.send(note);
                    }
                }
            }
            // Only run disconnect cleanup if no reconnect has since installed a newer connection;
            // otherwise a stale reader from a superseded socket would mark a healthy one dead.
            if inner.epoch.load(Ordering::SeqCst) == epoch {
                inner.connected.store(false, Ordering::SeqCst);
                inner.pending.lock().unwrap().clear();
            }
        });
    }

    /// Re-establish the connection. Serialized so concurrent callers open one socket; a no-op if
    /// another caller already reconnected.
    async fn reconnect(self: &Arc<Self>) -> Result<()> {
        let _guard = self.reconnecting.lock().await;
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }
        let stream = UnixStream::connect(&self.path)
            .await
            .with_context(|| format!("reconnecting to daemon at {}", self.path.display()))?;
        self.spawn_io(stream);

        // Re-establish the event subscription, if any: the daemon only forwards events on
        // connections that sent `subscribe`, and the new socket never has. Push the replayed
        // request straight onto the new connection's outbound channel rather than going through
        // `self.call()` — the `reconnecting` lock above is still held, and `call()` can recurse
        // back into `reconnect()` on failure, which would deadlock. The response comes back with
        // an id nobody is waiting on; the reader already drops unmatched ids, so that's harmless.
        let recorded = self.subscribe_params.lock().unwrap().clone();
        if let Some(params) = recorded {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let req = Request::new(id, "subscribe", params);
            if let Ok(bytes) = serde_json::to_vec(&req) {
                let out = self.out_tx.lock().unwrap().clone();
                let _ = out.send(bytes).await;
            }
        }
        Ok(())
    }
}

/// Keepalive: ping under the daemon's idle reaper so a quiet client is never dropped. Exits when
/// the last `DaemonClient` is dropped (the `Weak` stops upgrading).
async fn keepalive_loop(weak: Weak<Inner>, interval: Duration) {
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await; // consume the immediate first tick
    loop {
        tick.tick().await;
        let Some(inner) = weak.upgrade() else { return };
        let client = DaemonClient { inner };
        // Result ignored: a failure just means the next real call (or tick) will reconnect.
        let _ = client.call("ping", None).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::write_message;
    use std::sync::atomic::AtomicUsize;
    use tokio::net::UnixListener;

    /// A keepalive client must ping an idle connection on its own, so the daemon never reaps it.
    #[tokio::test]
    async fn keepalive_pings_idle_connection() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("d.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        let pings = Arc::new(AtomicUsize::new(0));
        let pings_srv = pings.clone();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut rd, mut wr) = stream.into_split();
            while let Ok(Some(frame)) = read_frame(&mut rd).await {
                if let Ok(req) = serde_json::from_slice::<Request>(&frame) {
                    if req.method == "ping" {
                        pings_srv.fetch_add(1, Ordering::SeqCst);
                    }
                    let _ = write_message(&mut wr, &Response::ok(req.id, Value::Null)).await;
                }
            }
        });

        // 80ms keepalive; stay idle for ~300ms -> at least a couple of pings with no `call`.
        let _client = DaemonClient::connect_inner(&sock, Some(Duration::from_millis(80)))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(320)).await;
        assert!(
            pings.load(Ordering::SeqCst) >= 2,
            "expected the keepalive to ping an idle connection, got {}",
            pings.load(Ordering::SeqCst)
        );
    }

    /// A reader task belonging to a since-superseded connection must not run disconnect cleanup
    /// on top of a healthy newer one. Drives `Inner::spawn_io` directly (bypassing `reconnect`'s
    /// serialization) to simulate the old reader noticing EOF only after the new connection is
    /// already live.
    #[tokio::test]
    async fn stale_reader_does_not_clobber_new_connection() {
        let (a_client, a_server) = UnixStream::pair().unwrap();
        let (b_client, _b_server) = UnixStream::pair().unwrap();

        let (events_tx, _rx) = broadcast::channel(8);
        let (placeholder, _) = mpsc::channel(1);
        let inner = Arc::new(Inner {
            path: PathBuf::from("/dev/null"),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            events_tx,
            out_tx: Mutex::new(placeholder),
            connected: AtomicBool::new(false),
            reconnecting: tokio::sync::Mutex::new(()),
            subscribe_params: Mutex::new(None),
            epoch: AtomicU64::new(0),
        });

        // "Connection A" goes live, then a reconnect installs "connection B" while A's reader
        // hasn't yet noticed its socket closed.
        inner.spawn_io(a_client);
        inner.spawn_io(b_client);
        assert!(inner.connected.load(Ordering::SeqCst));

        // Seed a pending call as if it's in flight on the healthy new connection.
        let (tx, _rx) = oneshot::channel();
        inner.pending.lock().unwrap().insert(42, tx);

        // Close A's peer -> A's reader observes EOF and runs its (now-stale) cleanup.
        drop(a_server);
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(
            inner.connected.load(Ordering::SeqCst),
            "a stale reader from a superseded connection must not mark the client disconnected"
        );
        assert!(
            inner.pending.lock().unwrap().contains_key(&42),
            "a stale reader must not clear in-flight calls belonging to the new connection"
        );
    }
}
