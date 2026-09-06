//! Shares one backend byte stream per window across subscribers. Generation checks prevent a stale
//! stream’s EOF from removing its replacement.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use repomon_core::model::LaneId;
use repomon_core::protocol::Notification;
use repomon_core::{ByteStreamEvent, SessionBackend};
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
    /// Last raw PTY chunk accepted from this generation.
    pub sequence: Arc<AtomicU64>,
    /// Last pane grid observed by either a mediated resize or the byte-watch grid probe.
    pub grid: Option<(u16, u16)>,
}

/// The registry of live byte watches, keyed by window. `Arc<Mutex<…>>` so the forwarder task can
/// hold its own handle and remove its entry when the stream closes.
pub type Watches = Arc<Mutex<HashMap<String, WatchEntry>>>;

/// One byte stream's stable identity and current position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamCursor {
    pub generation: u64,
    pub sequence: u64,
}

impl WatchEntry {
    fn cursor(&self) -> StreamCursor {
        StreamCursor {
            generation: self.generation,
            sequence: self.sequence.load(Ordering::Acquire),
        }
    }
}

/// Hands out a fresh generation to every new stream instance. Global and monotonic: uniqueness
/// across all windows is all the EOF guard needs.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Remove one watcher and report whether the last reference was released; unknown connection IDs
/// leave an active entry unchanged.
fn release_ref(entry: &mut WatchEntry, conn_id: u64) -> bool {
    entry.refs.remove(&conn_id);
    entry.refs.is_empty()
}

/// Whether the forwarder that started at `generation` still owns `window`'s entry - the
/// EOF-cleanup guard. False if the entry is gone or has been superseded by a newer stream. Pure.
fn eof_entry_is_current(map: &HashMap<String, WatchEntry>, window: &str, generation: u64) -> bool {
    map.get(window).is_some_and(|e| e.generation == generation)
}

/// Records a grid for the optional watch generation, returning None for an absent watch and
/// Some(false) for an unchanged grid.
pub async fn note_grid(
    watches: &Watches,
    window: &str,
    generation: Option<u64>,
    grid: (u16, u16),
) -> Option<bool> {
    let mut map = watches.lock().await;
    let entry = map.get_mut(window)?;
    if generation.is_some_and(|generation| generation != entry.generation) {
        return None;
    }
    if entry.grid == Some(grid) {
        return Some(false);
    }
    entry.grid = Some(grid);
    Some(true)
}

/// Starts or joins the window’s shared byte stream for the connection.
pub async fn watch(
    backend: Arc<dyn SessionBackend>,
    events: EventTx,
    watches: &Watches,
    lane: LaneId,
    window: String,
    conn_id: u64,
) -> Result<StreamCursor, String> {
    // Hold the lock across setup: a concurrent watcher of the SAME window must either join the
    // entry we create or wait and find it - never start a second stream (the backend allows only
    // one per window).
    let mut map = watches.lock().await;
    if let Some(entry) = map.get_mut(&window) {
        entry.refs.insert(conn_id);
        return Ok(entry.cursor());
    }

    let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
    let sequence = Arc::new(AtomicU64::new(0));
    let (stream, initial_grid) = {
        let backend = backend.clone();
        let window = window.clone();
        tokio::task::spawn_blocking(move || {
            let grid = backend.size_named(&window);
            backend
                .open_byte_stream(&window)
                .map(|stream| (stream, grid))
        })
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
            sequence: sequence.clone(),
            grid: initial_grid,
        },
    );

    // Sequence grid changes with bytes so clients resize before decoding new output; generation
    // checks prevent stale EOF from removing a replacement stream.
    {
        let forward_window = window.clone();
        let forward_watches = watches.clone();
        let forward_events = events.clone();
        let stream_sequence = sequence.clone();
        tokio::spawn(async move {
            let mut rx = stream.rx;
            while let Some(event) = rx.recv().await {
                let note = match event {
                    ByteStreamEvent::Bytes(chunk) => {
                        let sequence = stream_sequence.fetch_add(1, Ordering::AcqRel) + 1;
                        let data = base64::engine::general_purpose::STANDARD.encode(&chunk);
                        Notification::new(
                            pubsub::topic::AGENT_BYTES,
                            serde_json::json!({
                                "lane_id": lane,
                                "window": forward_window,
                                "generation": generation,
                                "sequence": sequence,
                                "data": data,
                            }),
                        )
                    }
                    ByteStreamEvent::Grid { cols, rows } => {
                        if note_grid(
                            &forward_watches,
                            &forward_window,
                            Some(generation),
                            (cols, rows),
                        )
                        .await
                            != Some(true)
                        {
                            continue;
                        }
                        let sequence = stream_sequence.fetch_add(1, Ordering::AcqRel) + 1;
                        Notification::new(
                            pubsub::topic::AGENT_GRID,
                            serde_json::json!({
                                "lane_id": lane,
                                "window": forward_window,
                                "generation": generation,
                                "sequence": sequence,
                                "cols": cols,
                                "rows": rows,
                            }),
                        )
                    }
                };
                if let Ok(value) = serde_json::to_value(&note) {
                    let _ = forward_events.send(value); // Err = no subscribers; fine
                }
            }
            let closed_current = {
                let mut map = forward_watches.lock().await;
                if eof_entry_is_current(&map, &forward_window, generation) {
                    map.remove(&forward_window);
                    true
                } else {
                    false
                }
            };
            // Explicit unwatch removes the entry before closing the backend, so only an
            // unexpected backend EOF (normally target-window death) reaches clients. The
            // generation prevents a delayed close from stopping a replacement watch.
            if closed_current {
                let note = Notification::new(
                    pubsub::topic::AGENT_STREAM_CLOSED,
                    serde_json::json!({
                        "lane_id": lane,
                        "window": forward_window,
                        "generation": generation,
                    }),
                );
                if let Ok(value) = serde_json::to_value(&note) {
                    let _ = forward_events.send(value);
                }
            }
        });
    }
    Ok(StreamCursor {
        generation,
        sequence: 0,
    })
}

