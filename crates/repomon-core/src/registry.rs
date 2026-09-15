//! Repo registry: add / remove / list / discover.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::model::{Repo, RepoId};
use crate::repo_json;
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
            return self.apply_repo_json(existing).await;
        }
        let name = resolved
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".to_string());
        let repo = self.store.add_repo(resolved, name, None).await?;
        self.apply_repo_json(repo).await
    }

    /// Adopt what the repository says about itself in its own `repo.json`.
    ///
    /// The file is the repository's claim, not this machine's: it fills the label only when nobody
    /// here has set one, so a local rename always wins. The accent has no local source, so it
    /// mirrors the file exactly — including being cleared when the file stops declaring a colour.
    pub async fn apply_repo_json(&self, repo: Repo) -> Result<Repo> {
        let path = repo.path.join("repo.json");
        let declaration = tokio::task::spawn_blocking(move || read_declaration(&path))
            .await
            .map_err(join_err)?;

        let declared = match declaration {
            // Nothing is known about this repository, so nothing already stored is disturbed.
            repo_json::Declaration::Unusable => return Ok(repo),
            repo_json::Declaration::None => repo_json::RepoJson::default(),
            repo_json::Declaration::Declared(d) => d,
        };

        let mut changed = false;
        if repo.label.is_none() {
            // A declared name equal to the folder name is not an override: writing it would leave a
            // stale label behind the moment the folder is renamed, for no visible difference.
            if let Some(name) = declared.name.filter(|n| *n != repo.name) {
                self.store.set_repo_label(repo.id, Some(name)).await?;
                changed = true;
            }
        }
        if declared.accent != repo.accent {
            self.store.set_repo_accent(repo.id, declared.accent).await?;
            changed = true;
        }
        if changed {
            self.store.get_repo(repo.id).await
        } else {
            Ok(repo)
        }
    }

    pub async fn remove(&self, id: RepoId) -> Result<()> {
        self.store.remove_repo(id).await
    }

    /// Hide or reveal a repo. The repo stays registered and watched; only client sidebars change.
    pub async fn set_hidden(&self, id: RepoId, hidden: bool) -> Result<()> {
        self.store.set_repo_hidden(id, hidden).await
    }

    /// Set a repo's display label (shown instead of the folder name). `None` clears the override.
    pub async fn set_label(&self, id: RepoId, label: Option<String>) -> Result<Repo> {
        self.store.set_repo_label(id, label).await?;
        self.store.get_repo(id).await
    }

    /// Persist a manual ordering. `ordered_ids` are assigned positions in list order; repos not
    /// listed keep their previous position.
    pub async fn reorder(&self, ordered_ids: Vec<RepoId>) -> Result<Vec<Repo>> {
        self.store.set_repo_order(ordered_ids).await?;
        self.store.list_repos().await
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

/// Read a repository's `repo.json` within bounds this machine sets rather than the repository.
///
/// A cloned repository is written by someone else, so the path is treated as hostile: only a
/// regular file is opened (a symlink would reach outside the checkout, a FIFO would block the
/// blocking pool forever), and the read stops one byte past the cap so a file that grows between
/// the stat and the read cannot get past it either.
fn read_declaration(path: &Path) -> repo_json::Declaration {
    use std::io::Read;

    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return repo_json::Declaration::None,
        Err(_) => return repo_json::Declaration::Unusable,
    };
    if !meta.file_type().is_file() || meta.len() > repo_json::MAX_BYTES {
        return repo_json::Declaration::Unusable;
    }

    let Ok(file) = std::fs::File::open(path) else {
        return repo_json::Declaration::Unusable;
    };
    let mut text = String::new();
    if file
        .take(repo_json::MAX_BYTES + 1)
        .read_to_string(&mut text)
        .is_err()
        || text.len() as u64 > repo_json::MAX_BYTES
    {
        return repo_json::Declaration::Unusable;
    }
    match repo_json::parse(&text) {
        Some(d) => repo_json::Declaration::Declared(d),
        None => repo_json::Declaration::Unusable,
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

    async fn repo_with(json: Option<&str>) -> (tempfile::TempDir, Registry, Repo) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("acme-platform");
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(j) = json {
            std::fs::write(dir.join("repo.json"), j).unwrap();
        }
        let reg = Registry::new(Store::open_in_memory().unwrap());
        let repo = reg
            .store
            .add_repo(dir, "acme-platform".to_string(), None)
            .await
            .unwrap();
        (tmp, reg, repo)
    }

    #[tokio::test]
    async fn adopts_a_declared_name_and_colour() {
        let (_t, reg, repo) =
            repo_with(Some(r##"{"name":"Acme Platform","color":"#0f766e"}"##)).await;
        let out = reg.apply_repo_json(repo).await.unwrap();
        assert_eq!(out.label.as_deref(), Some("Acme Platform"));
        assert_eq!(out.accent, Some(7));
        assert_eq!(out.name, "acme-platform", "identity must not move");
    }

    #[tokio::test]
    async fn a_local_rename_wins_over_the_file() {
        let (_t, reg, repo) = repo_with(Some(r##"{"name":"Acme Platform"}"##)).await;
        reg.store
            .set_repo_label(repo.id, Some("What I called it".into()))
            .await
            .unwrap();
        let repo = reg.store.get_repo(repo.id).await.unwrap();

        let out = reg.apply_repo_json(repo).await.unwrap();
        assert_eq!(out.label.as_deref(), Some("What I called it"));
    }

    #[tokio::test]
    async fn a_name_equal_to_the_folder_is_not_written_as_an_override() {
        let (_t, reg, repo) = repo_with(Some(r##"{"name":"acme-platform"}"##)).await;
        let out = reg.apply_repo_json(repo).await.unwrap();
        assert_eq!(
            out.label, None,
            "a redundant label would go stale on a rename"
        );
    }

    #[tokio::test]
    async fn a_repo_without_the_file_is_untouched() {
        let (_t, reg, repo) = repo_with(None).await;
        let out = reg.apply_repo_json(repo).await.unwrap();
        assert_eq!(out.label, None);
        assert_eq!(out.accent, None);
    }

    #[tokio::test]
    async fn an_accent_is_cleared_when_the_file_stops_declaring_one() {
        let (tmp, reg, repo) = repo_with(Some(r##"{"color":"#0f766e"}"##)).await;
        let repo = reg.apply_repo_json(repo).await.unwrap();
        assert_eq!(repo.accent, Some(7));

        // The repository changed its mind. A colour it no longer declares must not survive.
        std::fs::write(tmp.path().join("acme-platform/repo.json"), "{}").unwrap();
        let out = reg.apply_repo_json(repo).await.unwrap();
        assert_eq!(
            out.accent, None,
            "stale accent kept paneAccent off the id hash"
        );
    }

    #[tokio::test]
    async fn an_accent_is_cleared_when_the_file_is_deleted() {
        let (tmp, reg, repo) = repo_with(Some(r##"{"color":"#0f766e"}"##)).await;
        let repo = reg.apply_repo_json(repo).await.unwrap();
        assert_eq!(repo.accent, Some(7));

        std::fs::remove_file(tmp.path().join("acme-platform/repo.json")).unwrap();
        let out = reg.apply_repo_json(repo).await.unwrap();
        assert_eq!(out.accent, None, "a deleted file still declares nothing");
    }

    #[tokio::test]
    async fn a_file_we_cannot_use_leaves_what_is_stored_alone() {
        let (tmp, reg, repo) = repo_with(Some(r##"{"color":"#0f766e"}"##)).await;
        let repo = reg.apply_repo_json(repo).await.unwrap();
        assert_eq!(repo.accent, Some(7));

        // Unparseable is not the same statement as "declares nothing".
        std::fs::write(tmp.path().join("acme-platform/repo.json"), "{ truncated").unwrap();
        let out = reg.apply_repo_json(repo).await.unwrap();
        assert_eq!(
            out.accent,
            Some(7),
            "an unreadable file knows nothing, so it changes nothing"
        );
    }

    #[tokio::test]
    async fn an_oversized_file_is_refused_without_reading_it() {
        let (tmp, reg, repo) = repo_with(None).await;
        let path = tmp.path().join("acme-platform/repo.json");
        let padding = " ".repeat(repo_json::MAX_BYTES as usize + 1);
        std::fs::write(&path, format!(r##"{{"color":"#0f766e"}}{padding}"##)).unwrap();

        let out = reg.apply_repo_json(repo).await.unwrap();
        assert_eq!(
            out.accent, None,
            "size is capped by this machine, not by the repository"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlinked_file_is_refused_so_it_cannot_reach_outside_the_checkout() {
        let (tmp, reg, repo) = repo_with(None).await;
        let outside = tmp.path().join("outside.json");
        std::fs::write(&outside, r##"{"name":"Reached outside"}"##).unwrap();
        std::os::unix::fs::symlink(&outside, tmp.path().join("acme-platform/repo.json")).unwrap();

        let out = reg.apply_repo_json(repo).await.unwrap();
        assert_eq!(
            out.label, None,
            "a symlink is not the repository's own file"
        );
    }

    #[tokio::test]
    async fn an_unreadable_file_is_not_an_error() {
        let (_t, reg, repo) = repo_with(Some("{ this is not json")).await;
        let out = reg.apply_repo_json(repo).await.unwrap();
        assert_eq!(out.label, None);
        assert_eq!(out.accent, None);
    }

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
                    // Normalize so the assertions below hold on Windows too.
                    .replace('\\', "/")
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

        assert!(discover_walk(root, 2).is_empty());
        assert_eq!(discover_walk(root, 5).len(), 1);
    }
}
