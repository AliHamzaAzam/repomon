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
    ///
    /// **The two halves therefore converge differently, and deliberately.** A declared colour keeps
    /// following the file. A declared name is a SEED: once adopted it is an ordinary label, and
    /// `Repo` has no record of where a label came from, so a later change or removal in `repo.json`
    /// does not move it. Making the name follow the file too would need that provenance — a second
    /// stored value, and a decision about what a local rename then means — which is a product call
    /// rather than a detail of reading the file.
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

        // `repo` is a snapshot taken before the file was read off-thread, so it is only used to
        // decide what to ATTEMPT. Whether a write is allowed is decided by the write itself.
        let mut wrote = false;

        // A declared name equal to the folder name is not an override: writing it would leave a
        // stale label behind the moment the folder is renamed, for no visible difference.
        //
        // The emptiness check belongs to the write, not to the snapshot: a rename landing during
        // the read would otherwise be overwritten by a seed that believed there was no label.
        if let Some(name) = declared.name.filter(|n| *n != repo.name) {
            self.store.seed_repo_label(repo.id, name).await?;
            wrote = true;
        }
        if declared.accent != repo.accent {
            self.store.set_repo_accent(repo.id, declared.accent).await?;
            wrote = true;
        }

        // Re-read after ATTEMPTING anything, not after landing it: a refused seed means something
        // else changed the row, and returning the snapshot would hand back the value it replaced.
        if wrote {
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
/// **What is permitted**, rather than a list of what is refused: a `repo.json` is used only when
/// the final path component passes [`open_regular_file`] — on Unix that means it opens *without
/// following a link*, and on other platforms the weaker check documented there — and the OPEN
/// HANDLE is a regular file no larger than [`repo_json::MAX_BYTES`] holding UTF-8 that parses as
/// an object. Everything else — a symlink, a FIFO, a device, a directory, a socket, an oversized
/// or unreadable or malformed file — is [`repo_json::Declaration::Unusable`], without enumerating
/// them. The checks are tied
/// to the handle because a pathname checked and then opened can be swapped in between, and `open`
/// on a FIFO blocks before any read cap could apply.
///
/// **What is trusted, and why.** Only the final component is treated as hostile. The ancestry of
/// `repo.path` is not re-validated: it is canonicalized when the repo is registered, and an
/// attacker who can swap a parent directory already controls the checkout — they can put whatever
/// they like in the real `repo.json`, so reading a different file's name and colour buys them
/// nothing. `O_NOFOLLOW` covers the final component only, and that is the component this boundary
/// claims.
fn read_declaration(path: &Path) -> repo_json::Declaration {
    use std::io::Read;

    let Some(file) = open_regular_file(path) else {
        // No file, but only when the checkout itself is there: an unreachable directory — an
        // unmounted volume, a disconnected share — knows nothing and must not clear what is stored.
        let absent = matches!(std::fs::symlink_metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound);
        let checkout_present = path.parent().is_some_and(|d| d.is_dir());
        return if absent && checkout_present {
            repo_json::Declaration::None
        } else {
            repo_json::Declaration::Unusable
        };
    };

    // Size is taken from the handle, so it describes the file that was actually opened.
    match file.metadata() {
        Ok(m) if m.is_file() && m.len() <= repo_json::MAX_BYTES => {}
        _ => return repo_json::Declaration::Unusable,
    }

    let mut text = String::new();
    // The cap is applied again: a file may grow between the metadata call and the read.
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

/// Open `path` only if it is a regular file, refusing to follow a symlink and refusing to block.
///
/// `O_NOFOLLOW` makes the kernel reject a symlink at open time, and `O_NONBLOCK` means a FIFO
/// swapped in at the last moment returns instead of parking a thread from the blocking pool.
#[cfg(unix)]
fn open_regular_file(path: &Path) -> Option<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()
}

/// Windows has no `O_NOFOLLOW`, so the link check is made before the open rather than by it.
///
/// This is weaker than the Unix path and the difference is deliberate: a reparse point swapped in
/// between this check and the open would still be followed. Closing that would mean Win32 open
/// flags this crate does not currently pull in, written and shipped without a Windows machine to
/// test them on — a guarantee nobody has run is worse than a stated limit.
#[cfg(not(unix))]
fn open_regular_file(path: &Path) -> Option<std::fs::File> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    std::fs::File::open(path).ok()
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
    async fn a_declared_name_is_a_seed_and_does_not_follow_later_edits() {
        let (tmp, reg, repo) = repo_with(Some(r##"{"name":"Acme Platform"}"##)).await;
        let repo = reg.apply_repo_json(repo).await.unwrap();
        assert_eq!(repo.label.as_deref(), Some("Acme Platform"));

        // Once adopted the label is indistinguishable from a local rename, so the file cannot
        // move it. This pins the current behaviour; changing it needs label provenance.
        std::fs::write(
            tmp.path().join("acme-platform/repo.json"),
            r##"{"name":"Billing"}"##,
        )
        .unwrap();
        let out = reg.apply_repo_json(repo.clone()).await.unwrap();
        assert_eq!(
            out.label.as_deref(),
            Some("Acme Platform"),
            "a rename would have to win too"
        );

        std::fs::write(tmp.path().join("acme-platform/repo.json"), "{}").unwrap();
        let out = reg.apply_repo_json(out).await.unwrap();
        assert_eq!(
            out.label.as_deref(),
            Some("Acme Platform"),
            "and removal cannot clear it"
        );
    }

    #[tokio::test]
    async fn a_rename_landing_during_the_file_read_is_not_overwritten() {
        let (_t, reg, repo) = repo_with(Some(r##"{"name":"Acme Platform"}"##)).await;

        // `repo` is the snapshot apply_repo_json would carry across the off-thread read. The
        // rename happens after it was taken, which is the window the seed must not reopen.
        reg.store
            .set_repo_label(repo.id, Some("Renamed mid-read".into()))
            .await
            .unwrap();

        let out = reg.apply_repo_json(repo).await.unwrap();
        assert_eq!(out.label.as_deref(), Some("Renamed mid-read"));
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

    #[cfg(unix)]
    #[tokio::test]
    async fn a_fifo_is_refused_without_parking_a_blocking_thread() {
        use std::ffi::CString;
        let (tmp, reg, repo) = repo_with(None).await;
        let path = tmp.path().join("acme-platform/repo.json");
        let c = CString::new(path.to_str().unwrap()).unwrap();
        // SAFETY: a path inside this test's own tempdir, valid for the duration of the call.
        assert_eq!(
            unsafe { libc::mkfifo(c.as_ptr(), 0o600) },
            0,
            "mkfifo failed"
        );

        // No writer will ever open the other end. Without O_NONBLOCK this call never returns.
        let out =
            tokio::time::timeout(std::time::Duration::from_secs(5), reg.apply_repo_json(repo))
                .await
                .expect("open blocked on a FIFO with no writer")
                .unwrap();
        assert_eq!(out.label, None);
        assert_eq!(out.accent, None);
    }

    #[tokio::test]
    async fn an_unreachable_checkout_leaves_what_is_stored_alone() {
        let (tmp, reg, repo) = repo_with(Some(r##"{"color":"#0f766e"}"##)).await;
        let repo = reg.apply_repo_json(repo).await.unwrap();
        assert_eq!(repo.accent, Some(7));

        // The whole checkout is gone, not just the file — an unmounted volume looks like this.
        std::fs::remove_dir_all(tmp.path().join("acme-platform")).unwrap();
        let out = reg.apply_repo_json(repo).await.unwrap();
        assert_eq!(
            out.accent,
            Some(7),
            "an unreachable checkout declares nothing new"
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
