//! Debounced filesystem watching.
//!
//! We watch each worktree root recursively (FSEvents/inotify), debounce 250 ms, and classify
//! each changed path into a coarse [`ChangeKind`]. Anything under `.git/objects/` is dropped
//! (it churns constantly and tells us nothing useful), as are other `.git` internals we don't
//! care about. The daemon subscribes and re-syncs the affected repo on each change.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};
use tokio::sync::broadcast;

use crate::error::{Error, Result};

/// What kind of change a path represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    Head,
    Refs,
    Index,
    Worktree,
}

/// A debounced change within a watched worktree root.
#[derive(Debug, Clone)]
pub struct RepoChange {
    /// The watched root the change belongs to.
    pub path: PathBuf,
    pub kind: ChangeKind,
}

/// A filesystem watcher over a set of worktree roots.
pub struct Watcher {
    debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    tx: broadcast::Sender<RepoChange>,
    roots: Arc<Mutex<Vec<PathBuf>>>,
}

impl Watcher {
    /// Create a watcher with an internal broadcast channel.
    pub fn new() -> Result<Self> {
        let (tx, _rx) = broadcast::channel(512);
        let roots: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));

        let tx_cb = tx.clone();
        let roots_cb = roots.clone();
        let debouncer = new_debouncer(
            Duration::from_millis(250),
            None,
            move |res: DebounceEventResult| {
                let Ok(events) = res else { return };
                let roots = roots_cb.lock().unwrap();
                let mut seen = HashSet::new();
                for event in events {
                    for path in &event.event.paths {
                        if let Some((root, kind)) = classify(&roots, path) {
                            if seen.insert((root.clone(), kind)) {
                                let _ = tx_cb.send(RepoChange { path: root, kind });
                            }
                        }
                    }
                }
            },
        )
        .map_err(|e| Error::Other(format!("watcher init: {e}")))?;

        Ok(Watcher {
            debouncer,
            tx,
            roots,
        })
    }

    /// Subscribe to change events.
    pub fn subscribe(&self) -> broadcast::Receiver<RepoChange> {
        self.tx.subscribe()
    }

    /// Begin watching a worktree root recursively.
    pub fn watch_path(&mut self, root: &Path) -> Result<()> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        self.debouncer
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|e| Error::Other(format!("watch {}: {e}", root.display())))?;
        self.roots.lock().unwrap().push(root);
        Ok(())
    }

    /// Stop watching a worktree root.
    pub fn unwatch_path(&mut self, root: &Path) -> Result<()> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let _ = self.debouncer.unwatch(&root);
        self.roots.lock().unwrap().retain(|r| r != &root);
        Ok(())
    }
}

/// Map a changed path to its owning root and a coarse change kind, or `None` to drop it.
fn classify(roots: &[PathBuf], path: &Path) -> Option<(PathBuf, ChangeKind)> {
    let root = roots
        .iter()
        .filter(|r| path.starts_with(r))
        .max_by_key(|r| r.as_os_str().len())?
        .clone();

    let s = path.to_string_lossy();
    if s.contains("/.git/objects/") {
        return None;
    }
    // Build / dependency / tooling output churns constantly and is gitignored, so it never moves
    // the tracked git status we surface — don't wake a re-sync (and a gix status walk) for it.
    const NOISE: &[&str] = &[
        "/target/",
        "/node_modules/",
        "/.next/",
        "/.nuxt/",
        "/.turbo/",
        "/.venv/",
        "/__pycache__/",
        "/.shopify/",
        "/.pnpm-store/",
        "/.gradle/",
    ];
    if NOISE.iter().any(|n| s.contains(n)) {
        return None;
    }
    let in_git = s.contains("/.git/") || s.ends_with("/.git");
    let kind = if !in_git {
        ChangeKind::Worktree
    } else if s.ends_with("/HEAD") {
        ChangeKind::Head
    } else if s.contains("/refs/") {
        ChangeKind::Refs
    } else if s.ends_with("/index") {
        ChangeKind::Index
    } else {
        return None;
    };
    Some((root, kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify_str(roots: &[&str], p: &str) -> Option<ChangeKind> {
        let roots: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
        classify(&roots, Path::new(p)).map(|(_, k)| k)
    }

    #[test]
    fn classifies_git_internals() {
        let r = &["/repo"];
        assert_eq!(classify_str(r, "/repo/.git/HEAD"), Some(ChangeKind::Head));
        assert_eq!(
            classify_str(r, "/repo/.git/refs/heads/main"),
            Some(ChangeKind::Refs)
        );
        assert_eq!(classify_str(r, "/repo/.git/index"), Some(ChangeKind::Index));
        // Linked-worktree HEAD lives under .git/worktrees/<name>/HEAD.
        assert_eq!(
            classify_str(r, "/repo/.git/worktrees/x/HEAD"),
            Some(ChangeKind::Head)
        );
        // Objects churn is always dropped.
        assert_eq!(classify_str(r, "/repo/.git/objects/ab/cdef"), None);
        // Other .git internals are ignored.
        assert_eq!(classify_str(r, "/repo/.git/config"), None);
    }

    #[test]
    fn classifies_worktree_files() {
        assert_eq!(
            classify_str(&["/repo"], "/repo/src/main.rs"),
            Some(ChangeKind::Worktree)
        );
    }

    #[test]
    fn unknown_root_is_dropped() {
        assert_eq!(classify_str(&["/repo"], "/elsewhere/file"), None);
    }

    #[test]
    fn longest_root_wins_for_nested_worktrees() {
        // A worktree nested under another repo's tree should attribute to the closer root.
        let roots = &["/repo", "/repo/wt/feat"];
        let got = {
            let roots: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
            classify(&roots, Path::new("/repo/wt/feat/src/x.rs")).map(|(r, _)| r)
        };
        assert_eq!(got, Some(PathBuf::from("/repo/wt/feat")));
    }
}
