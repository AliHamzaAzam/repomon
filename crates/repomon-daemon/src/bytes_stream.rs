//! Streaming one pane's raw PTY bytes to the TUI — the embedded renderer's feed.
//!
//! The mediated view is a poll → `capture-pane -e` → re-parse pipeline; the embedded renderer
//! instead wants the pane's actual byte stream. `tmux pipe-pane` provides it: tmux runs a
//! `cat > fifo` for the pane and every byte the pane emits flows through. The daemon owns the
//! fifo and its reader; each chunk is broadcast as `event.agent.bytes` (base64 — PTY bytes are
//! not valid UTF-8 at chunk boundaries).
//!
//! One watch at a time (the Focus view streams exactly one pane): starting a new watch stops
//! the previous one. Lifecycle is EOF-driven — `pipe-pane` (off) or the window dying kills the
//! `cat`, whose write end closing EOFs the reader, which exits and removes the fifo. A stale
//! `BytesWatch` slot after that is harmless: [`stop`] is idempotent (pipe-pane off on a
//! pipeless window is a no-op, removing a missing fifo is a no-op). ORDERING: the reader must
//! open the fifo before `pipe-pane` starts `cat` — the fifo open is the rendezvous — so tmux
//! never buffers behind a dead pipe.

use std::path::PathBuf;
use std::process::Command;

use base64::Engine;
use repomon_core::TmuxRuntime;
use repomon_core::model::LaneId;
use repomon_core::protocol::Notification;
use tokio::sync::Mutex;

use crate::pubsub::{self, EventTx};

/// The active byte watch: which window streams and where its fifo lives.
pub struct BytesWatch {
    pub window: String,
    pub fifo: PathBuf,
}

/// Largest chunk broadcast per event — keeps single events bounded while a busy pane floods.
const CHUNK: usize = 16 * 1024;

/// Stop the current byte watch, if any: turn the pane's pipe off (killing its `cat`, which
/// EOFs the reader) and remove the fifo.
pub async fn stop(tmux: &TmuxRuntime, slot: &Mutex<Option<BytesWatch>>) {
    let Some(watch) = slot.lock().await.take() else {
        return;
    };
    let t = tmux.clone();
    let window = watch.window.clone();
    let _ = tokio::task::spawn_blocking(move || t.pipe_pane_off_named(&window)).await;
    let _ = std::fs::remove_file(&watch.fifo);
}

/// Start streaming `window`'s bytes. The caller stops any previous watch first (the RPC
/// handler always calls [`stop`] before this).
pub async fn start(
    tmux: TmuxRuntime,
    events: EventTx,
    slot: &Mutex<Option<BytesWatch>>,
    lane: LaneId,
    window: String,
) -> Result<(), String> {
    let fifo = std::env::temp_dir().join(format!("repomon-bytes-{}-{window}.fifo", tmux.session()));
    let _ = std::fs::remove_file(&fifo);
    let ok = Command::new("mkfifo")
        .arg(&fifo)
        .output()
        .map_err(|e| format!("mkfifo: {e}"))?
        .status
        .success();
    if !ok {
        return Err(format!("mkfifo {} failed", fifo.display()));
    }

    *slot.lock().await = Some(BytesWatch {
        window: window.clone(),
        fifo: fifo.clone(),
    });

    // Reader first: its open() is the rendezvous with cat's write-side open.
    {
        let window = window.clone();
        let fifo = fifo.clone();
        tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let Ok(mut f) = std::fs::File::open(&fifo) else {
                return;
            };
            let mut buf = vec![0u8; CHUNK];
            loop {
                match f.read(&mut buf) {
                    Ok(0) | Err(_) => break, // EOF: pipe turned off or window died
                    Ok(n) => {
                        let data = base64::engine::general_purpose::STANDARD.encode(&buf[..n]);
                        let note = Notification::new(
                            pubsub::topic::AGENT_BYTES,
                            serde_json::json!({
                                "lane_id": lane,
                                "window": window,
                                "data": data,
                            }),
                        );
                        if let Ok(value) = serde_json::to_value(&note) {
                            let _ = events.send(value); // Err = no subscribers; fine
                        }
                    }
                }
            }
            let _ = std::fs::remove_file(&fifo);
        });
    }

    let t = tmux.clone();
    let win = window.clone();
    let fifo_w = fifo.clone();
    tokio::task::spawn_blocking(move || t.pipe_pane_named(&win, &fifo_w))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Startup sweep: turn `pipe-pane` off on every window of our session. A daemon that died with
/// a watch active leaves a `cat` blocked on a fifo nobody reads — tmux would buffer that
/// pane's output in memory without bound.
pub async fn sweep(tmux: TmuxRuntime) {
    let _ = tokio::task::spawn_blocking(move || {
        for w in tmux.list_windows().unwrap_or_default() {
            let _ = tmux.pipe_pane_off_named(&w);
        }
    })
    .await;
}
