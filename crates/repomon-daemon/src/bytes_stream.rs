//! Streaming one pane's raw PTY bytes — the embedded renderer's feed, shared across watchers.
//!
//! The mediated view is a poll → `capture-pane -e` → re-parse pipeline; the embedded renderer
//! instead wants the pane's actual byte stream. `tmux pipe-pane` provides it: tmux runs a
//! `cat > fifo` for the pane and every byte the pane emits flows through. The daemon owns the
//! fifo and its reader; each chunk is broadcast as `event.agent.bytes` (base64 — PTY bytes are
//! not valid UTF-8 at chunk boundaries).
//!
//! ONE PIPE PER WINDOW, SHARED. tmux allows only one `pipe-pane` per pane, so a window can have
//! exactly one fifo/reader no matter how many clients watch it. The event bus already broadcasts
//! every chunk to every subscriber; who actually *receives* a window's bytes is decided per
//! connection at the forwarding loops (a connection forwards `event.agent.bytes` only for windows
//! in its `watched_bytes` set). This module therefore refcounts watchers per window: the first
//! watcher starts the pipe, later watchers just join the readership, and the pipe is torn down
//! only when the last watcher leaves. A phone, an iPad, and the Mac TUI can all watch the same
//! (or different) windows at once — the old single global slot let any new watch kill the
//! previous one, which is exactly what broke concurrency.
//!
//! ORDERING: the reader must open the fifo before `pipe-pane` starts `cat` — the fifo open is the
//! rendezvous — so tmux never buffers behind a dead pipe.
//!
//! GENERATION / EOF race: lifecycle is EOF-driven — `pipe-pane` (off) or the window dying kills
//! the `cat`, whose write end closing EOFs the reader, which exits and removes the fifo. Each
//! fresh pipe (a first watcher creating a new entry) gets a globally-unique `generation`, and the
//! generation is embedded in the fifo FILENAME, so two pipe instances of the same window never
//! share a path. That makes the reader's EOF unlink inherently safe: it can only ever remove its
//! own fifo, never a successor's — without the unique name, a rapid unwatch→rewatch of the same
//! window could have the dying reader unlink the NEW pipe's fifo just before `cat > fifo` opens
//! it, turning the fifo into a plain file that streams nothing while the fresh reader leaks. The
//! reader additionally drops its own map entry ONLY if the entry's generation still matches the
//! one it was started with; without that, the same race would delete the freshly-started entry,
//! leaving `watched_bytes` pointing at a live window whose entry is gone — it would accept refs
//! and stream nothing.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use repomon_core::TmuxRuntime;
use repomon_core::model::LaneId;
use repomon_core::protocol::Notification;
use tokio::sync::Mutex;

use crate::pubsub::{self, EventTx};

/// One window's single shared pipe-pane and the connections watching it.
pub struct WatchEntry {
    /// The lane this window belongs to (so `on:false`-without-window can match a session's watched
    /// windows to a lane by field, not by resolving a default window name).
    pub lane: LaneId,
    /// The fifo tmux's `cat` writes into and the reader thread drains.
    pub fifo: PathBuf,
    /// Connection ids sharing this window's single pipe. The pipe stops when this empties.
    pub refs: HashSet<u64>,
    /// Globally-unique tag for THIS pipe instance; guards EOF cleanup against a stop→restart race
    /// (see the module doc).
    pub generation: u64,
}

/// The registry of live byte watches, keyed by window. `Arc<Mutex<…>>` so an EOF reader thread can
/// hold its own handle and remove its entry via `blocking_lock` when the pipe closes.
pub type Watches = Arc<Mutex<HashMap<String, WatchEntry>>>;

/// Largest chunk broadcast per event — keeps single events bounded while a busy pane floods.
const CHUNK: usize = 16 * 1024;

/// Hands out a fresh generation to every new pipe instance. Global and monotonic: uniqueness
/// across all windows is all the EOF guard needs.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Release `conn_id` from an entry's readership; returns true once no watcher remains (the pipe
/// must then be torn down). Pure — the refcount transition unit tests drive this directly.
///
/// An entry always holds at least one ref (it is created with one and removed the moment it
/// empties), so `is_empty()` can only become true right after removing the entry's last watcher.
/// Releasing a `conn_id` the entry never held is a harmless no-op that leaves it non-empty.
fn release_ref(entry: &mut WatchEntry, conn_id: u64) -> bool {
    entry.refs.remove(&conn_id);
    entry.refs.is_empty()
}

/// Whether the reader that started at `generation` still owns `window`'s entry — the EOF-cleanup
/// guard. False if the entry is gone or has been superseded by a newer pipe. Pure.
fn eof_entry_is_current(map: &HashMap<String, WatchEntry>, window: &str, generation: u64) -> bool {
    map.get(window).is_some_and(|e| e.generation == generation)
}

