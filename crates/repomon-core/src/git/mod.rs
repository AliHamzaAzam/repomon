//! The git layer: gix-backed reads ([`reader`]), worktree CRUD ([`worktree`]), and lane-vs-base
//! diffing ([`diff`]).

use std::path::PathBuf;

pub mod diff;
pub mod reader;
pub mod worktree;

pub use diff::{LaneDiff, diff_patch, lane_diff};
pub use reader::{
    HeadInfo, ahead_behind, commits_in_range, dirty_state, head_info, open, read_commits_in_range,
    read_state,
};
pub use worktree::{WorktreeEntry, parse_porcelain};

/// The two locations the Git for Windows installer offers by default. A process started by the
/// app before a re-login can still have the machine PATH from before the installer updated it,
/// so `find_git` falls back to these once PATH search comes up empty.
pub const WINDOWS_STANDARD_GIT_DIRS: [&str; 2] = [
    r"C:\Program Files\Git\cmd",
    r"C:\Program Files (x86)\Git\cmd",
];

/// Pure `git.exe` resolution: PATH first, then (when given) a list of standard install
/// directories to check for `git.exe` directly. Takes the PATH value and candidate directories
/// as parameters, so the Windows fallback is unit-testable on every OS without touching real
/// environment state.
pub fn find_git_from(path_var: Option<&std::ffi::OsStr>, standard_dirs: &[PathBuf]) -> Option<PathBuf> {
    let on_path = match path_var {
        Some(p) => crate::exec::find_in(p, "git"),
        None => crate::exec::find_in_path("git"),
    };
    if let Some(p) = on_path {
        return Some(p);
    }
    for dir in standard_dirs {
        let candidate = dir.join("git.exe");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Probe git availability, version, and path. On Windows, falls back to the standard Git for
/// Windows install locations when PATH search misses (see [`find_git_from`]).
pub fn probe() -> crate::model::GitDoctorInfo {
    let standard_dirs: Vec<PathBuf> =
        if crate::model::DoctorPlatform::current() == crate::model::DoctorPlatform::Windows {
            WINDOWS_STANDARD_GIT_DIRS.iter().map(PathBuf::from).collect()
        } else {
            Vec::new()
        };
    let path = find_git_from(std::env::var_os("PATH").as_deref(), &standard_dirs);
    match path {
        Some(p) => match std::process::Command::new(&p).arg("--version").output() {
            Ok(out) if out.status.success() => {
                let version_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
                crate::model::GitDoctorInfo {
                    available: true,
                    version: if version_str.is_empty() {
                        None
                    } else {
                        Some(version_str)
                    },
                    path: Some(p.to_string_lossy().into_owned()),
                }
            }
            _ => crate::model::GitDoctorInfo {
                available: false,
                version: None,
                path: Some(p.to_string_lossy().into_owned()),
            },
        },
        None => crate::model::GitDoctorInfo {
            available: false,
            version: None,
            path: None,
        },
    }
}

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
        assert!(
            after
                .iter()
                .any(|w| w.branch.as_deref() == Some("feat/thing"))
        );

        worktree::remove(p, &wt_path, false).unwrap();
        assert_eq!(worktree::list(p).unwrap().len(), 1);
    }

    #[test]
    fn probe_reports_git_health() {
        let probe = super::probe();
        assert!(probe.available);
        assert!(probe.version.is_some());
        assert!(probe.path.is_some());
        let version = probe.version.unwrap();
        assert!(version.starts_with("git version") || version.contains("git"));
    }

    #[test]
    fn find_git_from_prefers_path_over_standard_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let on_path = dir.path().join("git");
        std::fs::write(&on_path, b"fake").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&on_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path_var = std::env::join_paths([dir.path()]).unwrap();

        let standard_dir = dir.path().join("standard");
        std::fs::create_dir_all(&standard_dir).unwrap();
        std::fs::write(standard_dir.join("git.exe"), b"fake-standard").unwrap();

        let found = find_git_from(Some(path_var.as_os_str()), &[standard_dir]);
        assert_eq!(found, Some(on_path));
    }

    #[test]
    fn find_git_from_falls_back_to_standard_windows_install_dirs() {
        // The Git for Windows scenario: not (yet) on PATH, but installed at a standard location -
        // `C:\Program Files\Git\cmd` in production, a temp stand-in here.
        let dir = tempfile::tempdir().unwrap();
        let standard_dir = dir.path().join("Git").join("cmd");
        std::fs::create_dir_all(&standard_dir).unwrap();
        let git_exe = standard_dir.join("git.exe");
        std::fs::write(&git_exe, b"fake-git-for-windows").unwrap();

        let empty_path = std::ffi::OsStr::new("");
        let found = find_git_from(Some(empty_path), &[standard_dir]);
        assert_eq!(found, Some(git_exe));
    }

    #[test]
    fn find_git_from_returns_none_when_nowhere_to_find_it() {
        let dir = tempfile::tempdir().unwrap();
        let empty_standard_dir = dir.path().join("nothing-here");
        std::fs::create_dir_all(&empty_standard_dir).unwrap();

        let empty_path = std::ffi::OsStr::new("");
        let found = find_git_from(Some(empty_path), &[empty_standard_dir]);
        assert_eq!(found, None);
    }
}
