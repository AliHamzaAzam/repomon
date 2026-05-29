//! Repo registry: add / remove / list / discover.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::model::{Repo, RepoId};
use crate::store::Store;

fn join_err(e: tokio::task::JoinError) -> Error {
    Error::Other(format!("task failed: {e}"))
}

/// Adds, removes, lists, and discovers git repositories.
#[derive(Clone)]
pub struct Registry {
    store: Store,
}

impl Registry {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    /// Register the repo containing `path`. The stored path is the main worktree,
    /// canonicalized. Adding an already-registered repo returns the existing record.
    pub async fn add(&self, path: &Path) -> Result<Repo> {
        let input = path.to_path_buf();
        let resolved = tokio::task::spawn_blocking(move || resolve_main_worktree(&input))
            .await
            .map_err(join_err)??;

        if let Some(existing) = self.store.find_repo_by_path(resolved.clone()).await? {
            return Ok(existing);
        }
        let name = resolved
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".to_string());
        self.store.add_repo(resolved, name, None).await
    }

    pub async fn remove(&self, id: RepoId) -> Result<()> {
        self.store.remove_repo(id).await
    }

    pub async fn list(&self) -> Result<Vec<Repo>> {
        self.store.list_repos().await
    }

    /// Recursively find git repositories under `root` (to `max_depth`), without
    /// descending into discovered repos or common heavy directories.
    pub async fn discover(&self, root: &Path, max_depth: usize) -> Result<Vec<PathBuf>> {
        let root = root.to_path_buf();
        tokio::task::spawn_blocking(move || discover_walk(&root, max_depth))
            .await
            .map_err(join_err)
    }
}

/// Resolve any path inside a repo (main or linked worktree) to the main worktree path.
fn resolve_main_worktree(input: &Path) -> Result<PathBuf> {
    let repo = gix::open(input).map_err(|e| Error::Git(e.to_string()))?;
    // The common dir is `<main-worktree>/.git`; its parent is the main worktree.
    let common = repo.common_dir();
    let main = if common.file_name() == Some(OsStr::new(".git")) {
        common.parent().map(Path::to_path_buf)
    } else {
        None
    };
    let path = main
        .or_else(|| repo.workdir().map(Path::to_path_buf))
        .unwrap_or_else(|| input.to_path_buf());
    Ok(path.canonicalize().unwrap_or(path))
}

fn discover_walk(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if dir.join(".git").exists() {
            // It's a repo — record it and don't descend further.
            found.push(dir.canonicalize().unwrap_or(dir));
            continue;
        }
        if depth >= max_depth {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            let skip = matches!(
                p.file_name().and_then(OsStr::to_str),
                Some(".git" | "node_modules" | "target" | ".cargo" | ".venv" | "vendor")
            );
            if !skip {
                stack.push((p, depth + 1));
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_finds_repos_and_skips_heavy_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // root/a/.git, root/b/c/.git, root/node_modules/x/.git (must be skipped)
        std::fs::create_dir_all(root.join("a/.git")).unwrap();
        std::fs::create_dir_all(root.join("b/c/.git")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/x/.git")).unwrap();

        let found = discover_walk(root, 4);
        let names: Vec<String> = found
            .iter()
            .map(|p| {
                p.strip_prefix(root.canonicalize().unwrap())
                    .unwrap_or(p)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert!(names.iter().any(|n| n == "a"), "found {names:?}");
        assert!(names.iter().any(|n| n == "b/c"), "found {names:?}");
        assert!(
            !names.iter().any(|n| n.contains("node_modules")),
            "found {names:?}"
        );
    }

    #[test]
    fn discover_respects_depth() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("a/b/c/d/.git")).unwrap();
        // Depth 2 is too shallow to reach a/b/c/d.
        assert!(discover_walk(root, 2).is_empty());
        assert_eq!(discover_walk(root, 5).len(), 1);
    }
}