/// Start (or join) a byte watch on `window` for `conn_id`.
///
/// - Entry exists → the pipe is already flowing; `conn_id` just joins the readership.
/// - No entry → create the window's single pipe (remove any stale fifo, mkfifo, reader-first
///   rendezvous, then `pipe-pane`) with a fresh generation and `refs = {conn_id}`.
pub async fn watch(
    tmux: TmuxRuntime,
    events: EventTx,
    watches: &Watches,
    lane: LaneId,
    window: String,
    conn_id: u64,
) -> Result<(), String> {
    // Hold the lock across setup: a concurrent watcher of the SAME window must either join the
    // entry we create or wait and find it — never start a second pipe (tmux allows only one).
    let mut map = watches.lock().await;
    if let Some(entry) = map.get_mut(&window) {
        entry.refs.insert(conn_id);
        return Ok(());
    }

    let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
    // The generation is part of the fifo NAME: pipe instances of the same window never collide on
    // a path, so a superseded reader's EOF unlink can only ever remove its own fifo (see the
    // module doc). The pre-mkfifo unlink still guards the one same-path case left — a leftover
    // fifo from a previous daemon run (the counter restarts at 0).
    let fifo = std::env::temp_dir().join(format!(
        "repomon-bytes-{}-{window}-{generation}.fifo",
        tmux.session()
    ));
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

    map.insert(
        window.clone(),
        WatchEntry {
            lane,
            fifo: fifo.clone(),
            refs: HashSet::from([conn_id]),
            generation,
        },
    );

    // Reader first: its open() is the rendezvous with cat's write-side open. On EOF it removes its
    // fifo (safe unconditionally: the generation-unique name means it can only be its own, never a
    // successor's) and drops its own entry, the latter only while the entry is still this pipe's
    // (generation match) — a later watch of the same window supersedes it.
    {
        let window = window.clone();
        let fifo = fifo.clone();
        let watches = watches.clone();
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
            let mut map = watches.blocking_lock();
            if eof_entry_is_current(&map, &window, generation) {
                map.remove(&window);
            }
        });
    }

    let t = tmux.clone();
    let win = window.clone();
    let fifo_w = fifo.clone();
    let started = tokio::task::spawn_blocking(move || t.pipe_pane_named(&win, &fifo_w))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| e.to_string()));
    if let Err(e) = started {
        // The pipe never started: drop the entry so it can't accept refs and stream nothing, and
        // remove the fifo (the reader, still blocked on open(), is a pre-existing leak parity —
        // no `cat` ever opens the write side, same as the old single-slot `start`).
        map.remove(&window);
        let _ = std::fs::remove_file(&fifo);
        return Err(e);
    }
    Ok(())
}

/// Release `conn_id` from `window`'s watch. When the last watcher leaves, turn the pane's pipe off
/// (EOFing the reader) and remove the fifo and entry. Idempotent: releasing an unwatched window,
/// or a conn that wasn't a watcher, is a no-op.
pub async fn unwatch(tmux: &TmuxRuntime, watches: &Watches, window: &str, conn_id: u64) {
    let mut map = watches.lock().await;
    let Some(entry) = map.get_mut(window) else {
        return;
    };
    if !release_ref(entry, conn_id) {
        return; // other connections still share this window's pipe
    }
    let entry = map
        .remove(window)
        .expect("entry present under the same lock");
    drop(map);
    let t = tmux.clone();
    let win = window.to_string();
    let _ = tokio::task::spawn_blocking(move || t.pipe_pane_off_named(&win)).await;
    let _ = std::fs::remove_file(&entry.fifo);
}

/// Release `conn_id` from EVERY window it watches, stopping the pipes that thereby empty. Called
/// from `Ctx::close_session` so a connection's byte watches die with it, whatever it was watching.
pub async fn unwatch_all(tmux: &TmuxRuntime, watches: &Watches, conn_id: u64) {
    let mut stopped: Vec<(String, PathBuf)> = Vec::new();
    {
        let mut map = watches.lock().await;
        map.retain(|window, entry| {
            if release_ref(entry, conn_id) {
                stopped.push((window.clone(), entry.fifo.clone()));
                false // this window's last watcher left: drop the entry
            } else {
                true
            }
        });
    }
    for (window, fifo) in stopped {
        let t = tmux.clone();
        let win = window.clone();
        let _ = tokio::task::spawn_blocking(move || t.pipe_pane_off_named(&win)).await;
        let _ = std::fs::remove_file(&fifo);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(refs: &[u64], generation: u64) -> WatchEntry {
        WatchEntry {
            lane: 1,
            fifo: PathBuf::from("/tmp/none.fifo"),
            refs: refs.iter().copied().collect(),
            generation,
        }
    }

    #[test]
    fn release_ref_reports_empty_only_when_last_watcher_leaves() {
        let mut e = entry(&[1, 2], 0);
        // Removing one of two watchers leaves the pipe live.
        assert!(!release_ref(&mut e, 1));
        assert_eq!(e.refs.iter().copied().collect::<Vec<_>>(), vec![2]);
        // Removing the last watcher empties it — the caller must stop the pipe.
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
    fn joining_watcher_shares_the_single_pipe() {
        // Modelling `watch`'s "entry exists" branch: a second conn just joins the readership.
        let mut e = entry(&[1], 5);
        e.refs.insert(2);
        assert_eq!(e.refs.len(), 2);
        // The generation is untouched — the same pipe, not a new one.
        assert_eq!(e.generation, 5);
    }

    #[test]
    fn eof_guard_removes_only_the_current_generation() {
        let mut map = HashMap::new();
        map.insert("lane-1".to_string(), entry(&[1], 3));
        // A reader from the current pipe (gen 3) owns the entry.
        assert!(eof_entry_is_current(&map, "lane-1", 3));
        // A stale reader (gen 2) from a superseded pipe must NOT delete the live entry.
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
