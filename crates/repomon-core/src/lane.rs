//! Lane management: list / get / create / delete / focus.
//!
//! A lane is the materialized `(repo, worktree)` join. `list` enumerates every worktree of
//! every repo, computes live state, and assembles lanes (agent sessions are overlaid later,
//! in Phase 2). `create` runs `git worktree add`; `delete` runs `git worktree remove`.

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Manages lanes across all registered repos.
#[derive(Clone)]
pub struct Lanes {
    store: Store,
    config: Config,
}

impl Lanes {
    pub fn new(store: Store, config: Config) -> Self {
        Self { store, config }
    }

    /// Enumerate every lane across all repos, with live worktree state.
    pub async fn list(&self) -> Result<Vec<Lane>> {
        let repos = self.store.list_repos().await?;
        let metas = self.store.list_lane_meta().await?;
        let mut lanes = Vec::new();

        for repo in repos {
            let rp = repo.path.clone();
            let entries = match tokio::task::spawn_blocking(move || worktree::list(&rp))
                .await
                .map_err(join_err)?
            {
                Ok(e) => e,
                // A repo that's gone missing on disk shouldn't sink the whole fleet view.
                Err(_) => continue,
            };

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

                // Live state; fall back to a prunable placeholder if the worktree dir is gone.
                let p = entry.path.clone();
                let wid = wt.id;
                let mut state =
                    match tokio::task::spawn_blocking(move || reader::read_state(&p, wid))
                        .await
                        .map_err(join_err)?
                    {
                        Ok(s) => s,
                        Err(_) => WorktreeState {
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
                    repo: repo.clone(),
                    worktree: wt,
                    state,
                    agent_sessions: Vec::new(),
                    last_activity_at,
                    pinned,
                });
            }

            self.store.prune_worktrees(repo.id, keep).await?;
        }

        sort_lanes(&mut lanes);
        Ok(lanes)
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
        Ok(())
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
    async fn refuses_to_delete_main_worktree() {
        let (dir, store, cfg) = repo_with_commit().await;
        let reg = Registry::new(store.clone());
        reg.add(dir.path()).await.unwrap();
        let lanes = Lanes::new(store, cfg);
        let main = lanes.list().await.unwrap().into_iter().next().unwrap();
        assert!(lanes.delete(main.id, false).await.is_err());
    }
}
