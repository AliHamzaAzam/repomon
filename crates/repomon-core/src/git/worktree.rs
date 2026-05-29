//! Worktree enumeration and CRUD.
//!
//! gix can't enumerate or create worktrees yet, so we shell out to `git worktree
//! list --porcelain` (a stable format we parse) and `git worktree add/remove/prune`
//! (so we get exact git semantics rather than a reimplementation).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};

/// One worktree as reported by `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    /// `None` for an unborn HEAD (all-zero oid).
    pub head: Option<gix::ObjectId>,
    /// Short branch name (`refs/heads/` stripped); `None` when detached.
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    /// `Some(reason)` (reason may be empty) when the worktree is locked.
    pub locked: Option<String>,
    /// `Some(reason)` (reason may be empty) when the worktree is prunable.
    pub prunable: Option<String>,
}

/// List all worktrees of the repo whose main checkout lives at `repo_path`.
pub fn list(repo_path: &Path) -> Result<Vec<WorktreeEntry>> {
    let out = run(repo_path, &["worktree", "list", "--porcelain"])?;
    Ok(parse_porcelain(&out))
}

/// Parse the stable `git worktree list --porcelain` format.
pub fn parse_porcelain(text: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut cur: Option<WorktreeEntry> = None;
    for line in text.lines() {
        if line.is_empty() {
            if let Some(e) = cur.take() {
                entries.push(e);
            }
            continue;
        }
        let (key, rest) = match line.split_once(' ') {
            Some((k, r)) => (k, Some(r)),
            None => (line, None),
        };
        match key {
            "worktree" => {
                if let Some(e) = cur.take() {
                    entries.push(e);
                }
                cur = Some(WorktreeEntry {
                    path: PathBuf::from(rest.unwrap_or_default()),
                    head: None,
                    branch: None,
                    detached: false,
                    bare: false,
                    locked: None,
                    prunable: None,
                });
            }
            "HEAD" => {
                if let Some(e) = cur.as_mut() {
                    let h = rest.unwrap_or_default();
                    if !h.is_empty() && !h.bytes().all(|b| b == b'0') {
                        e.head = h.parse().ok();
                    }
                }
            }
            "branch" => {
                if let Some(e) = cur.as_mut() {
                    e.branch = rest.map(strip_refs_heads);
                }
            }
            "detached" => {
                if let Some(e) = cur.as_mut() {
                    e.detached = true;
                }
            }
            "bare" => {
                if let Some(e) = cur.as_mut() {
                    e.bare = true;
                }
            }
            "locked" => {
                if let Some(e) = cur.as_mut() {
                    e.locked = Some(rest.unwrap_or_default().to_string());
                }
            }
            "prunable" => {
                if let Some(e) = cur.as_mut() {
                    e.prunable = Some(rest.unwrap_or_default().to_string());
                }
            }
            _ => {}
        }
    }
    if let Some(e) = cur.take() {
        entries.push(e);
    }
    entries
}

fn strip_refs_heads(r: &str) -> String {
    r.strip_prefix("refs/heads/").unwrap_or(r).to_string()
}

/// Add a worktree. With `create_branch`, runs `worktree add -b <branch> <path> [source]`;
/// otherwise `worktree add <path> <branch>` to check out an existing branch.
pub fn add(
    repo_path: &Path,
    new_path: &Path,
    branch: &str,
    source: Option<&str>,
    create_branch: bool,
) -> Result<()> {
    let new = new_path.to_string_lossy();
    let mut args: Vec<&str> = vec!["worktree", "add"];
    if create_branch {
        args.push("-b");
        args.push(branch);
        args.push(&new);
        if let Some(s) = source {
            args.push(s);
        }
    } else {
        args.push(&new);
        args.push(branch);
    }
    run(repo_path, &args)?;
    Ok(())
}

/// Remove a worktree (`--force` for dirty/locked checkouts when asked).
pub fn remove(repo_path: &Path, worktree_path: &Path, force: bool) -> Result<()> {
    let p = worktree_path.to_string_lossy();
    let mut args: Vec<&str> = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&p);
    run(repo_path, &args)?;
    Ok(())
}

/// Prune stale worktree administrative entries.
pub fn prune(repo_path: &Path) -> Result<()> {
    run(repo_path, &["worktree", "prune"])?;
    Ok(())
}

fn run(repo_path: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(args)
        .output()
        .map_err(Error::Io)?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multi_worktree_porcelain() {
        let text = "\
worktree /code/pos-saas
HEAD a3f7c1d00000000000000000000000000000000a
branch refs/heads/main

worktree /code/pos-saas-wt/checkout
HEAD b2e8d4f00000000000000000000000000000000b
branch refs/heads/hotfix/checkout-bug
locked some reason

worktree /code/pos-saas-wt/detached
HEAD c1d9e5a00000000000000000000000000000000c
detached
prunable gitdir file points to non-existent location

worktree /code/bare-repo
bare
";
        let e = parse_porcelain(text);
        assert_eq!(e.len(), 4);

        assert_eq!(e[0].path, PathBuf::from("/code/pos-saas"));
        assert_eq!(e[0].branch.as_deref(), Some("main"));
        assert!(!e[0].detached);
        assert!(e[0].head.is_some());

        // Slashes in branch names are preserved.
        assert_eq!(e[1].branch.as_deref(), Some("hotfix/checkout-bug"));
        assert_eq!(e[1].locked.as_deref(), Some("some reason"));

        assert!(e[2].detached);
        assert!(e[2].branch.is_none());
        assert!(e[2].prunable.is_some());

        assert!(e[3].bare);
        assert!(e[3].head.is_none());
    }

    #[test]
    fn handles_unborn_head_zeros() {
        let text = "worktree /code/new\nHEAD 0000000000000000000000000000000000000000\nbranch refs/heads/main\n";
        let e = parse_porcelain(text);
        assert_eq!(e.len(), 1);
        assert!(e[0].head.is_none());
        assert_eq!(e[0].branch.as_deref(), Some("main"));
    }
}
