//! Provides synchronous gix-backed repository reads with a fresh repository handle per call for use
//! on blocking threads.

use std::path::Path;

use chrono::{DateTime, Utc};
use gix::bstr::ByteSlice;

use crate::error::{Error, Result};
use crate::model::{Commit, DirtyState, RepoId, TimeRange, WorktreeId, WorktreeState};

fn gix_err<E: std::fmt::Display>(e: E) -> Error {
    Error::Git(e.to_string())
}

/// Open the repository containing `path` (the worktree root or its `.git`).
pub fn open(path: &Path) -> Result<gix::Repository> {
    gix::open(path).map_err(gix_err)
}

/// HEAD branch (short name), commit id, and detached flag.
#[derive(Debug, Clone)]
pub struct HeadInfo {
    pub branch: Option<String>,
    pub head: Option<gix::ObjectId>,
    pub detached: bool,
}

pub fn head_info(repo: &gix::Repository) -> Result<HeadInfo> {
    let head = repo.head().map_err(gix_err)?;
    let detached = head.is_detached();
    let branch = head.referent_name().map(|n| n.shorten().to_string());
    // `head_id` errors on an unborn HEAD (a branch with no commits yet).
    let head_oid = repo.head_id().ok().map(|id| id.detach());
    Ok(HeadInfo {
        branch,
        head: head_oid,
        detached,
    })
}

/// Count staged / unstaged / untracked changes via the gix status iterator.
pub fn dirty_state(repo: &gix::Repository) -> Result<DirtyState> {
    Ok(dirty_and_activity(repo, None)?.0)
}

/// How many changed worktree files to `stat` for the activity mtime before giving up. Any single
/// recently-touched file already proves activity, so a small cap keeps the syscalls bounded even
/// for a worktree with thousands of changes.
const ACTIVITY_STAT_CAP: u32 = 512;

/// Collect dirty entries and optional file activity in one status walk, including work from agents
/// without transcripts or independent processes.
fn dirty_and_activity(
    repo: &gix::Repository,
    worktree_root: Option<&Path>,
) -> Result<(DirtyState, Option<DateTime<Utc>>)> {
    use gix::status::Item;
    use gix::status::index_worktree::Item as IwItem;

    let platform = repo.status(gix::progress::Discard).map_err(gix_err)?;
    let iter = platform
        .into_iter(Vec::<gix::bstr::BString>::new())
        .map_err(gix_err)?;

    let mut d = DirtyState::default();
    let mut newest: Option<std::time::SystemTime> = None;
    let mut stats = 0u32;
    for item in iter {
        match item.map_err(gix_err)? {
            // HEAD tree vs index → staged.
            Item::TreeIndex(_) => d.staged += 1,
            Item::IndexWorktree(iw) => {
                // Worktree-side changes reflect live file edits; stat the path for its mtime.
                if let Some(root) = worktree_root {
                    if stats < ACTIVITY_STAT_CAP {
                        stats += 1;
                        note_mtime(root, iw.rela_path(), &mut newest);
                    }
                }
                match iw {
                    IwItem::Modification { .. } => d.unstaged += 1,
                    // Default status() excludes ignored files, so directory contents are untracked.
                    IwItem::DirectoryContents { .. } => d.untracked += 1,
                    IwItem::Rewrite { .. } => d.unstaged += 1,
                }
            }
        }
    }
    Ok((d, newest.map(DateTime::<Utc>::from)))
}

/// Update `newest` with the mtime of `root/rela`, if it's newer (best-effort; ignores errors).
fn note_mtime(root: &Path, rela: &gix::bstr::BStr, newest: &mut Option<std::time::SystemTime>) {
    let Ok(rel) = rela.to_path() else {
        return;
    };
    if let Ok(md) = std::fs::symlink_metadata(root.join(rel)) {
        if let Ok(m) = md.modified() {
            if newest.is_none_or(|n| m > n) {
                *newest = Some(m);
            }
        }
    }
}

