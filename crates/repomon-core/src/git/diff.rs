//! Lane diff: what a lane's branch has produced vs the repo's base branch, plus its uncommitted
//! state. Shells out to `git log`/`git diff`/`git merge-base` — precedent is [`super::worktree`],
//! which shells out because gix lacks ergonomic coverage for these too. Read-only; used by the
//! daemon's `lane.diff` RPC to give the orchestrator git visibility before it trusts a worker's
//! "done" claim.

use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};

/// Commit log lines are capped this low; a lane with more than this many commits ahead still
/// reports a useful headline without shipping an unbounded log to the caller.
const COMMITS_LINE_CAP: usize = 20;

/// A lane's branch compared against the repo's base branch, plus its own uncommitted state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneDiff {
    /// The base branch name the lane was compared against.
    pub base: String,
    /// Short hash of `git merge-base HEAD <base>`.
    pub merge_base: String,
    /// `git log --oneline <merge_base>..HEAD`, capped at [`COMMITS_LINE_CAP`] lines.
    pub commits: String,
    /// Whether `commits` was cut short of the full log.
    pub commits_truncated: bool,
    /// `git diff --stat <merge_base>..HEAD` — committed work vs the base branch.
    pub committed_stat: String,
    /// `git diff HEAD --stat` — staged + unstaged changes.
    pub uncommitted_stat: String,
}

/// Compute `worktree_path`'s [`LaneDiff`] against `base` (a branch name resolvable from the
/// worktree — e.g. the repo main checkout's current branch).
pub fn lane_diff(worktree_path: &Path, base: &str) -> Result<LaneDiff> {
    let merge_base_full = run(worktree_path, &["merge-base", "HEAD", base])
        .map_err(|e| Error::Git(format!("no common ancestor between HEAD and '{base}': {e}")))?
        .trim()
        .to_string();
    let merge_base = run(worktree_path, &["rev-parse", "--short", &merge_base_full])?
        .trim()
        .to_string();

    let range = format!("{merge_base_full}..HEAD");
    // Ask for one more than the cap so we can tell whether the log was truncated without
    // fetching a potentially huge history.
    let log_limit = format!("-{}", COMMITS_LINE_CAP + 1);
    let commits_raw = run(worktree_path, &["log", "--oneline", &log_limit, &range])?;
    let lines: Vec<&str> = commits_raw.lines().collect();
    let commits_truncated = lines.len() > COMMITS_LINE_CAP;
    let commits = lines[..lines.len().min(COMMITS_LINE_CAP)].join("\n");

    let committed_stat = run(worktree_path, &["diff", "--stat", &range])?;
    let uncommitted_stat = run(worktree_path, &["diff", "HEAD", "--stat"])?;

    Ok(LaneDiff {
        base: base.to_string(),
        merge_base,
        commits,
        commits_truncated,
        committed_stat,
        uncommitted_stat,
    })
}

/// `git diff HEAD` (staged + unstaged) — the actual patch text for `include_patch`. Capping to a
/// caller-supplied character limit is the caller's responsibility.
pub fn diff_patch(worktree_path: &Path) -> Result<String> {
    run(worktree_path, &["diff", "HEAD"])
}

fn run(worktree_path: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(worktree_path)
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
    use std::process::Command as StdCommand;

    fn git(dir: &Path, args: &[&str]) {
        let ok = StdCommand::new("git")
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

    /// A base repo on `main` with one commit, plus a `feat` worktree branched from it.
    fn repo_with_lane_worktree() -> (tempfile::TempDir, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-b", "main"]);
        std::fs::write(p.join("README.md"), "hello\n").unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-m", "init"]);

        let wt_parent = tempfile::tempdir().unwrap();
        let wt_path = wt_parent.path().join("feat");
        git(
            p,
            &[
                "worktree",
                "add",
                "-b",
                "feat/thing",
                wt_path.to_str().unwrap(),
            ],
        );
        (dir, wt_parent)
    }

    #[test]
    fn reports_one_commit_ahead_and_uncommitted_file() {
        let (dir, wt_parent) = repo_with_lane_worktree();
        let wt_path = wt_parent.path().join("feat");

        // One commit ahead of main.
        std::fs::write(wt_path.join("a.txt"), "a\n").unwrap();
        git(&wt_path, &["add", "a.txt"]);
        git(&wt_path, &["commit", "-m", "feat: add a"]);

        // Plus an uncommitted (unstaged) change.
        std::fs::write(wt_path.join("README.md"), "changed\n").unwrap();

        let d = lane_diff(&wt_path, "main").unwrap();
        assert_eq!(d.base, "main");
        assert!(!d.merge_base.is_empty());
        assert!(
            d.commits.contains("feat: add a"),
            "commits was: {:?}",
            d.commits
        );
        assert!(!d.commits_truncated);
        assert!(
            d.committed_stat.contains("a.txt"),
            "committed_stat was: {:?}",
            d.committed_stat
        );
        assert!(
            d.uncommitted_stat.contains("README.md"),
            "uncommitted_stat was: {:?}",
            d.uncommitted_stat
        );

        let _ = dir; // keep the main repo tempdir alive for the duration of the test
    }

    #[test]
    fn commits_truncated_past_the_cap() {
        let (dir, wt_parent) = repo_with_lane_worktree();
        let wt_path = wt_parent.path().join("feat");

        for i in 0..(COMMITS_LINE_CAP + 3) {
            std::fs::write(wt_path.join(format!("f{i}.txt")), "x\n").unwrap();
            git(&wt_path, &["add", "."]);
            git(&wt_path, &["commit", "-m", &format!("commit {i}")]);
        }

        let d = lane_diff(&wt_path, "main").unwrap();
        assert!(d.commits_truncated);
        assert_eq!(d.commits.lines().count(), COMMITS_LINE_CAP);

        let _ = dir;
    }

    #[test]
    fn errors_when_base_branch_has_no_common_ancestor() {
        let (dir, wt_parent) = repo_with_lane_worktree();
        let wt_path = wt_parent.path().join("feat");

        // An unrelated branch (no shared history) makes merge-base fail.
        git(&wt_path, &["checkout", "--orphan", "orphan-branch"]);
        std::fs::write(wt_path.join("only.txt"), "x\n").unwrap();
        git(&wt_path, &["add", "."]);
        git(&wt_path, &["commit", "-m", "orphan commit"]);

        let err = lane_diff(&wt_path, "main").unwrap_err();
        assert!(
            err.to_string().contains("no common ancestor"),
            "error was: {err}"
        );

        let _ = dir;
    }

    #[test]
    fn diff_patch_returns_uncommitted_text() {
        let (dir, wt_parent) = repo_with_lane_worktree();
        let wt_path = wt_parent.path().join("feat");

        std::fs::write(wt_path.join("README.md"), "patched\n").unwrap();
        let patch = diff_patch(&wt_path).unwrap();
        assert!(patch.contains("patched"), "patch was: {patch:?}");

        let _ = dir;
    }
}
