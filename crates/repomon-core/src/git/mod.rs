//! The git layer: gix-backed reads ([`reader`]) and worktree CRUD ([`worktree`]).

pub mod reader;
pub mod worktree;

pub use reader::{
    ahead_behind, commits_in_range, dirty_state, head_info, open, read_commits_in_range,
    read_state, HeadInfo,
};
pub use worktree::{parse_porcelain, WorktreeEntry};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("run git");
        assert!(
            status.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&status.stderr)
        );
    }

    /// A temp repo on branch `main` with one commit adding `README.md`.
    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-b", "main"]);
        std::fs::write(p.join("README.md"), "hello\n").unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-m", "feat: initial commit"]);
        dir
    }

    #[test]
    fn reads_head_branch_and_clean_state() {
        let dir = init_repo();
        let state = read_state(dir.path(), 1).unwrap();
        assert_eq!(state.branch.as_deref(), Some("main"));
        assert!(state.dirty.is_clean(), "fresh commit should be clean");
        assert_eq!((state.ahead, state.behind), (0, 0));
        assert!(state.last_commit_at.is_some());
        assert!(state.upstream.is_none());
    }

    #[test]
    fn detects_untracked_and_staged() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("new.txt"), "x\n").unwrap();
        let state = read_state(p, 1).unwrap();
        assert_eq!(state.dirty.untracked, 1, "one untracked file");
        assert_eq!(state.dirty.staged, 0);

        git(p, &["add", "new.txt"]);
        let state = read_state(p, 1).unwrap();
        assert_eq!(state.dirty.staged, 1, "after add, one staged change");
        assert_eq!(state.dirty.untracked, 0);
    }

    #[test]
    fn walks_commits_in_range() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "a\n").unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-m", "feat: add a"]);

        let now = chrono::Utc::now();
        let range = crate::model::TimeRange {
            from: now - chrono::Duration::hours(1),
            to: now + chrono::Duration::hours(1),
        };
        let commits = read_commits_in_range(p, 7, range).unwrap();
        assert_eq!(commits.len(), 2);
        // Newest first.
        assert_eq!(commits[0].summary, "feat: add a");
        assert_eq!(commits[1].summary, "feat: initial commit");
        assert_eq!(commits[0].repo_id, 7);
        assert_eq!(commits[0].author_name, "Test");
        assert_eq!(commits[1].parent_count, 0, "root commit has no parents");
    }

    #[test]
    fn lists_and_adds_worktrees() {
        let dir = init_repo();
        let p = dir.path();
        let initial = worktree::list(p).unwrap();
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].branch.as_deref(), Some("main"));

        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("feat");
        worktree::add(p, &wt_path, "feat/thing", Some("main"), true).unwrap();

        let after = worktree::list(p).unwrap();
        assert_eq!(after.len(), 2);
        assert!(after
            .iter()
            .any(|w| w.branch.as_deref() == Some("feat/thing")));

        worktree::remove(p, &wt_path, false).unwrap();
        assert_eq!(worktree::list(p).unwrap().len(), 1);
    }
}
