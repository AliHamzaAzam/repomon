//! Lane diff: what a lane's branch has produced vs the repo's base branch, plus its uncommitted
//! state. Shells out to `git log`/`git diff`/`git merge-base` — precedent is [`super::worktree`],
//! which shells out because gix lacks ergonomic coverage for these too. Read-only; used by the
//! daemon's `lane.diff` RPC to give the orchestrator git visibility before it trusts a worker's
//! "done" claim.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::process::background_command;

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
    /// Count of untracked files (`git ls-files --others --exclude-standard`), computed live
    /// with the stats above — never a cached scan, so one snapshot is self-consistent.
    pub untracked: usize,
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
    let untracked = run(
        worktree_path,
        &["ls-files", "--others", "--exclude-standard"],
    )?
    .lines()
    .filter(|l| !l.is_empty())
    .count();

    Ok(LaneDiff {
        base: base.to_string(),
        merge_base,
        commits,
        commits_truncated,
        committed_stat,
        uncommitted_stat,
        untracked,
    })
}

/// `git diff HEAD` (staged + unstaged) — the actual patch text for `include_patch`. Capping to a
/// caller-supplied character limit is the caller's responsibility.
pub fn diff_patch(worktree_path: &Path) -> Result<String> {
    run(worktree_path, &["diff", "HEAD"])
}

/// One commit's full detail, for the `commit.show` RPC: everything a commit-detail view needs to
/// render without a second round trip. `patch`/`stat` mirror `LaneDiff`'s equivalent fields
/// (unified diff text and `git diff --stat` text respectively); capping `patch` to a
/// caller-supplied character limit is the caller's responsibility, same as `diff_patch` above.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct CommitShow {
    /// The full 40-char oid, resolved from whatever the caller passed in (which may have been
    /// abbreviated) — never the caller's own possibly-short string, so a client always has a
    /// stable, unambiguous id to key off of.
    pub oid: String,
    pub author_name: String,
    pub author_email: String,
    pub time: DateTime<Utc>,
    /// First line of the commit message.
    pub summary: String,
    /// Everything after the summary line (empty string for a subject-only commit message).
    pub body: String,
    pub patch: String,
    pub stat: String,
    /// Always `false` from [`commit_show`] itself — truncation happens one layer up, in the
    /// `commit.show` RPC handler, the same `cap_chars` server-side cap `lane.diff`'s patch uses.
    /// Carried as a field here (rather than a separate bool the handler bolts onto a hand-built
    /// JSON object, as `lane.diff` does) so `CommitShow`'s ts-rs binding is the RPC's actual wire
    /// shape and the frontend gets a real, always-present `boolean` instead of an optional one.
    pub patch_truncated: bool,
}

/// A field separator that can't plausibly appear in a commit's author name/email/subject —
/// `git log`/`git show` `--format` output is otherwise plain text, so splitting on this is safe
/// (unlike, say, a comma or pipe, which real commit metadata could legitimately contain).
const FIELD_SEP: &str = "\u{1f}";

