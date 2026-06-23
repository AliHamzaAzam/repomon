//! Lane management: list / get / create / delete / focus.
//!
//! A lane is the materialized `(repo, worktree)` join. `list` enumerates every worktree of
//! every repo, computes live state, and assembles lanes (agent sessions are overlaid later,
//! in Phase 2). `create` runs `git worktree add`; `delete` runs `git worktree remove`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::git::{reader, worktree};
use crate::model::{CreateLaneParams, Lane, LaneId, Repo, RepoId, WorktreeState};
use crate::store::Store;

fn join_err(e: tokio::task::JoinError) -> Error {
    Error::Other(format!("task failed: {e}"))
}

fn null_oid() -> gix::ObjectId {
    gix::ObjectId::null(gix::hash::Kind::Sha1)
}

fn same_path(a: &Path, b: &Path) -> bool {
    let ca = a.canonicalize();
    let cb = b.canonicalize();
    match (ca, cb) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// How long a *clean* (not-since-invalidated) cached worktree state stays valid before a safety
/// refresh — for changes the file watcher doesn't cover (e.g. linked worktrees outside a watched
/// repo root). A worktree the watcher flags as changed re-walks on the next list regardless, so
/// this can be generous; the refresh is also capped per list so it never re-walks all at once.
const STATE_TTL: Duration = Duration::from_secs(180);
/// How long a repo's cached `git worktree list` stays valid. Worktrees change only on lane
/// create/delete (which clear the cache) or external `git worktree` ops; this TTL bounds the
/// latter while sparing a git subprocess per repo on every overlay.
const WORKTREES_TTL: Duration = Duration::from_secs(10);

/// A cached worktree git-state: when it was last walked, whether the watcher has flagged it dirty
/// since, and the state itself.
struct StateEntry {
    walked_at: Instant,
    dirty: bool,
    state: WorktreeState,
}

/// Per-repo cache of `git worktree list` results, keyed by repo path.
type WorktreeCache = Arc<Mutex<HashMap<PathBuf, (Instant, Vec<worktree::WorktreeEntry>)>>>;

/// Manages lanes across all registered repos.
#[derive(Clone)]
pub struct Lanes {
    store: Store,
    config: Config,
    /// Per-worktree git state cache (keyed by worktree path). The gix status walk is the dominant
    /// cost of `list`; we reuse a recent result and only re-walk a worktree the file watcher
    /// flagged as changed (see [`Lanes::invalidate_state`]) or after [`STATE_TTL`]. Shared across
    /// clones via `Arc` so a watcher invalidation reaches every handler.
    state_cache: Arc<Mutex<HashMap<PathBuf, StateEntry>>>,
    /// Per-repo `git worktree list` cache (keyed by repo path), so a repo's worktrees aren't
    /// re-enumerated with a git subprocess on every overlay. Cleared by create/delete; otherwise
    /// bounded by [`WORKTREES_TTL`]. Shared across clones via `Arc`.
    worktrees_cache: WorktreeCache,
}

impl Lanes {
    pub fn new(store: Store, config: Config) -> Self {
        Self {
            store,
            config,
            state_cache: Arc::new(Mutex::new(HashMap::new())),
            worktrees_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Enumerate every lane across all repos, with live worktree state.
    pub async fn list(&self) -> Result<Vec<Lane>> {
        let repos = self.store.list_repos().await?;
        let metas = self.store.list_lane_meta().await?;

        // Phase 1a — each repo's worktrees, reusing a recent `git worktree list` instead of forking
        // git per repo on every overlay (worktrees change only on create/delete or external git
        // ops). Cache misses run in parallel.
        let mut repo_entries: Vec<(Repo, Vec<worktree::WorktreeEntry>)> = Vec::new();
        let mut wt_misses: Vec<Repo> = Vec::new();
        {
            let cache = self.worktrees_cache.lock().unwrap_or_else(|e| e.into_inner());
            for repo in repos {
                match cache.get(&repo.path) {
                    Some((t, entries)) if t.elapsed() < WORKTREES_TTL => {
                        repo_entries.push((repo, entries.clone()));
                    }
                    _ => wt_misses.push(repo),
                }
            }
        }
        let wt_handles: Vec<_> = wt_misses
            .iter()
            .map(|repo| {
                let rp = repo.path.clone();
                tokio::task::spawn_blocking(move || worktree::list(&rp))
            })
            .collect();
        let mut wt_fresh = Vec::with_capacity(wt_handles.len());
        for h in wt_handles {
            wt_fresh.push(h.await.map_err(join_err)?);
        }
        {
            let mut cache = self.worktrees_cache.lock().unwrap_or_else(|e| e.into_inner());
            for (repo, entries_res) in wt_misses.into_iter().zip(wt_fresh) {
                // A repo that's gone missing on disk shouldn't sink the whole fleet view.
                if let Ok(entries) = entries_res {
                    cache.insert(repo.path.clone(), (Instant::now(), entries.clone()));
                    repo_entries.push((repo, entries));
                }
            }
        }

        // Phase 1b — upsert each worktree's DB rows (cheap), collecting what each needs for its
        // (expensive) git-state read, which we then run in parallel.
        let mut pending = Vec::new();
        for (repo, entries) in repo_entries {
            let mut keep = Vec::new();
            for entry in entries {
                if entry.bare {
                    continue;
                }
                keep.push(entry.path.to_string_lossy().into_owned());

                let is_main = same_path(&entry.path, &repo.path);
                let name = entry
                    .path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "?".to_string());
                let head = entry.head.unwrap_or_else(null_oid);

                let wt = self
                    .store
                    .upsert_worktree(
                        repo.id,
                        entry.path.clone(),
                        entry.branch.clone(),
                        head,
                        is_main,
                        name,
                    )
                    .await?;
                let lane_id = self
                    .store
                    .get_or_create_lane(repo.id, entry.path.to_string_lossy().into_owned())
                    .await?;

                pending.push((repo.clone(), entry, wt, lane_id, head));
            }

            self.store.prune_worktrees(repo.id, keep).await?;
        }

        // Phase 2 — each worktree's live git state. `read_state` (gix status) is a full directory
        // walk and the dominant cost of `lane.list`, so we cache it per worktree: reuse a recent
        // state and only re-walk worktrees the watcher flagged as changed (`dirty`) or that are
        // first-seen. The clean-but-TTL-expired refresh is capped per call (oldest first) so a
        // synchronized expiry never re-walks every worktree at once — that spiked CPU; the rest
        // keep their slightly-stale state and refresh on a later list. Walks run in parallel.
        // (Agent transcript writes touch no worktree, so they never invalidate a cached state.)
        const WALK_CAP: usize = 2;
        let mut states: Vec<Option<Result<WorktreeState>>> =
            std::iter::repeat_with(|| None).take(pending.len()).collect();
        let mut must_walk: Vec<usize> = Vec::new(); // first-seen or watcher-invalidated
        let mut stale: Vec<(usize, Instant)> = Vec::new(); // clean but past the TTL (reusable)
        {
            let cache = self.state_cache.lock().unwrap_or_else(|e| e.into_inner());
            for (i, (_, entry, _, _, _)) in pending.iter().enumerate() {
                match cache.get(&entry.path) {
                    None => must_walk.push(i),
                    Some(e) if e.dirty => must_walk.push(i),
                    Some(e) if e.walked_at.elapsed() < STATE_TTL => {
                        states[i] = Some(Ok(e.state.clone()));
                    }
                    Some(e) => {
                        states[i] = Some(Ok(e.state.clone())); // reuse for now…
                        stale.push((i, e.walked_at)); // …but eligible to refresh (oldest first)
                    }
                }
            }
        }
        stale.sort_by_key(|(_, t)| *t);
        let walk: Vec<usize> = must_walk
            .iter()
            .copied()
            .chain(stale.iter().take(WALK_CAP).map(|(i, _)| *i))
            .collect();
        let handles: Vec<_> = walk
            .iter()
            .map(|&i| {
                let (_, entry, wt, _, _) = &pending[i];
                let p = entry.path.clone();
                let wid = wt.id;
                tokio::task::spawn_blocking(move || reader::read_state(&p, wid))
            })
            .collect();
        let mut fresh = Vec::with_capacity(handles.len());
        for h in handles {
            fresh.push(h.await.map_err(join_err)?);
        }
        {
            let mut cache = self.state_cache.lock().unwrap_or_else(|e| e.into_inner());
            for (&i, st) in walk.iter().zip(fresh) {
                if let Ok(s) = &st {
                    cache.insert(
                        pending[i].1.path.clone(),
                        StateEntry {
                            walked_at: Instant::now(),
                            dirty: false,
                            state: s.clone(),
                        },
                    );
                }
                states[i] = Some(st);
            }
        }

        // Phase 3 — assemble the lanes.
        let mut lanes = Vec::with_capacity(pending.len());
        for ((repo, entry, wt, lane_id, head), st) in pending.into_iter().zip(states) {
            // Fall back to a prunable placeholder if the worktree dir is gone.
            let mut state = match st {
                Some(Ok(s)) => s,
                _ => WorktreeState {
                    worktree_id: wt.id,
                    head,
                    branch: entry.branch.clone(),
                    upstream: None,
                    ahead: 0,
                    behind: 0,
                    dirty: Default::default(),
                    last_commit_at: None,
                    locked: entry.locked.is_some(),
                    prunable: true,
                    last_change_at: None,
                },
            };
            state.locked = entry.locked.is_some();
            if entry.prunable.is_some() {
                state.prunable = true;
            }

            let pinned = metas
                .iter()
                .find(|m| m.id == lane_id)
                .map(|m| m.pinned)
                .unwrap_or(false);
            let last_activity_at = state.last_commit_at.unwrap_or(repo.added_at);

            lanes.push(Lane {
                id: lane_id,
                repo,
                worktree: wt,
                state,
                agent_sessions: Vec::new(),
                last_activity_at,
                pinned,
            });
        }

        sort_lanes(&mut lanes);
        Ok(lanes)
    }

    /// Drop cached git state for the worktree that owns `root` so the next `list` re-walks it. The
    /// file watcher calls this the moment a worktree's files change, keeping the fleet's dirty state
    /// fresh without re-walking every worktree on every poll.
    ///
    /// A changed path belongs to a *single* worktree — the one whose cached path is the longest
    /// prefix of the change (mirroring `watch::classify`'s `max_by_key(len)` ownership rule). The
    /// earlier bidirectional `p.starts_with(root) || root.starts_with(p)` test also flagged the
    /// PARENT worktree whenever a nested worktree changed, over-invalidating the cache and forcing
    /// needless re-walks of the parent.
    pub fn invalidate_state(&self, root: &Path) {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let mut cache = self.state_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = cache
            .iter_mut()
            .filter(|(p, _)| root.starts_with(p))
            .max_by_key(|(p, _)| p.as_os_str().len())
            .map(|(_, e)| e)
        {
            e.dirty = true;
        }
    }

    pub async fn get(&self, id: LaneId) -> Result<Lane> {
        self.list()
            .await?
            .into_iter()
            .find(|l| l.id == id)
            .ok_or_else(|| Error::NotFound(format!("lane {id}")))
    }

    /// Create a new lane: add a worktree (with `-b` for a new branch), honor the path
    /// template, and copy any requested files into the new worktree.
    pub async fn create(&self, params: CreateLaneParams) -> Result<Lane> {
        let repo = self.store.get_repo(params.repo_id).await?;
        let branch = params.branch.clone();
        let path = match &params.path {
            Some(p) => p.clone(),
            None => self.template_path(&repo, &branch),
        };

        let repo_path = repo.path.clone();
        let b = branch.clone();
        let exists = tokio::task::spawn_blocking(move || branch_exists(&repo_path, &b))
            .await
            .map_err(join_err)?;
        let create_branch = !exists;

        let rp = repo.path.clone();
        let np = path.clone();
        let src = params.source_branch.clone();
        let b2 = branch.clone();
        tokio::task::spawn_blocking(move || {
            worktree::add(&rp, &np, &b2, src.as_deref(), create_branch)
        })
        .await
        .map_err(join_err)??;

        if !params.copy_files.is_empty() {
            let rp = repo.path.clone();
            let np = path.clone();
            let pats = params.copy_files.clone();
            tokio::task::spawn_blocking(move || copy_matching(&rp, &np, &pats))
                .await
                .map_err(join_err)??;
        }

        self.store
            .get_or_create_lane(repo.id, path.to_string_lossy().into_owned())
            .await?;

        // A worktree was added — drop the cached enumeration so list() picks it up at once.
        self.worktrees_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();

        // Re-list and return the freshly created lane.
        self.list()
            .await?
            .into_iter()
            .find(|l| same_path(&l.worktree.path, &path))
            .ok_or_else(|| Error::Other("created lane not found after worktree add".into()))
    }

    /// Remove a lane's worktree (refuses the main worktree), optionally deleting its branch.
    pub async fn delete(&self, id: LaneId, also_delete_branch: bool) -> Result<()> {
        let meta = self
            .store
            .list_lane_meta()
            .await?
            .into_iter()
            .find(|m| m.id == id)
            .ok_or_else(|| Error::NotFound(format!("lane {id}")))?;
        let repo = self.store.get_repo(meta.repo_id).await?;
        let wt_path = meta.worktree_path.clone();

        if same_path(&wt_path, &repo.path) {
            return Err(Error::Other("cannot delete the main worktree".into()));
        }

        let branch = if also_delete_branch {
            let rp = repo.path.clone();
            let wp = wt_path.clone();
            tokio::task::spawn_blocking(move || worktree_branch(&rp, &wp))
                .await
                .map_err(join_err)?
        } else {
            None
        };

        let rp = repo.path.clone();
        let wp = wt_path.clone();
        tokio::task::spawn_blocking(move || worktree::remove(&rp, &wp, false))
            .await
            .map_err(join_err)??;

        if let Some(b) = branch {
            let rp = repo.path.clone();
            tokio::task::spawn_blocking(move || delete_branch(&rp, &b))
                .await
                .map_err(join_err)??;
        }
        // A worktree was removed — drop the cached enumeration and its stale state entry.
        self.worktrees_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.state_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&wt_path);
        Ok(())
    }

    /// Quick-merge a lane's branch into the repo's main worktree (best-effort).
    ///
    /// Runs `git -C <main-worktree> merge --no-edit <lane-branch>`. The main worktree must be
    /// on the target branch and clean; conflicts surface as an error. Returns a status message.
    pub async fn merge(&self, id: LaneId, into: Option<String>) -> Result<String> {
        let meta = self
            .store
            .list_lane_meta()
            .await?
            .into_iter()
            .find(|m| m.id == id)
            .ok_or_else(|| Error::NotFound(format!("lane {id}")))?;
        let repo = self.store.get_repo(meta.repo_id).await?;
        let wt_path = meta.worktree_path.clone();

        if same_path(&wt_path, &repo.path) {
            return Err(Error::Other("this lane is the main worktree".into()));
        }
        let branch = {
            let rp = repo.path.clone();
            let wp = wt_path.clone();
            tokio::task::spawn_blocking(move || worktree_branch(&rp, &wp))
                .await
                .map_err(join_err)?
        }
        .ok_or_else(|| Error::Other("lane has no branch to merge (detached HEAD)".into()))?;

        let rp = repo.path.clone();
        let b = branch.clone();
        tokio::task::spawn_blocking(move || merge_branch(&rp, &b, into.as_deref()))
            .await
            .map_err(join_err)?
    }

    /// The filesystem path to cd into for a lane.
    pub async fn focus(&self, id: LaneId) -> Result<PathBuf> {
        self.store
            .list_lane_meta()
            .await?
            .into_iter()
            .find(|m| m.id == id)
            .map(|m| m.worktree_path)
            .ok_or_else(|| Error::NotFound(format!("lane {id}")))
    }

    fn template_path(&self, repo: &Repo, branch: &str) -> PathBuf {
        let template = repo
            .worktree_root_template
            .clone()
            .unwrap_or_else(|| self.config.worktree_template_for(&repo.name).to_string());
        let safe_branch = branch.replace('/', "-");
        let rendered = template
            .replace("{repo}", &repo.name)
            .replace("{branch}", &safe_branch);
        expand_tilde(&rendered)
    }
}

fn sort_lanes(lanes: &mut [Lane]) {
    use std::collections::HashMap;
    // Most-recent activity per repo, so repos sort by their liveliest lane.
    let mut repo_activity: HashMap<RepoId, chrono::DateTime<chrono::Utc>> = HashMap::new();
    for l in lanes.iter() {
        let e = repo_activity.entry(l.repo.id).or_insert(l.last_activity_at);
        if l.last_activity_at > *e {
            *e = l.last_activity_at;
        }
    }
    lanes.sort_by(|a, b| {
        let ra = repo_activity[&a.repo.id];
        let rb = repo_activity[&b.repo.id];
        rb.cmp(&ra) // repos: newest activity first
            .then(a.repo.id.cmp(&b.repo.id)) // stable grouping
            .then(b.worktree.is_main.cmp(&a.worktree.is_main)) // main first
            .then(b.last_activity_at.cmp(&a.last_activity_at)) // then activity desc
    });
}

fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(base) = directories::BaseDirs::new() {
            return base.home_dir().join(rest);
        }
    }
    PathBuf::from(s)
}

