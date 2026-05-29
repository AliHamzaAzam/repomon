//! Async client for the daemon's framed JSON-RPC socket.
//!
//! A reader task demultiplexes incoming frames: responses (with an `id`) resolve the
//! matching pending call; notifications (`event.*`) fan out on a broadcast channel.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use repomon_core::protocol::{read_frame, write_frame, Notification, Request, Response};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::net::UnixStream;
use tokio::sync::{broadcast, mpsc, oneshot};

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>>;

/// A connected daemon client. Cheap to clone.
#[derive(Clone)]
pub struct DaemonClient {
    out_tx: mpsc::Sender<Vec<u8>>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
    events_tx: broadcast::Sender<Notification>,
}

impl DaemonClient {
    /// Connect to the daemon socket and start the reader/writer tasks.
    pub async fn connect(path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(path)
            .await
            .with_context(|| format!("connecting to daemon at {}", path.display()))?;
        let (mut rd, mut wr) = stream.into_split();

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(128);
        let (events_tx, _rx) = broadcast::channel(256);

        tokio::spawn(async move {
            while let Some(frame) = out_rx.recv().await {
                if write_frame(&mut wr, &frame).await.is_err() {
                    break;
                }
            }
        });

        let pending_r = pending.clone();
        let events_r = events_tx.clone();
        tokio::spawn(async move {
            while let Ok(Some(frame)) = read_frame(&mut rd).await {
                // A response carries an id; a notification does not.
                if let Ok(resp) = serde_json::from_slice::<Response>(&frame) {
                    if let Some(id) = resp.id {
                        if let Some(tx) = pending_r.lock().unwrap().remove(&id) {
                            let _ = tx.send(resp);
                        }
                        continue;
                    }
                }
                if let Ok(note) = serde_json::from_slice::<Notification>(&frame) {
                    if note.method.starts_with("event.") {
                        let _ = events_r.send(note);
                    }
                }
            }
        });

        Ok(DaemonClient {
            out_tx,
            pending,
            next_id: Arc::new(AtomicU64::new(1)),
            events_tx,
        })
    }

    /// Call a method and return the raw result value.
    pub async fn call(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);

        let req = Request::new(id, method, params);
        let bytes = serde_json::to_vec(&req)?;
        self.out_tx
            .send(bytes)
            .await
            .map_err(|_| anyhow!("daemon connection closed"))?;

        let resp = tokio::time::timeout(Duration::from_secs(15), rx)
            .await
            .map_err(|_| anyhow!("request '{method}' timed out"))?
            .map_err(|_| anyhow!("request '{method}' was dropped"))?;
        if let Some(err) = resp.error {
            return Err(anyhow!("{} (code {})", err.message, err.code));
        }
        Ok(resp.result.unwrap_or(Value::Null))
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

    /// Subscribe to the daemon's event stream (after sending a `subscribe` request).
    pub fn subscribe(&self) -> broadcast::Receiver<Notification> {
        self.events_tx.subscribe()
    }
}
