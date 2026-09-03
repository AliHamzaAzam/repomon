//! Worktree filesystem watcher for active lanes.
//!
//! Debounces worktree changes at 250ms using `notify-debouncer-full`.
//! Filters out `.git/` internals and gitignored files/directories,
//! invalidates the lane's file index cache, and broadcasts `event.file.changed`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{Config, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, NoCache, new_debouncer_opt};
use repomon_core::model::LaneId;
use repomon_core::protocol::Notification;
use serde_json::json;
use tokio::sync::{Mutex, mpsc};
use tokio::task::AbortHandle;

use crate::CachedIndex;
use crate::files::{check_ignored, index_worktree};
use crate::pubsub::EventTx;

/// Handle to a live per-lane worktree watcher.
pub struct WorktreeWatcher {
    _debouncer: Debouncer<RecommendedWatcher, NoCache>,
    worker_abort: AbortHandle,
}

impl Drop for WorktreeWatcher {
    fn drop(&mut self) {
        self.worker_abort.abort();
    }
}

/// Start a 250ms debounced filesystem watcher over `root` for `lane_id`.
pub fn start_lane_watcher(
    events: EventTx,
    file_indices: Arc<Mutex<HashMap<LaneId, CachedIndex>>>,
    lane_id: LaneId,
    root: PathBuf,
) -> std::io::Result<WorktreeWatcher> {
    let original_root = root.clone();
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.clone());
    let (tx, mut rx) = mpsc::unbounded_channel();

    let debouncer = new_debouncer_opt::<_, RecommendedWatcher, NoCache>(
        Duration::from_millis(250),
        None,
        move |res: DebounceEventResult| {
            if let Ok(events) = res {
                let _ = tx.send(events);
            }
        },
        NoCache::new(),
        Config::default(),
    )
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("watcher init: {e}")))?;

    let mut watcher = debouncer;
    watcher
        .watch(&canonical_root, RecursiveMode::Recursive)
        .map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("watch {}: {e}", canonical_root.display()),
            )
        })?;

    let initial_paths: HashSet<String> = index_worktree(&canonical_root)
        .map(|(paths, _)| paths.into_iter().collect())
        .unwrap_or_default();

    let root_task = original_root;
    let canonical_task = canonical_root;
    let worker_task = tokio::spawn(async move {
        let mut known_paths = initial_paths;
        while let Some(events_batch) = rx.recv().await {
            process_debounced_events(
                &events,
                &file_indices,
                lane_id,
                &root_task,
                &canonical_task,
                &mut known_paths,
                events_batch,
            )
            .await;
        }
    });

    Ok(WorktreeWatcher {
        _debouncer: watcher,
        worker_abort: worker_task.abort_handle(),
    })
}

async fn invalidate_file_index(
    file_indices: &Arc<Mutex<HashMap<LaneId, CachedIndex>>>,
    lane_id: LaneId,
) {
    let mut indices = file_indices.lock().await;
    let entry = indices.entry(lane_id).or_insert_with(|| CachedIndex {
        generation: 0,
        paths: Vec::new(),
        truncated: false,
        valid: false,
    });
    entry.generation = entry.generation.wrapping_add(1);
    entry.valid = false;
    entry.paths.clear();
    entry.truncated = false;
}

fn broadcast_file_changed(events: &EventTx, payload: serde_json::Value) {
    let note = Notification::new("event.file.changed", payload);
    if let Ok(value) = serde_json::to_value(&note) {
        let _ = events.send(value);
    }
}

fn rel_for_path(root: &Path, canonical_root: &Path, path: &Path) -> Option<String> {
    let rel = path
        .strip_prefix(canonical_root)
        .or_else(|_| path.strip_prefix(root))
        .ok()?;
    let s = rel.to_string_lossy().replace('\\', "/");
    if is_git_internal(&s) {
        None
    } else {
        Some(s)
    }
}

async fn process_debounced_events(
    events: &EventTx,
    file_indices: &Arc<Mutex<HashMap<LaneId, CachedIndex>>>,
    lane_id: LaneId,
    root: &Path,
    canonical_root: &Path,
    known_paths: &mut HashSet<String>,
    debounced_list: Vec<notify_debouncer_full::DebouncedEvent>,
) {
    for debounced in debounced_list {
        let event = debounced.event;
        if let notify::EventKind::Modify(notify::event::ModifyKind::Name(_)) = event.kind {
            if event.paths.len() >= 2 {
                let Some(from_str) = rel_for_path(root, canonical_root, &event.paths[0]) else {
                    continue;
                };
                let Some(to_str) = rel_for_path(root, canonical_root, &event.paths[1]) else {
                    continue;
                };

                let ignored = check_ignored(root, &[from_str.clone(), to_str.clone()]);
                if ignored.contains(&to_str) {
                    if !ignored.contains(&from_str) {
                        known_paths.remove(&from_str);
                        invalidate_file_index(file_indices, lane_id).await;
                        broadcast_file_changed(
                            events,
                            json!({
                                "lane_id": lane_id,
                                "path": from_str,
                                "op": "removed",
                            }),
                        );
                    }
                    continue;
                }

                known_paths.remove(&from_str);
                known_paths.insert(to_str.clone());
                invalidate_file_index(file_indices, lane_id).await;
                broadcast_file_changed(
                    events,
                    json!({
                        "lane_id": lane_id,
                        "path": to_str,
                        "op": "renamed",
                        "from": from_str,
                    }),
                );
                continue;
            }
        }

        for path in &event.paths {
            let Some(rel_str) = rel_for_path(root, canonical_root, path) else {
                continue;
            };
            if check_ignored(root, &[rel_str.clone()]).contains(&rel_str) {
                continue;
            }

            let exists = path.exists() || root.join(&rel_str).exists();
            let op = if !exists || matches!(event.kind, notify::EventKind::Remove(_)) {
                known_paths.remove(&rel_str);
                "removed"
            } else if known_paths.contains(&rel_str) {
                "modified"
            } else {
                known_paths.insert(rel_str.clone());
                "created"
            };

            invalidate_file_index(file_indices, lane_id).await;
            broadcast_file_changed(
                events,
                json!({
                    "lane_id": lane_id,
                    "path": rel_str,
                    "op": op,
                }),
            );
        }
    }
}

fn is_git_internal(rel: &str) -> bool {
    rel.is_empty() || rel == ".git" || rel.starts_with(".git/")
}
