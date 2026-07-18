//! Streaming one pane's raw PTY bytes — the embedded renderer's feed, shared across watchers.
//!
//! The mediated view is a poll → capture → re-parse pipeline; the embedded renderer instead
//! wants the pane's actual byte stream. The backend provides it via
//! [`SessionBackend::open_byte_stream`] (on tmux: `pipe-pane` into a daemon-owned fifo); each
//! chunk is broadcast as `event.agent.bytes` (base64 — PTY bytes are not valid UTF-8 at chunk
//! boundaries).
//!
//! ONE PIPE PER WINDOW, SHARED. The backend allows only one byte stream per window (tmux allows
//! one `pipe-pane` per pane), so a window can have exactly one stream no matter how many clients
//! watch it. The event bus already broadcasts every chunk to every subscriber; who actually
//! *receives* a window's bytes is decided per connection at the forwarding loops (a connection
//! forwards `event.agent.bytes` only for windows in its `watched_bytes` set). This module
//! therefore refcounts watchers per window: the first watcher starts the stream, later watchers
//! just join the readership, and the stream is torn down only when the last watcher leaves. A
//! phone, an iPad, and the Mac TUI can all watch the same (or different) windows at once — the
//! old single global slot let any new watch kill the previous one, which is exactly what broke
//! concurrency.
//!
//! GENERATION / EOF race: lifecycle is EOF-driven — closing the stream (or the window dying)
//! ends the backend's byte channel, whose closure the forwarder task sees, and it then removes
//! its own map entry. Each fresh stream (a first watcher creating a new entry) gets a
//! globally-unique `generation`, and the forwarder drops its entry ONLY if the entry's
//! generation still matches the one it was started with; without that, a rapid unwatch→rewatch
//! of the same window could have the dying forwarder delete the freshly-started entry, leaving
//! `watched_bytes` pointing at a live window whose entry is gone — it would accept refs and
//! stream nothing. (The matching fifo-path race is handled inside the tmux backend, which bakes
//! its own unique tag into every fifo name.)

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use repomon_core::SessionBackend;
use repomon_core::model::LaneId;
use repomon_core::protocol::Notification;
use tokio::sync::Mutex;

use crate::pubsub::{self, EventTx};

/// One window's single shared byte stream and the connections watching it.
pub struct WatchEntry {
    /// The lane this window belongs to (so `on:false`-without-window can match a session's watched
    /// windows to a lane by field, not by resolving a default window name).
    pub lane: LaneId,
    /// Connection ids sharing this window's single stream. The stream stops when this empties.
    pub refs: HashSet<u64>,
    /// Globally-unique tag for THIS stream instance; guards EOF cleanup against a stop→restart
    /// race (see the module doc).
    pub generation: u64,
}

/// The registry of live byte watches, keyed by window. `Arc<Mutex<…>>` so the forwarder task can
/// hold its own handle and remove its entry when the stream closes.
pub type Watches = Arc<Mutex<HashMap<String, WatchEntry>>>;

/// Hands out a fresh generation to every new stream instance. Global and monotonic: uniqueness
/// across all windows is all the EOF guard needs.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Release `conn_id` from an entry's readership; returns true once no watcher remains (the stream
/// must then be torn down). Pure — the refcount transition unit tests drive this directly.
///
/// An entry always holds at least one ref (it is created with one and removed the moment it
/// empties), so `is_empty()` can only become true right after removing the entry's last watcher.
/// Releasing a `conn_id` the entry never held is a harmless no-op that leaves it non-empty.
fn release_ref(entry: &mut WatchEntry, conn_id: u64) -> bool {
    entry.refs.remove(&conn_id);
    entry.refs.is_empty()
}

/// Whether the forwarder that started at `generation` still owns `window`'s entry — the
/// EOF-cleanup guard. False if the entry is gone or has been superseded by a newer stream. Pure.
fn eof_entry_is_current(map: &HashMap<String, WatchEntry>, window: &str, generation: u64) -> bool {
    map.get(window).is_some_and(|e| e.generation == generation)
}

/// Start (or join) a byte watch on `window` for `conn_id`.
///
/// - Entry exists → the stream is already flowing; `conn_id` just joins the readership.
/// - No entry → open the window's single backend byte stream with a fresh generation and
///   `refs = {conn_id}`, and spawn the forwarder that broadcasts its chunks.
pub async fn watch(
    backend: Arc<dyn SessionBackend>,
    events: EventTx,
    watches: &Watches,
    lane: LaneId,
    window: String,
    conn_id: u64,
) -> Result<(), String> {
    // Hold the lock across setup: a concurrent watcher of the SAME window must either join the
    // entry we create or wait and find it — never start a second stream (the backend allows only
    // one per window).
    let mut map = watches.lock().await;
    if let Some(entry) = map.get_mut(&window) {
        entry.refs.insert(conn_id);
        return Ok(());
    }

    let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
    let stream = {
        let backend = backend.clone();
        let window = window.clone();
        tokio::task::spawn_blocking(move || backend.open_byte_stream(&window))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| e.to_string()))?
    };

    map.insert(
        window.clone(),
        WatchEntry {
            lane,
            refs: HashSet::from([conn_id]),
            generation,
        },
    );

    // The forwarder: drain the backend's chunks onto the event bus. When the channel closes
    // (stream turned off or window died) it drops its own entry, but only while the entry is
    // still this stream's (generation match) — a later watch of the same window supersedes it.
    {
        let window = window.clone();
        let watches = watches.clone();
        tokio::spawn(async move {
            let mut rx = stream.rx;
            while let Some(chunk) = rx.recv().await {
                let data = base64::engine::general_purpose::STANDARD.encode(&chunk);
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
            let mut map = watches.lock().await;
            if eof_entry_is_current(&map, &window, generation) {
                map.remove(&window);
            }
        });
    }
    Ok(())
}