/// Ahead/behind counts vs the current branch's upstream, plus the upstream's short name.
/// Returns `(0, 0, None)` when there is no upstream (or HEAD is detached/unborn).
pub fn ahead_behind(repo: &gix::Repository) -> Result<(u32, u32, Option<String>)> {
    let head_id = match repo.head_id() {
        Ok(id) => id.detach(),
        Err(_) => return Ok((0, 0, None)),
    };
    let head_name = match repo.head_name().map_err(gix_err)? {
        Some(n) => n,
        None => return Ok((0, 0, None)),
    };
    let tracking = match repo
        .branch_remote_tracking_ref_name(head_name.as_ref(), gix::remote::Direction::Fetch)
    {
        Some(Ok(t)) => t,
        _ => return Ok((0, 0, None)),
    };
    let upstream_name = tracking.shorten().to_string();
    let upstream_id = match repo.find_reference(tracking.as_ref()) {
        Ok(mut r) => match r.peel_to_id() {
            Ok(id) => id.detach(),
            Err(_) => return Ok((0, 0, Some(upstream_name))),
        },
        Err(_) => return Ok((0, 0, Some(upstream_name))),
    };

    let ahead = repo
        .rev_walk([head_id])
        .with_hidden([upstream_id])
        .all()
        .map_err(gix_err)?
        .filter_map(|r| r.ok())
        .count() as u32;
    let behind = repo
        .rev_walk([upstream_id])
        .with_hidden([head_id])
        .all()
        .map_err(gix_err)?
        .filter_map(|r| r.ok())
        .count() as u32;
    Ok((ahead, behind, Some(upstream_name)))
}

/// Branch names tried, in order, as the repository's default branch when `origin/HEAD` does not
/// name one. Local first: a worktree checked out from a repo with no remote still has one.
const DEFAULT_BRANCH_CANDIDATES: [&str; 4] = [
    "refs/remotes/origin/main",
    "refs/remotes/origin/master",
    "refs/heads/main",
    "refs/heads/master",
];

/// Reports whether the worktree HEAD adds no commits beyond the repository’s default branch.
pub fn merged_into_default(repo: &gix::Repository) -> Result<bool> {
    let head_id = match repo.head_id() {
        Ok(id) => id.detach(),
        Err(_) => return Ok(false),
    };
    // A detached head has no branch to call merged, and the default branch is never merged into
    // itself: resolving it against itself would mark every main lane stale.
    let head_ref = match repo.head_name().map_err(gix_err)? {
        Some(n) => n.as_bstr().to_string(),
        None => return Ok(false),
    };
    let default_ref = DEFAULT_BRANCH_CANDIDATES
        .iter()
        .find(|candidate| {
            repo.try_find_reference(**candidate)
                .ok()
                .flatten()
                .is_some()
        })
        .copied();
    let Some(default_ref) = default_ref else {
        return Ok(false);
    };
    // Comparing short names, so a local `main` is not reported as merged into `origin/main`.
    let short = |r: &str| {
        r.trim_start_matches("refs/remotes/origin/")
            .trim_start_matches("refs/heads/")
            .to_string()
    };
    if short(default_ref) == short(&head_ref) {
        return Ok(false);
    }
    let Some(default_id) = repo
        .try_find_reference(default_ref)
        .ok()
        .flatten()
        .and_then(|mut r| r.peel_to_id().ok().map(|id| id.detach()))
    else {
        return Ok(false);
    };
    let unmerged = repo
        .rev_walk([head_id])
        .with_hidden([default_id])
        .all()
        .map_err(gix_err)?
        .filter_map(|r| r.ok())
        .count();
    Ok(unmerged == 0)
}

/// The committer time of HEAD, if any.
pub fn head_commit_time(repo: &gix::Repository) -> Result<Option<DateTime<Utc>>> {
    match repo.head_id() {
        Ok(id) => {
            let commit = repo.find_commit(id.detach()).map_err(gix_err)?;
            let t = commit.time().map_err(gix_err)?;
            Ok(DateTime::from_timestamp(t.seconds, 0))
        }
        Err(_) => Ok(None),
    }
}

/// The full live state of the worktree rooted at `path`.
pub fn read_state(path: &Path, worktree_id: WorktreeId) -> Result<WorktreeState> {
    let repo = open(path)?;
    let hi = head_info(&repo)?;
    let (dirty, last_change_at) = dirty_and_activity(&repo, Some(path))?;
    let (ahead, behind, upstream) = ahead_behind(&repo)?;
    let last_commit_at = head_commit_time(&repo).ok().flatten();
    let head = hi
        .head
        .unwrap_or_else(|| gix::ObjectId::null(gix::hash::Kind::Sha1));
    Ok(WorktreeState {
        worktree_id,
        head,
        branch: hi.branch,
        upstream,
        ahead,
        behind,
        dirty,
        last_commit_at,
        locked: false,
        prunable: false,
        last_change_at,
        merged: merged_into_default(&repo).unwrap_or(false),
    })
}

