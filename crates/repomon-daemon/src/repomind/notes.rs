//! File-first per-repo notes: `fleet/<repo>/notes.md` in the repomind home.
//!
//! Repo notes used to live only in the daemon's `repo-notes/` app-support directory, readable
//! by nothing but the `repo_notes` MCP tools. They now live in the home instead, so a controller
//! with basic-memory (or any agent that can read a file) sees the same conventions and gotchas
//! the daemon folds into every worker prompt.
//!
//! The file carries frontmatter in the home's note conventions; the body is what agents read and
//! write, so the 8 KB cap and the MCP semantics are unchanged from the app-support era. Notes are
//! a full replace, never an append log, which is why there is no merge here.

use std::path::{Path, PathBuf};

use repomon_core::error::{Error, Result};
use repomon_core::model::Repo;
use repomon_core::notes::{MAX_NOTES_BYTES, TRUNCATION_MARKER};

use super::md;

/// The home directory holding one repo's notes: `<home>/fleet/<slug>`. Repo names are not
/// unique, so on a slug collision every collider resolves to `<slug>-<id>` rather than the bare
/// name, and notes can never bleed across repos (the `repomon_core::notes` rule).
pub fn repo_dir(home: &Path, repo: &Repo, all: &[Repo]) -> PathBuf {
    let base = md::slug(&repo.name);
    let collides = all
        .iter()
        .any(|other| other.id != repo.id && md::slug(&other.name) == base);
    let name = if collides {
        format!("{base}-{}", repo.id)
    } else {
        base
    };
    home.join("fleet").join(name)
}

/// `<home>/fleet/<slug>/notes.md`.
pub fn notes_path(home: &Path, repo: &Repo, all: &[Repo]) -> PathBuf {
    repo_dir(home, repo, all).join("notes.md")
}

/// The path relative to the home, for the export batch and for RPC results.
pub fn rel_path(home: &Path, path: &Path) -> String {
    path.strip_prefix(home)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Read a repo's notes body (frontmatter stripped). `Ok(None)` when no file exists. A
/// hand-edited body over [`MAX_NOTES_BYTES`] is truncated at a char boundary with
/// [`TRUNCATION_MARKER`] appended, exactly as the app-support reader did.
pub fn read(home: &Path, repo: &Repo, all: &[Repo]) -> Result<Option<String>> {
    let path = notes_path(home, repo, all);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::Io(e)),
    };
    let (_, mut body) = md::split_frontmatter(&raw);
    if body.len() > MAX_NOTES_BYTES {
        let mut end = MAX_NOTES_BYTES;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        body.truncate(end);
        body.push('\n');
        body.push_str(TRUNCATION_MARKER);
    }
    Ok(Some(body))
}

/// Replace a repo's notes wholesale: the body plus the home's frontmatter conventions.
/// Rejects a body over [`MAX_NOTES_BYTES`]. Returns the path written.
pub fn write(home: &Path, repo: &Repo, all: &[Repo], body: &str) -> Result<PathBuf> {
    if body.len() > MAX_NOTES_BYTES {
        return Err(Error::Config(format!(
            "notes are {} bytes; the cap is {MAX_NOTES_BYTES} bytes - trim before writing",
            body.len()
        )));
    }
    let path = notes_path(home, repo, all);
    let dir = path.parent().unwrap_or(home).to_path_buf();
    std::fs::create_dir_all(&dir)?;

    let slug = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo")
        .to_string();
    // No `.bak` sibling here (unlike the app-support writer): the home is a git repo, so the
    // previous version is one `git show` away and a stray backup file would just be clutter the
    // export never commits.
    let doc = md::frontmatter(&[
        ("title", format!("{} notes", repo.name)),
        ("type", "note".to_string()),
        ("permalink", format!("repomind/fleet/{slug}/notes")),
        (
            "source",
            format!("repomond {}", chrono::Utc::now().format("%Y-%m-%d")),
        ),
    ]) + body;
    std::fs::write(&path, doc)?;
    Ok(path)
}