/// Release `conn_id` from `window`'s watch. When the last watcher leaves, close the window's
/// stream (which EOFs the forwarder) and remove the entry. Idempotent: releasing an unwatched
/// window, or a conn that wasn't a watcher, is a no-op.
pub async fn unwatch(
    backend: &Arc<dyn SessionBackend>,
    watches: &Watches,
    window: &str,
    conn_id: u64,
) {
    let mut map = watches.lock().await;
    let Some(entry) = map.get_mut(window) else {
        return;
    };
    if !release_ref(entry, conn_id) {
        return; // other connections still share this window's stream
    }
    map.remove(window)
        .expect("entry present under the same lock");
    drop(map);
    let backend = backend.clone();
    let win = window.to_string();
    let _ = tokio::task::spawn_blocking(move || backend.close_byte_stream(&win)).await;
}

/// Release `conn_id` from EVERY window it watches, closing the streams that thereby empty. Called
/// from `Ctx::close_session` so a connection's byte watches die with it, whatever it was watching.
pub async fn unwatch_all(backend: &Arc<dyn SessionBackend>, watches: &Watches, conn_id: u64) {
    let mut stopped: Vec<String> = Vec::new();
    {
        let mut map = watches.lock().await;
        map.retain(|window, entry| {
            if release_ref(entry, conn_id) {
                stopped.push(window.clone());
                false // this window's last watcher left: drop the entry
            } else {
                true
            }
        });
    }
    for window in stopped {
        let backend = backend.clone();
        let _ = tokio::task::spawn_blocking(move || backend.close_byte_stream(&window)).await;
    }
}

/// Startup sweep: close the byte stream on every window of our session. A daemon that died with
/// a watch active leaves the backend's pipe running with no reader — on tmux that makes the
/// server buffer the pane's output in memory without bound.
pub async fn sweep(backend: Arc<dyn SessionBackend>) {
    let _ = tokio::task::spawn_blocking(move || {
        for w in backend.list_windows().unwrap_or_default() {
            let _ = backend.close_byte_stream(&w);
        }
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(refs: &[u64], generation: u64) -> WatchEntry {
        WatchEntry {
            lane: 1,
            refs: refs.iter().copied().collect(),
            generation,
        }
    }

    #[test]
    fn release_ref_reports_empty_only_when_last_watcher_leaves() {
        let mut e = entry(&[1, 2], 0);
        // Removing one of two watchers leaves the stream live.
        assert!(!release_ref(&mut e, 1));
        assert_eq!(e.refs.iter().copied().collect::<Vec<_>>(), vec![2]);
        // Removing the last watcher empties it — the caller must stop the stream.
        assert!(release_ref(&mut e, 2));
        assert!(e.refs.is_empty());
    }

    #[test]
    fn release_ref_of_non_watcher_never_empties_a_live_entry() {
        let mut e = entry(&[7], 0);
        // Conn 99 never watched this window: no-op, still one watcher.
        assert!(!release_ref(&mut e, 99));
        assert!(e.refs.contains(&7));
    }

    #[test]
    fn joining_watcher_shares_the_single_stream() {
        // Modelling `watch`'s "entry exists" branch: a second conn just joins the readership.
        let mut e = entry(&[1], 5);
        e.refs.insert(2);
        assert_eq!(e.refs.len(), 2);
        // The generation is untouched — the same stream, not a new one.
        assert_eq!(e.generation, 5);
    }

    #[test]
    fn eof_guard_removes_only_the_current_generation() {
        let mut map = HashMap::new();
        map.insert("lane-1".to_string(), entry(&[1], 3));
        // A forwarder from the current stream (gen 3) owns the entry.
        assert!(eof_entry_is_current(&map, "lane-1", 3));
        // A stale forwarder (gen 2) from a superseded stream must NOT delete the live entry.
        assert!(!eof_entry_is_current(&map, "lane-1", 2));
        // A window with no entry at all.
        assert!(!eof_entry_is_current(&map, "lane-9", 3));
    }

    #[test]
    fn unwatch_all_stops_only_the_windows_that_empty() {
        // Conn 1 watches window A alone and shares window B with conn 2. Releasing conn 1 should
        // stop A (its last watcher) but keep B (conn 2 still watches it).
        let mut map = HashMap::new();
        map.insert("A".to_string(), entry(&[1], 0));
        map.insert("B".to_string(), entry(&[1, 2], 1));
        let mut stopped: Vec<String> = Vec::new();
        map.retain(|window, e| {
            if release_ref(e, 1) {
                stopped.push(window.clone());
                false
            } else {
                true
            }
        });
        assert_eq!(stopped, vec!["A".to_string()]);
        assert!(!map.contains_key("A"));
        assert!(map.contains_key("B"));
        assert_eq!(
            map["B"].refs.iter().copied().collect::<Vec<_>>(),
            vec![2],
            "conn 1 was released from the shared window too"
        );
    }
}
