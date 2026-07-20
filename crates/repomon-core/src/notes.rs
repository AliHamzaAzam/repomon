//! Per-repo fleet memory: one human-editable markdown file per registered repo.
//!
//! The daemon owns a `repo-notes/` directory (under its data dir) holding durable per-repo
//! knowledge — conventions, build/test commands, gotchas — that the orchestrator folds into
//! worker prompts. Files are keyed by a sanitized repo name so a human can find and edit
//! `myrepo.md` directly; repo names are not unique, so on a slug collision every collider
//! resolves to `<slug>-<id>.md` instead (never the bare name, so notes can't bleed across
//! repos). Removing a repo leaves its file: knowledge survives re-registration.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::model::Repo;

/// Hard cap on a notes file, enforced on write and (defensively) on read — notes travel
/// inside worker prompts, so an unbounded file would blow up token budgets.
pub const MAX_NOTES_BYTES: usize = 8192;

/// Marker appended when a hand-edited file over the cap is truncated on read.
pub const TRUNCATION_MARKER: &str = "[notes truncated at 8 KB — edit the file to trim]";

/// Reduce a repo name to a filesystem-safe slug: keep `[A-Za-z0-9._-]`, map runs of anything
/// else to `-`, trim leading/trailing `.` and `-` (no dotfiles, no `..`), cap at 64 chars.
/// Empty results fall back to `"repo"`.
fn slug(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            s.push(c);
        } else if !s.ends_with('-') {
            s.push('-');
        }
    }
    let mut s: String = s.trim_matches(['.', '-']).chars().take(64).collect();
    while s.ends_with(['.', '-']) {
        s.pop();
    }
    if s.is_empty() {
        s.push_str("repo");
    }
    s
}

/// The notes file path for `repo`, collision-aware against every registered repo in `all`:
/// a unique slug gets `<slug>.md`; colliding slugs all get `<slug>-<id>.md`.
pub fn notes_path(dir: &Path, repo: &Repo, all: &[Repo]) -> PathBuf {
    let s = slug(&repo.name);
    let collides = all.iter().any(|r| r.id != repo.id && slug(&r.name) == s);
    if collides {
        dir.join(format!("{s}-{}.md", repo.id))
    } else {
        dir.join(format!("{s}.md"))
    }
}

/// Read the repo's notes. `Ok(None)` when no file exists. A hand-edited file larger than
/// [`MAX_NOTES_BYTES`] is truncated at a char boundary with [`TRUNCATION_MARKER`] appended.
pub fn read(dir: &Path, repo: &Repo, all: &[Repo]) -> Result<Option<String>> {
    let path = notes_path(dir, repo, all);
    let mut s = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::Io(e)),
    };
    if s.len() > MAX_NOTES_BYTES {
        let mut end = MAX_NOTES_BYTES;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        s.push('\n');
        s.push_str(TRUNCATION_MARKER);
    }
    Ok(Some(s))
}