/// One-time migration: copy notes that only exist in the app-support directory into the home.
/// A repo whose home file already exists is left alone, and the old file is never deleted, so a
/// rollback still finds its notes where it left them. Returns the paths written.
pub fn migrate(home: &Path, legacy_dir: &Path, repos: &[Repo]) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for repo in repos {
        if notes_path(home, repo, repos).exists() {
            continue;
        }
        let Some(body) = repomon_core::notes::read(legacy_dir, repo, repos)? else {
            continue;
        };
        if body.trim().is_empty() {
            continue;
        }
        written.push(write(home, repo, repos, &body)?);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(id: i64, name: &str) -> Repo {
        Repo {
            id,
            path: PathBuf::from(format!("/tmp/{name}")),
            name: name.to_string(),
            added_at: chrono::Utc::now(),
            worktree_root_template: None,
            hidden: false,
            position: None,
            label: None,
        }
    }

    fn home() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        super::super::ensure_layout(&home).unwrap();
        (dir, home)
    }

    #[test]
    fn notes_path_is_one_directory_per_repo_under_fleet() {
        let (_d, home) = home();
        let r = repo(1, "Repomon");
        assert_eq!(
            notes_path(&home, &r, &[r.clone()]),
            home.join("fleet/repomon/notes.md")
        );
    }

    #[test]
    fn notes_path_disambiguates_two_repos_sharing_a_name() {
        let (_d, home) = home();
        let all = vec![repo(1, "site"), repo(2, "site")];
        assert_eq!(
            notes_path(&home, &all[0], &all),
            home.join("fleet/site-1/notes.md")
        );
        assert_eq!(
            notes_path(&home, &all[1], &all),
            home.join("fleet/site-2/notes.md")
        );
    }

    #[test]
    fn write_then_read_round_trips_the_body_and_leaves_frontmatter_out_of_it() {
        let (_d, home) = home();
        let r = repo(1, "repomon");
        let all = vec![r.clone()];

        let path = write(&home, &r, &all, "# repomon\n\nRun `cargo test`.\n").unwrap();

        assert_eq!(path, home.join("fleet/repomon/notes.md"));
        let raw = std::fs::read_to_string(&path).unwrap();
        let (fm, _) = md::split_frontmatter(&raw);
        let fm = fm.expect("notes need frontmatter");
        assert_eq!(md::field(&fm, "type").as_deref(), Some("note"));
        assert_eq!(
            md::field(&fm, "permalink").as_deref(),
            Some("repomind/fleet/repomon/notes")
        );
        assert_eq!(
            read(&home, &r, &all).unwrap().as_deref(),
            Some("# repomon\n\nRun `cargo test`.\n")
        );
    }

    #[test]
    fn read_is_none_when_the_repo_has_no_notes_file() {
        let (_d, home) = home();
        let r = repo(1, "repomon");
        assert_eq!(read(&home, &r, &[r.clone()]).unwrap(), None);
    }

    #[test]
    fn read_returns_a_hand_written_file_that_has_no_frontmatter() {
        let (_d, home) = home();
        let r = repo(1, "repomon");
        std::fs::create_dir_all(home.join("fleet/repomon")).unwrap();
        std::fs::write(home.join("fleet/repomon/notes.md"), "just prose\n").unwrap();

        assert_eq!(
            read(&home, &r, &[r.clone()]).unwrap().as_deref(),
            Some("just prose\n")
        );
    }

    #[test]
    fn read_truncates_a_body_a_human_grew_past_the_cap() {
        let (_d, home) = home();
        let r = repo(1, "repomon");
        std::fs::create_dir_all(home.join("fleet/repomon")).unwrap();
        std::fs::write(
            home.join("fleet/repomon/notes.md"),
            "x".repeat(MAX_NOTES_BYTES + 500),
        )
        .unwrap();

        let body = read(&home, &r, &[r.clone()]).unwrap().unwrap();

        assert!(body.ends_with(TRUNCATION_MARKER), "{}", &body[..40]);
    }

    #[test]
    fn write_rejects_a_body_over_the_cap() {
        let (_d, home) = home();
        let r = repo(1, "repomon");
        let err = write(&home, &r, &[r.clone()], &"x".repeat(MAX_NOTES_BYTES + 1)).unwrap_err();
        assert!(err.to_string().contains("cap"), "{err}");
    }

    #[test]
    fn migration_copies_a_legacy_notes_file_into_the_home_and_keeps_the_original() {
        let (_d, home) = home();
        let legacy_dir = tempfile::tempdir().unwrap();
        let r = repo(1, "repomon");
        let all = vec![r.clone()];
        let legacy = repomon_core::notes::notes_path(legacy_dir.path(), &r, &all);
        std::fs::write(&legacy, "legacy knowledge\n").unwrap();

        let written = migrate(&home, legacy_dir.path(), &all).unwrap();

        assert_eq!(written, vec![home.join("fleet/repomon/notes.md")]);
        assert_eq!(
            read(&home, &r, &all).unwrap().as_deref(),
            Some("legacy knowledge\n")
        );
        assert!(legacy.exists(), "the old file must not be deleted");

        // Idempotent: a second start migrates nothing.
        assert!(migrate(&home, legacy_dir.path(), &all).unwrap().is_empty());
    }

    #[test]
    fn migration_never_overwrites_notes_already_in_the_home() {
        let (_d, home) = home();
        let legacy_dir = tempfile::tempdir().unwrap();
        let r = repo(1, "repomon");
        let all = vec![r.clone()];
        std::fs::write(
            repomon_core::notes::notes_path(legacy_dir.path(), &r, &all),
            "stale\n",
        )
        .unwrap();
        write(&home, &r, &all, "current\n").unwrap();

        assert!(migrate(&home, legacy_dir.path(), &all).unwrap().is_empty());
        assert_eq!(read(&home, &r, &all).unwrap().as_deref(), Some("current\n"));
    }
}