fn branch_exists(repo_path: &Path, branch: &str) -> bool {
    match gix::open(repo_path) {
        Ok(r) => r.find_reference(&format!("refs/heads/{branch}")).is_ok(),
        Err(_) => false,
    }
}

fn worktree_branch(repo_path: &Path, wt_path: &Path) -> Option<String> {
    worktree::list(repo_path)
        .ok()?
        .into_iter()
        .find(|e| same_path(&e.path, wt_path))
        .and_then(|e| e.branch)
}

/// Merge `branch` into the main worktree (which must already be on the target branch).
fn merge_branch(repo_path: &Path, branch: &str, into: Option<&str>) -> Result<String> {
    if let Some(target) = into {
        // Best-effort: ensure the main worktree is on the requested target branch.
        let out = Command::new("git")
            .arg("-C")
            .arg(repo_path)
            .args(["symbolic-ref", "--short", "HEAD"])
            .output()
            .map_err(Error::Io)?;
        let current = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if current != target {
            return Err(Error::Other(format!(
                "main worktree is on '{current}', not '{target}'; switch it first"
            )));
        }
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["merge", "--no-edit", branch])
        .output()
        .map_err(Error::Io)?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "merge {branch}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(format!(
        "merged {branch} ({})",
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("ok")
            .trim()
    ))
}