/// Replace the repo's notes wholesale, atomically (temp file + fsync + rename, the
/// [`crate::config::Config::save_to`] pattern). Rejects content over [`MAX_NOTES_BYTES`].
/// The previous version, when different, is kept one generation in `<file>.bak`.
/// Returns the path written.
pub fn write(dir: &Path, repo: &Repo, all: &[Repo], content: &str) -> Result<PathBuf> {
    use std::io::Write;
    if content.len() > MAX_NOTES_BYTES {
        return Err(Error::Config(format!(
            "notes are {} bytes; the cap is {MAX_NOTES_BYTES} bytes — trim before writing",
            content.len()
        )));
    }
    std::fs::create_dir_all(dir)?;
    let path = notes_path(dir, repo, all);

    // One-generation backup: keep what we're about to overwrite, unless it's identical.
    match std::fs::read_to_string(&path) {
        Ok(prev) if prev != content => {
            let bak = path.with_file_name(format!(
                "{}.bak",
                path.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("notes.md")
            ));
            std::fs::write(&bak, prev)?;
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::Io(e)),
    }

    // Unique temp name (pid + nanos) beside the target so the rename stays on one filesystem.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let base = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("notes.md");
    let tmp = path.with_file_name(format!(".{base}.{}.{nanos}.tmp", std::process::id()));

    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(content.as_bytes())?;
    f.sync_all()?; // flush data to disk before it becomes the live file
    drop(f);
    std::fs::rename(&tmp, &path)?;
    // Best-effort: fsync the directory so the rename itself survives a crash.
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn repo(id: i64, name: &str) -> Repo {
        Repo {
            id,
            path: PathBuf::from(format!("/tmp/{name}")),
            name: name.to_string(),
            added_at: Utc::now(),
            worktree_root_template: None,
        }
    }

    #[test]
    fn slug_sanitizes_hostile_names() {
        assert_eq!(slug("my repo!"), "my-repo");
        // Path traversal: no separators survive, no leading dot, never empty.
        let s = slug("../../etc");
        assert!(!s.contains('/'));
        assert!(!s.starts_with('.'));
        assert!(!s.is_empty());
        assert_eq!(slug(""), "repo");
        assert_eq!(slug("!!!"), "repo");
        assert!(slug(&"x".repeat(200)).len() <= 64);
    }

    #[test]
    fn unique_name_gets_bare_filename() {
        let api = repo(1, "api");
        let other = repo(2, "web");
        let all = vec![api.clone(), other];
        assert_eq!(
            notes_path(Path::new("/notes"), &api, &all),
            PathBuf::from("/notes/api.md")
        );
    }

    #[test]
    fn colliding_names_both_get_id_suffix() {
        // Same basename registered from two parents; also collides post-slug ("api!" → "api").
        let a = repo(3, "api");
        let b = repo(7, "api!");
        let all = vec![a.clone(), b.clone()];
        assert_eq!(
            notes_path(Path::new("/notes"), &a, &all),
            PathBuf::from("/notes/api-3.md")
        );
        assert_eq!(
            notes_path(Path::new("/notes"), &b, &all),
            PathBuf::from("/notes/api-7.md")
        );
    }

    #[test]
    fn read_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let r = repo(1, "api");
        let all = vec![r.clone()];
        assert!(read(dir.path(), &r, &all).unwrap().is_none());
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let r = repo(1, "api");
        let all = vec![r.clone()];
        let path = write(dir.path(), &r, &all, "use `pnpm test`, never `npm test`").unwrap();
        // The human-editability contract: the file is where a human would look for it.
        assert_eq!(path, dir.path().join("api.md"));
        assert!(path.exists());
        assert_eq!(
            read(dir.path(), &r, &all).unwrap().as_deref(),
            Some("use `pnpm test`, never `npm test`")
        );
    }

    #[test]
    fn write_rejects_over_cap() {
        let dir = tempfile::tempdir().unwrap();
        let r = repo(1, "api");
        let all = vec![r.clone()];
        let err = write(dir.path(), &r, &all, &"x".repeat(MAX_NOTES_BYTES + 1)).unwrap_err();
        assert!(err.to_string().contains("8192"), "unhelpful error: {err}");
        assert!(write(dir.path(), &r, &all, &"x".repeat(MAX_NOTES_BYTES)).is_ok());
    }

    #[test]
    fn overwrite_keeps_previous_in_bak() {
        let dir = tempfile::tempdir().unwrap();
        let r = repo(1, "api");
        let all = vec![r.clone()];
        write(dir.path(), &r, &all, "v1").unwrap();
        write(dir.path(), &r, &all, "v2").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("api.md.bak")).unwrap(),
            "v1"
        );
        assert_eq!(read(dir.path(), &r, &all).unwrap().as_deref(), Some("v2"));
    }

    #[test]
    fn read_truncates_oversized_hand_edited_file() {
        let dir = tempfile::tempdir().unwrap();
        let r = repo(1, "api");
        let all = vec![r.clone()];
        std::fs::write(dir.path().join("api.md"), "a".repeat(20_000)).unwrap();
        let s = read(dir.path(), &r, &all).unwrap().unwrap();
        assert!(s.ends_with(TRUNCATION_MARKER));
        assert!(s.len() <= MAX_NOTES_BYTES + TRUNCATION_MARKER.len() + 2);

        // Multibyte content must truncate on a char boundary, not panic mid-codepoint.
        std::fs::write(dir.path().join("api.md"), "é".repeat(10_000)).unwrap();
        let s = read(dir.path(), &r, &all).unwrap().unwrap();
        assert!(s.ends_with(TRUNCATION_MARKER));
    }
}