/// Walk commits reachable from HEAD whose committer time falls in `[from, to)`,
/// newest first.
pub fn commits_in_range(
    repo: &gix::Repository,
    repo_id: RepoId,
    range: TimeRange,
) -> Result<Vec<Commit>> {
    use gix::revision::walk::Sorting;
    use gix::traverse::commit::simple::CommitTimeOrder;

    let head_id = match repo.head_id() {
        Ok(id) => id.detach(),
        Err(_) => return Ok(Vec::new()),
    };
    let from = range.from.timestamp();
    let to = range.to.timestamp();

    let walk = repo
        .rev_walk([head_id])
        .sorting(Sorting::ByCommitTime(CommitTimeOrder::NewestFirst))
        .all()
        .map_err(gix_err)?;

    let mut out = Vec::new();
    for info in walk {
        let info = info.map_err(gix_err)?;
        let t = info.commit_time.unwrap_or(0);
        if t >= to {
            continue;
        }
        if t < from {
            break;
        }
        let commit = repo.find_commit(info.id).map_err(gix_err)?;
        let author = commit.author().map_err(gix_err)?;
        let raw = commit.message_raw().map_err(gix_err)?;
        let summary = raw
            .lines()
            .next()
            .map(|l| l.to_str_lossy().into_owned())
            .unwrap_or_default();
        out.push(Commit {
            oid: info.id,
            repo_id,
            author_name: author.name.to_string(),
            author_email: author.email.to_string(),
            summary,
            time: DateTime::from_timestamp(t, 0).unwrap_or_else(Utc::now),
            parent_count: commit.parent_ids().count() as u32,
        });
    }
    Ok(out)
}

/// Convenience: open `path` and walk commits in `range`.
pub fn read_commits_in_range(
    path: &Path,
    repo_id: RepoId,
    range: TimeRange,
) -> Result<Vec<Commit>> {
    let repo = open(path)?;
    commits_in_range(&repo, repo_id, range)
}

/// Walk HEAD newest-first and return up to `limit` commits, ignoring dates. Used for the
/// "recent commits" panel so a worktree on a feature branch (or a repo with nothing today)
/// still shows its latest history.
pub fn recent_commits(
    repo: &gix::Repository,
    repo_id: RepoId,
    limit: usize,
) -> Result<Vec<Commit>> {
    use gix::revision::walk::Sorting;
    use gix::traverse::commit::simple::CommitTimeOrder;

    let head_id = match repo.head_id() {
        Ok(id) => id.detach(),
        Err(_) => return Ok(Vec::new()),
    };
    let walk = repo
        .rev_walk([head_id])
        .sorting(Sorting::ByCommitTime(CommitTimeOrder::NewestFirst))
        .all()
        .map_err(gix_err)?;

    let mut out = Vec::new();
    for info in walk {
        if out.len() >= limit {
            break;
        }
        let info = info.map_err(gix_err)?;
        let t = info.commit_time.unwrap_or(0);
        let commit = repo.find_commit(info.id).map_err(gix_err)?;
        let author = commit.author().map_err(gix_err)?;
        let raw = commit.message_raw().map_err(gix_err)?;
        let summary = raw
            .lines()
            .next()
            .map(|l| l.to_str_lossy().into_owned())
            .unwrap_or_default();
        out.push(Commit {
            oid: info.id,
            repo_id,
            author_name: author.name.to_string(),
            author_email: author.email.to_string(),
            summary,
            time: DateTime::from_timestamp(t, 0).unwrap_or_else(Utc::now),
            parent_count: commit.parent_ids().count() as u32,
        });
    }
    Ok(out)
}

/// Convenience: open `path` and return its latest `limit` commits.
pub fn read_recent_commits(path: &Path, repo_id: RepoId, limit: usize) -> Result<Vec<Commit>> {
    let repo = open(path)?;
    recent_commits(&repo, repo_id, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A worktree branch is "merged" only once the default branch already contains every one of
    /// its commits. Stale `repomon-feat-*` worktrees pile up in the sidebar precisely because
    /// nothing says so.
    #[test]
    fn merged_reports_only_branches_the_default_branch_already_contains() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .expect("git");
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.txt"), "a").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "first"]);

        // The default branch itself is never "merged": that would mark every main lane stale.
        let main_repo = open(root).unwrap();
        assert!(!merged_into_default(&main_repo).unwrap());

        git(&["checkout", "-q", "-b", "landed"]);
        let landed = open(root).unwrap();
        assert!(merged_into_default(&landed).unwrap());

        std::fs::write(root.join("b.txt"), "b").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "second"]);
        let ahead = open(root).unwrap();
        assert!(!merged_into_default(&ahead).unwrap());
    }
}
