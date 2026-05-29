//! gix-backed git reads: HEAD, branch, dirty state, ahead/behind, and commit walking.
//!
//! Everything here is synchronous and `!Sync`-friendly: the daemon calls these from
//! `spawn_blocking`, opening the repository per call. We never shell out for status/log/diff
//! — those go through gix. (Worktree CRUD is the one shellout, in [`super::worktree`].)

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
    use gix::status::index_worktree::Item as IwItem;
    use gix::status::Item;

    let platform = repo.status(gix::progress::Discard).map_err(gix_err)?;
    let iter = platform
        .into_iter(Vec::<gix::bstr::BString>::new())
        .map_err(gix_err)?;

    let mut d = DirtyState::default();
    for item in iter {
        match item.map_err(gix_err)? {
            // HEAD tree vs index → staged.
            Item::TreeIndex(_) => d.staged += 1,
            Item::IndexWorktree(iw) => match iw {
                IwItem::Modification { .. } => d.unstaged += 1,
                // Default status() excludes ignored files, so directory contents are untracked.
                IwItem::DirectoryContents { .. } => d.untracked += 1,
                IwItem::Rewrite { .. } => d.unstaged += 1,
            },
        }
    }
    Ok(d)
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
    let dirty = dirty_state(&repo)?;
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