fn delete_branch(repo_path: &Path, branch: &str) -> Result<()> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["branch", "-D", branch])
        .output()
        .map_err(Error::Io)?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "git branch -D {branch}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Copy files matching `patterns` (gitignore-style globs) from `repo_root` into
/// `dest_root`, preserving relative paths.
fn copy_matching(repo_root: &Path, dest_root: &Path, patterns: &[String]) -> Result<()> {
    let mut builder = globset::GlobSetBuilder::new();
    for p in patterns {
        if let Ok(g) = globset::Glob::new(p) {
            builder.add(g);
        }
    }
    let set = builder.build().map_err(|e| Error::Other(e.to_string()))?;

    let mut stack = vec![repo_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            let fname = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if p.is_dir() {
                if !matches!(fname, ".git" | "node_modules" | "target") {
                    stack.push(p);
                }
                continue;
            }
            let rel = p.strip_prefix(repo_root).unwrap_or(&p);
            if set.is_match(rel) || set.is_match(Path::new(fname)) {
                let dest = dest_root.join(rel);
                if let Some(parent) = dest.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::copy(&p, &dest);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@e.com")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@e.com")
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    async fn repo_with_commit() -> (tempfile::TempDir, Store, Config) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-b", "main"]);
        std::fs::write(p.join("README.md"), "hi\n").unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-m", "init"]);
        let store = Store::open_in_memory().unwrap();
        (dir, store, Config::default())
    }

    #[tokio::test]
    async fn lists_main_worktree_as_a_lane() {
        let (dir, store, cfg) = repo_with_commit().await;
        let reg = Registry::new(store.clone());
        reg.add(dir.path()).await.unwrap();
        let lanes = Lanes::new(store, cfg).list().await.unwrap();
        assert_eq!(lanes.len(), 1);
        assert!(lanes[0].worktree.is_main);
        assert_eq!(lanes[0].state.branch.as_deref(), Some("main"));
    }

    #[tokio::test]
    async fn create_then_delete_lane() {
        let (dir, store, cfg) = repo_with_commit().await;
        let reg = Registry::new(store.clone());
        let repo = reg.add(dir.path()).await.unwrap();
        let lanes = Lanes::new(store.clone(), cfg.clone());

        let wt_parent = tempfile::tempdir().unwrap();
        let wt_path = wt_parent.path().join("feat");
        let lane = lanes
            .create(CreateLaneParams {
                repo_id: repo.id,
                branch: "feat/thing".into(),
                source_branch: Some("main".into()),
                path: Some(wt_path.clone()),
                copy_files: vec![],
            })
            .await
            .unwrap();
        assert_eq!(lane.state.branch.as_deref(), Some("feat/thing"));
        assert!(!lane.worktree.is_main);
        assert_eq!(lanes.list().await.unwrap().len(), 2);

        lanes.delete(lane.id, true).await.unwrap();
        let after = lanes.list().await.unwrap();
        assert_eq!(after.len(), 1);
        assert!(after[0].worktree.is_main);
    }

    #[tokio::test]
    async fn worktree_file_activity_is_detected() {
        use chrono::Utc;
        let (dir, store, cfg) = repo_with_commit().await;
        let reg = Registry::new(store.clone());
        reg.add(dir.path()).await.unwrap();
        let lanes = Lanes::new(store, cfg);

        // A clean worktree has no recent file-change signal.
        let before = lanes.list().await.unwrap();
        assert!(before[0].state.last_change_at.is_none());

        // Writing an untracked file makes the worktree show recent activity (the "file activity"
        // signal the daemon uses to surface agents that leave no transcript).
        std::fs::write(dir.path().join("scratch.txt"), "work\n").unwrap();
        // The file watcher flags the worktree on this write; do so explicitly so the cached clean
        // state is re-walked (back-to-back list calls otherwise reuse the per-worktree cache).
        lanes.invalidate_state(dir.path());
        let after = lanes.list().await.unwrap();
        let changed = after[0]
            .state
            .last_change_at
            .expect("a dirty worktree should report a change time");
        assert!((Utc::now() - changed).num_seconds().abs() < 60);
    }

    #[tokio::test]
    async fn invalidate_marks_only_the_longest_matching_worktree() {
        // A nested worktree lives under a parent worktree's path. A change inside the nested
        // worktree must dirty *only* the nested entry (the longest cached prefix of the change),
        // not the parent — the old bidirectional prefix test over-invalidated the parent.
        let base = tempfile::tempdir().unwrap();
        // Canonicalize so the manually-seeded keys match what `invalidate_state` canonicalizes to.
        let parent = base.path().canonicalize().unwrap();
        let nested = parent.join("nested");
        std::fs::create_dir(&nested).unwrap();

        let store = Store::open_in_memory().unwrap();
        let lanes = Lanes::new(store, Config::default());

        let entry = || StateEntry {
            walked_at: Instant::now(),
            dirty: false,
            state: WorktreeState {
                worktree_id: 1,
                head: null_oid(),
                branch: None,
                upstream: None,
                ahead: 0,
                behind: 0,
                dirty: Default::default(),
                last_commit_at: None,
                locked: false,
                prunable: false,
                last_change_at: None,
            },
        };
        {
            let mut cache = lanes.state_cache.lock().unwrap_or_else(|e| e.into_inner());
            cache.insert(parent.clone(), entry());
            cache.insert(nested.clone(), entry());
        }

        // A change inside the nested worktree.
        lanes.invalidate_state(&nested.join("file.txt"));

        let cache = lanes.state_cache.lock().unwrap_or_else(|e| e.into_inner());
        assert!(cache[&nested].dirty, "nested worktree should be flagged");
        assert!(
            !cache[&parent].dirty,
            "parent worktree must not be flagged by a nested change"
        );
    }

    #[tokio::test]
    async fn refuses_to_delete_main_worktree() {
        let (dir, store, cfg) = repo_with_commit().await;
        let reg = Registry::new(store.clone());
        reg.add(dir.path()).await.unwrap();
        let lanes = Lanes::new(store, cfg);
        let main = lanes.list().await.unwrap().into_iter().next().unwrap();
        assert!(lanes.delete(main.id, false).await.is_err());
    }
}