/// True for a string `git rev-parse` would accept as an abbreviated-or-full hex object id: only
/// hex digits, no `-` (so a caller-supplied oid can never be mistaken for a git flag — every git
/// flag starts with `-`, which is never a hex digit), and a length in git's actual abbreviation
/// range (a `git show`/`rev-parse` short hash is never shorter than 4 hex digits, never longer
/// than a 40-char sha1 — this crate doesn't yet support sha256 repos).
fn looks_like_oid(oid: &str) -> bool {
    (4..=40).contains(&oid.len()) && oid.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `git show <oid>`, split into structured fields for the `commit.show` RPC. Shells out via the
/// same `run()` helper (and the same arg-array-not-shell-string discipline) as every other
/// function in this module.
///
/// `oid` is validated and resolved before anything else runs:
/// 1. [`looks_like_oid`] rejects anything that isn't plain hex up front — defense in depth against
///    an oid-shaped argument being misread as a flag, even though arg-array invocation already
///    makes shell injection impossible.
/// 2. `git rev-parse --verify <oid>^{commit}` confirms the oid both resolves *within this lane's
///    repo* and names a commit (not a blob/tree/tag) before any other command runs, and yields the
///    full oid every other command below then uses — so a short, ambiguous, or otherwise
///    unverified id is never passed to `git show` directly.
pub fn commit_show(worktree_path: &Path, oid: &str) -> Result<CommitShow> {
    if !looks_like_oid(oid) {
        return Err(Error::Git(format!("not a valid commit id: {oid:?}")));
    }
    let full = run(
        worktree_path,
        &["rev-parse", "--verify", &format!("{oid}^{{commit}}")],
    )
    .map_err(|e| Error::Git(format!("{oid} does not resolve to a commit in this repo: {e}")))?
    .trim()
    .to_string();

    let format = format!("--format=%an{FIELD_SEP}%ae{FIELD_SEP}%at{FIELD_SEP}%s{FIELD_SEP}%b");
    let meta = run(worktree_path, &["show", "--no-patch", &format, &full])?;
    let mut parts = meta.splitn(5, FIELD_SEP);
    let author_name = parts.next().unwrap_or_default().to_string();
    let author_email = parts.next().unwrap_or_default().to_string();
    let epoch_field = parts.next().unwrap_or_default().trim();
    let summary = parts.next().unwrap_or_default().to_string();
    // %b's trailing newline (git always emits one) isn't part of the message body.
    let body = parts
        .next()
        .unwrap_or_default()
        .trim_end_matches('\n')
        .to_string();

    let epoch: i64 = epoch_field
        .parse()
        .map_err(|_| Error::Git(format!("git show returned an unparsable author date for {full}: {epoch_field:?}")))?;
    let time = DateTime::<Utc>::from_timestamp(epoch, 0)
        .ok_or_else(|| Error::Git(format!("author date out of range for {full}: {epoch}")))?;

    let stat = run(worktree_path, &["show", "--stat", "--format=", &full])?
        .trim()
        .to_string();
    let patch = run(worktree_path, &["show", "--patch", "--format=", &full])?;

    Ok(CommitShow {
        oid: full,
        author_name,
        author_email,
        time,
        summary,
        body,
        patch,
        stat,
        patch_truncated: false,
    })
}

fn run(worktree_path: &Path, args: &[&str]) -> Result<String> {
    let out = background_command("git")
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
        assert_eq!(d.untracked, 0); // a tracked-file edit is not an untracked file

        let _ = dir; // keep the main repo tempdir alive for the duration of the test
    }

    #[test]
    fn counts_untracked_files_live() {
        let (dir, wt_parent) = repo_with_lane_worktree();
        let wt_path = wt_parent.path().join("feat");

        std::fs::write(wt_path.join("scratch.txt"), "x\n").unwrap();
        std::fs::write(wt_path.join("notes.txt"), "y\n").unwrap();

        let d = lane_diff(&wt_path, "main").unwrap();
        assert_eq!(d.untracked, 2);

        // Ignored files don't count (--exclude-standard honors .gitignore).
        std::fs::write(wt_path.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(wt_path.join("ignored.txt"), "z\n").unwrap();
        let d = lane_diff(&wt_path, "main").unwrap();
        assert_eq!(d.untracked, 3); // scratch, notes, and the new .gitignore itself

        let _ = dir;
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

    #[test]
    fn looks_like_oid_accepts_only_plain_hex_in_gits_abbreviation_range() {
        assert!(looks_like_oid("abc1234"));
        assert!(looks_like_oid(
            "0123456789abcdef0123456789abcdef01234567"
        ));
        assert!(looks_like_oid("ABC1234")); // git accepts uppercase hex too
        assert!(!looks_like_oid("abc")); // shorter than git's 4-char floor
        assert!(!looks_like_oid(
            "0123456789abcdef0123456789abcdef012345678"
        )); // one char past a full sha1
        assert!(!looks_like_oid("not-hex-at-all"));
        // The one case that matters most: nothing starting with `-` (a git flag) ever passes,
        // regardless of what follows.
        assert!(!looks_like_oid("--upload-pack=x"));
        assert!(!looks_like_oid(""));
    }

    #[test]
    fn commit_show_returns_full_metadata_body_and_patch() {
        let (dir, wt_parent) = repo_with_lane_worktree();
        let wt_path = wt_parent.path().join("feat");

        std::fs::write(wt_path.join("a.txt"), "a\n").unwrap();
        git(&wt_path, &["add", "a.txt"]);
        git(
            &wt_path,
            &["commit", "-m", "feat: add a\n\nExplains why a was added.\nSecond body line."],
        );
        let short = run(&wt_path, &["rev-parse", "--short", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let full = run(&wt_path, &["rev-parse", "HEAD"]).unwrap().trim().to_string();

        // Resolves from an abbreviated oid, same as a commit row's short hash would supply.
        let show = commit_show(&wt_path, &short).unwrap();
        assert_eq!(show.oid, full, "should resolve to the full oid, not echo the short input");
        assert_eq!(show.author_name, "T");
        assert_eq!(show.author_email, "t@e.com");
        assert_eq!(show.summary, "feat: add a");
        assert!(
            show.body.contains("Explains why a was added.") && show.body.contains("Second body line."),
            "body was: {:?}",
            show.body
        );
        assert!(show.patch.contains("+a"), "patch was: {:?}", show.patch);
        assert!(show.stat.contains("a.txt"), "stat was: {:?}", show.stat);

        let _ = dir;
    }

    #[test]
    fn commit_show_rejects_a_malformed_oid_without_running_git() {
        let (dir, wt_parent) = repo_with_lane_worktree();
        let wt_path = wt_parent.path().join("feat");

        let err = commit_show(&wt_path, "--not-an-oid").unwrap_err();
        assert!(
            err.to_string().contains("not a valid commit id"),
            "error was: {err}"
        );

        let _ = dir;
    }

    #[test]
    fn commit_show_rejects_an_oid_that_does_not_resolve_in_this_repo() {
        let (dir, wt_parent) = repo_with_lane_worktree();
        let wt_path = wt_parent.path().join("feat");

        // Well-formed hex, but no such commit exists in this repo.
        let err = commit_show(&wt_path, "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef").unwrap_err();
        assert!(
            err.to_string().contains("does not resolve to a commit"),
            "error was: {err}"
        );

        let _ = dir;
    }

    #[test]
    fn commit_show_rejects_an_oid_that_names_a_non_commit_object() {
        let (dir, wt_parent) = repo_with_lane_worktree();
        let wt_path = wt_parent.path().join("feat");

        // A blob oid (the README's own content object) is well-formed hex and really exists in
        // this repo, but isn't a commit - the `^{commit}` peel in commit_show must reject it.
        let blob = run(&wt_path, &["rev-parse", "HEAD:README.md"])
            .unwrap()
            .trim()
            .to_string();

        let err = commit_show(&wt_path, &blob).unwrap_err();
        assert!(
            err.to_string().contains("does not resolve to a commit"),
            "error was: {err}"
        );

        let _ = dir;
    }
}