/// The current stream position for `window`, if a byte watch is active.
pub async fn cursor(watches: &Watches, window: &str) -> Option<StreamCursor> {
    watches.lock().await.get(window).map(WatchEntry::cursor)
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
                false
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
/// a watch active leaves the backend's pipe running with no reader - on tmux that makes the
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
            sequence: Arc::new(AtomicU64::new(0)),
            grid: Some((80, 24)),
        }
    }

    #[test]
    fn release_ref_reports_empty_only_when_last_watcher_leaves() {
        let mut e = entry(&[1, 2], 0);

        assert!(!release_ref(&mut e, 1));
        assert_eq!(e.refs.iter().copied().collect::<Vec<_>>(), vec![2]);
        // Removing the last watcher empties it - the caller must stop the stream.
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
        // The generation is untouched - the same stream, not a new one.
        assert_eq!(e.generation, 5);
    }

    #[test]
    fn cursor_tracks_sequence_within_one_generation() {
        let e = entry(&[1], 9);
        assert_eq!(
            e.cursor(),
            StreamCursor {
                generation: 9,
                sequence: 0,
            }
        );
        e.sequence.store(4, Ordering::Release);
        assert_eq!(
            e.cursor(),
            StreamCursor {
                generation: 9,
                sequence: 4,
            }
        );
    }

    #[test]
    fn eof_guard_removes_only_the_current_generation() {
        let mut map = HashMap::new();
        map.insert("lane-1".to_string(), entry(&[1], 3));

        assert!(eof_entry_is_current(&map, "lane-1", 3));
        // A stale forwarder (gen 2) from a superseded stream must NOT delete the live entry.
        assert!(!eof_entry_is_current(&map, "lane-1", 2));

        assert!(!eof_entry_is_current(&map, "lane-9", 3));
    }

    #[tokio::test]
    async fn grid_tracking_deduplicates_and_rejects_stale_streams() {
        let watches = Arc::new(Mutex::new(HashMap::from([(
            "lane-1".to_string(),
            entry(&[1], 3),
        )])));
        assert_eq!(
            note_grid(&watches, "lane-1", Some(3), (80, 24)).await,
            Some(false)
        );
        assert_eq!(
            note_grid(&watches, "lane-1", Some(2), (100, 30)).await,
            None
        );
        assert_eq!(
            note_grid(&watches, "lane-1", Some(3), (100, 30)).await,
            Some(true)
        );
        assert_eq!(
            note_grid(&watches, "lane-1", None, (100, 30)).await,
            Some(false)
        );
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
