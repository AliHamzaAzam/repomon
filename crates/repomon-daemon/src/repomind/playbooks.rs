//! File-first playbooks: `playbooks/<name>.md` for approved ones, `playbooks/drafts/<name>.md`
//! for everything a controller has written but no human has blessed.
//!
//! The approval gate is the whole point, and it is now visible on disk. A controller's
//! `playbook_save` can only ever create a file under `drafts/`; moving that file up one level is
//! what approval means, whether the move comes from `repomon playbooks approve`, the desktop
//! panel, or the operator's own `mv`. `search` reads the approved directory and nothing else, so
//! an unapproved draft can never be fed back to an agent as guidance.
//!
//! A revision of an approved playbook lands as a draft under the same name, beside the approved
//! file rather than over it: the approved text stays live until a human approves the revision.
//!
//! Unlike the SQLite era there is no draft expiry sweep here. Deleting a file out of the
//! operator's own git repo behind their back is a different act from dropping a hidden row, and
//! the home's history makes an abandoned draft cheap to keep.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use repomon_core::error::{Error, Result};
use repomon_core::model::Playbook;

use super::md;

/// Cap on a playbook file, matching the RPC's own limit.
pub const MAX_PLAYBOOK_BYTES: usize = 16384;

/// `<home>/playbooks/<name>.md`: an approved playbook, the only kind [`search`] returns.
pub fn approved_path(home: &Path, name: &str) -> PathBuf {
    home.join("playbooks").join(format!("{name}.md"))
}

/// `<home>/playbooks/drafts/<name>.md`: a draft, inert until a human moves it up a level.
pub fn draft_path(home: &Path, name: &str) -> PathBuf {
    home.join("playbooks")
        .join("drafts")
        .join(format!("{name}.md"))
}

/// Save `content` as a draft. A name that already has an approved file is saved as a revision
/// (frontmatter `revises`) beside it, never over it. Returns the playbook as it now stands: a
/// bare draft, or the approved one carrying a pending revision.
pub fn save(home: &Path, name: &str, content: &str) -> Result<Playbook> {
    if content.len() > MAX_PLAYBOOK_BYTES {
        return Err(Error::Config(format!(
            "playbook is {} bytes; the cap is {MAX_PLAYBOOK_BYTES} bytes",
            content.len()
        )));
    }
    let path = draft_path(home, name);
    std::fs::create_dir_all(path.parent().unwrap_or(home))?;

    let now = Utc::now();
    let created = read_doc(&path)?
        .map(|(fm, _)| stamp(&fm, "created", now))
        .unwrap_or(now);
    let revises = approved_path(home, name).exists();

    let mut fields = vec![
        ("title", name.to_string()),
        ("type", "playbook".to_string()),
        ("permalink", format!("repomind/playbooks/drafts/{name}")),
        ("status", "draft".to_string()),
        ("source", format!("repomond {}", now.format("%Y-%m-%d"))),
        ("created", created.to_rfc3339()),
    ];
    if revises {
        fields.push(("revises", name.to_string()));
    }
    std::fs::write(&path, md::frontmatter(&fields) + content)?;

    get(home, name)?.ok_or_else(|| Error::NotFound(format!("playbook {name}")))
}

/// Read a playbook file into its frontmatter block and body. `Ok(None)` when it does not exist.
fn read_doc(path: &Path) -> Result<Option<(String, String)>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let (fm, body) = md::split_frontmatter(&raw);
            Ok(Some((fm.unwrap_or_default(), body)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Io(e)),
    }
}

/// The file's last-modified time, the closest thing a file has to `updated_at`.
fn mtime(path: &Path, fallback: DateTime<Utc>) -> DateTime<Utc> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(DateTime::<Utc>::from)
        .unwrap_or(fallback)
}

/// One playbook by name, folding an approved file and any pending revision into one row.
/// `Ok(None)` when neither file exists.
pub fn get(home: &Path, name: &str) -> Result<Option<Playbook>> {
    let approved = read_doc(&approved_path(home, name))?;
    let draft = read_doc(&draft_path(home, name))?;
    let now = Utc::now();

    Ok(match (approved, draft) {
        (None, None) => None,
        // Approved, with or without a revision waiting: the approved text is what agents get.
        (Some((fm, body)), draft) => Some(Playbook {
            name: name.to_string(),
            content: body,
            status: "approved".to_string(),
            draft_content: draft.map(|(_, body)| body),
            created_at: stamp(&fm, "created", now),
            updated_at: mtime(&approved_path(home, name), now),
            approved_at: Some(stamp(&fm, "approved", now)),
        }),
        // Draft only: inert until a human moves the file.
        (None, Some((fm, body))) => Some(Playbook {
            name: name.to_string(),
            content: body,
            status: "draft".to_string(),
            draft_content: None,
            created_at: stamp(&fm, "created", now),
            updated_at: mtime(&draft_path(home, name), now),
            approved_at: None,
        }),
    })
}

/// The playbook names in one directory. `README.md` is the home's own guide, never a playbook.
fn names_in(dir: &Path) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::Io(e)),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".md") else {
            continue;
        };
        if stem.eq_ignore_ascii_case("README") {
            continue;
        }
        out.push(stem.to_string());
    }
    Ok(out)
}

/// Every playbook, drafts included, in name order: the approval surface's view.
pub fn list(home: &Path) -> Result<Vec<Playbook>> {
    let mut names = names_in(&home.join("playbooks"))?;
    names.extend(names_in(&home.join("playbooks").join("drafts"))?);
    names.sort();
    names.dedup();

    let mut out = Vec::with_capacity(names.len());
    for name in names {
        if let Some(book) = get(home, &name)? {
            out.push(book);
        }
    }
    Ok(out)
}

/// Search APPROVED playbooks only (case-insensitive substring over name and content), most
/// recently approved first. Drafts and pending revisions never surface here.
pub fn search(home: &Path, query: &str, limit: usize) -> Result<Vec<Playbook>> {
    let needle = query.to_lowercase();
    let mut hits: Vec<Playbook> = Vec::new();
    for name in names_in(&home.join("playbooks"))? {
        let Some(book) = get(home, &name)? else {
            continue;
        };
        if book.status != "approved" {
            continue;
        }
        if !needle.is_empty()
            && !book.name.to_lowercase().contains(&needle)
            && !book.content.to_lowercase().contains(&needle)
        {
            continue;
        }
        hits.push(book);
    }
    hits.sort_by(|a, b| {
        b.approved_at
            .cmp(&a.approved_at)
            .then_with(|| a.name.cmp(&b.name))
    });
    hits.truncate(limit);
    Ok(hits)
}

/// Approve a draft (or promote an approved playbook's pending revision) by moving the draft file
/// into `playbooks/` with `status: approved`.
pub fn approve(home: &Path, name: &str) -> Result<Playbook> {
    let draft = draft_path(home, name);
    let Some((fm, body)) = read_doc(&draft)? else {
        // Nothing pending. An already-approved playbook is a no-op rather than an error, so a
        // double-click in the panel cannot fail; anything else never existed.
        return get(home, name)?.ok_or_else(|| Error::NotFound(format!("playbook {name}")));
    };
    let now = Utc::now();
    let approved = approved_path(home, name);
    std::fs::create_dir_all(approved.parent().unwrap_or(home))?;
    let fields = [
        ("title", name.to_string()),
        ("type", "playbook".to_string()),
        ("permalink", format!("repomind/playbooks/{name}")),
        ("status", "approved".to_string()),
        ("source", format!("repomond {}", now.format("%Y-%m-%d"))),
        ("created", stamp(&fm, "created", now).to_rfc3339()),
        ("approved", now.to_rfc3339()),
    ];
    std::fs::write(&approved, md::frontmatter(&fields) + &body)?;
    std::fs::remove_file(&draft)?;

    get(home, name)?.ok_or_else(|| Error::NotFound(format!("playbook {name}")))
}

/// Delete a playbook outright: the approved file, the draft, or both.
pub fn delete(home: &Path, name: &str) -> Result<()> {
    let mut removed = false;
    for path in [approved_path(home, name), draft_path(home, name)] {
        match std::fs::remove_file(&path) {
            Ok(()) => removed = true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    if removed {
        Ok(())
    } else {
        Err(Error::NotFound(format!("playbook {name}")))
    }
}

/// One-time migration of the SQLite playbook rows into files: approved content to
/// `playbooks/<name>.md`, draft content (a bare draft, or a pending revision) to
/// `playbooks/drafts/<name>.md`. A file that already exists wins; the rows are left in place.
pub fn migrate(home: &Path, rows: &[Playbook]) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for row in rows {
        // The approved half, when the row has one.
        if row.status == "approved" {
            let path = approved_path(home, &row.name);
            if !path.exists() {
                std::fs::create_dir_all(path.parent().unwrap_or(home))?;
                let fields = [
                    ("title", row.name.clone()),
                    ("type", "playbook".to_string()),
                    ("permalink", format!("repomind/playbooks/{}", row.name)),
                    ("status", "approved".to_string()),
                    (
                        "source",
                        format!("repomond {}", Utc::now().format("%Y-%m-%d")),
                    ),
                    ("created", row.created_at.to_rfc3339()),
                    (
                        "approved",
                        row.approved_at.unwrap_or(row.updated_at).to_rfc3339(),
                    ),
                ];
                std::fs::write(&path, md::frontmatter(&fields) + &row.content)?;
                written.push(path);
            }
        }

        // The draft half: a bare draft row's content, or an approved row's pending revision.
        let draft_body = if row.status == "approved" {
            row.draft_content.clone()
        } else {
            Some(row.content.clone())
        };
        let Some(body) = draft_body else {
            continue;
        };
        let path = draft_path(home, &row.name);
        if path.exists() {
            continue;
        }
        std::fs::create_dir_all(path.parent().unwrap_or(home))?;
        let mut fields = vec![
            ("title", row.name.clone()),
            ("type", "playbook".to_string()),
            (
                "permalink",
                format!("repomind/playbooks/drafts/{}", row.name),
            ),
            ("status", "draft".to_string()),
            (
                "source",
                format!("repomond {}", Utc::now().format("%Y-%m-%d")),
            ),
            ("created", row.created_at.to_rfc3339()),
        ];
        if row.status == "approved" {
            fields.push(("revises", row.name.clone()));
        }
        std::fs::write(&path, md::frontmatter(&fields) + &body)?;
        written.push(path);
    }
    Ok(written)
}

/// The path relative to the home, for the export batch and RPC results.
pub fn rel_path(home: &Path, path: &Path) -> String {
    super::notes::rel_path(home, path)
}

/// Parse an RFC3339 frontmatter stamp, falling back to `default`.
fn stamp(frontmatter: &str, key: &str, default: DateTime<Utc>) -> DateTime<Utc> {
    md::field(frontmatter, key)
        .and_then(|v| DateTime::parse_from_rfc3339(&v).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        super::super::ensure_layout(&home).unwrap();
        (dir, home)
    }

    #[test]
    fn save_writes_a_draft_under_drafts_with_draft_frontmatter() {
        let (_d, home) = home();

        let book = save(&home, "fleet-sweep", "# Fleet sweep\n\nSteps.\n").unwrap();

        assert_eq!(book.status, "draft");
        assert_eq!(book.content, "# Fleet sweep\n\nSteps.\n");
        assert!(draft_path(&home, "fleet-sweep").is_file());
        assert!(
            !approved_path(&home, "fleet-sweep").exists(),
            "a save must never land in the approved directory"
        );
        let raw = std::fs::read_to_string(draft_path(&home, "fleet-sweep")).unwrap();
        let fm = md::split_frontmatter(&raw).0.expect("frontmatter");
        assert_eq!(md::field(&fm, "status").as_deref(), Some("draft"));
        assert_eq!(md::field(&fm, "title").as_deref(), Some("fleet-sweep"));
        assert!(md::field(&fm, "created").is_some());
        assert!(md::field(&fm, "source").unwrap().starts_with("repomond"));
        assert_eq!(md::field(&fm, "revises"), None);
    }

    #[test]
    fn search_never_returns_a_draft() {
        let (_d, home) = home();
        save(&home, "fleet-sweep", "sweep the fleet\n").unwrap();

        assert!(search(&home, "sweep", 10).unwrap().is_empty());
        assert!(search(&home, "", 10).unwrap().is_empty());
    }

    #[test]
    fn approve_moves_the_draft_into_playbooks_and_search_then_finds_it() {
        let (_d, home) = home();
        save(&home, "fleet-sweep", "sweep the fleet\n").unwrap();

        let book = approve(&home, "fleet-sweep").unwrap();

        assert_eq!(book.status, "approved");
        assert!(book.approved_at.is_some());
        assert!(approved_path(&home, "fleet-sweep").is_file());
        assert!(
            !draft_path(&home, "fleet-sweep").exists(),
            "approval moves the file, it does not copy it"
        );
        let raw = std::fs::read_to_string(approved_path(&home, "fleet-sweep")).unwrap();
        let fm = md::split_frontmatter(&raw).0.expect("frontmatter");
        assert_eq!(md::field(&fm, "status").as_deref(), Some("approved"));

        let hits = search(&home, "sweep", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "fleet-sweep");
        assert_eq!(hits[0].content, "sweep the fleet\n");
    }

    #[test]
    fn saving_over_an_approved_name_lands_as_a_revision_draft_beside_it() {
        let (_d, home) = home();
        save(&home, "fleet-sweep", "v1\n").unwrap();
        approve(&home, "fleet-sweep").unwrap();

        let book = save(&home, "fleet-sweep", "v2\n").unwrap();

        assert_eq!(book.status, "approved", "the approved text stays live");
        assert_eq!(book.content, "v1\n");
        assert_eq!(book.draft_content.as_deref(), Some("v2\n"));
        assert!(draft_path(&home, "fleet-sweep").is_file());
        let fm = md::split_frontmatter(&std::fs::read_to_string(draft_path(&home, "fleet-sweep")).unwrap())
            .0
            .expect("frontmatter");
        assert_eq!(md::field(&fm, "revises").as_deref(), Some("fleet-sweep"));
        // The approved file, and therefore what search returns, is untouched.
        assert_eq!(search(&home, "v1", 10).unwrap().len(), 1);
        assert!(search(&home, "v2", 10).unwrap().is_empty());
    }

    #[test]
    fn approving_a_revision_replaces_the_approved_file() {
        let (_d, home) = home();
        save(&home, "fleet-sweep", "v1\n").unwrap();
        approve(&home, "fleet-sweep").unwrap();
        save(&home, "fleet-sweep", "v2\n").unwrap();

        let book = approve(&home, "fleet-sweep").unwrap();

        assert_eq!(book.content, "v2\n");
        assert_eq!(book.draft_content, None);
        assert!(!draft_path(&home, "fleet-sweep").exists());
        assert_eq!(search(&home, "v2", 10).unwrap().len(), 1);
    }

    #[test]
    fn search_matches_name_or_content_and_honours_the_limit() {
        let (_d, home) = home();
        for (name, body) in [("alpha", "merge lanes\n"), ("beta", "spawn workers\n")] {
            save(&home, name, body).unwrap();
            approve(&home, name).unwrap();
        }

        assert_eq!(search(&home, "alpha", 10).unwrap().len(), 1);
        assert_eq!(search(&home, "SPAWN", 10).unwrap()[0].name, "beta");
        assert_eq!(search(&home, "", 1).unwrap().len(), 1);
    }

    #[test]
    fn list_shows_drafts_and_approved_playbooks_by_name() {
        let (_d, home) = home();
        save(&home, "beta", "b\n").unwrap();
        save(&home, "alpha", "a\n").unwrap();
        approve(&home, "alpha").unwrap();

        let all = list(&home).unwrap();

        let names: Vec<&str> = all.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
        assert_eq!(all[0].status, "approved");
        assert_eq!(all[1].status, "draft");
    }

    #[test]
    fn list_ignores_the_directory_readme() {
        let (_d, home) = home();
        std::fs::write(home.join("playbooks/README.md"), "guide\n").unwrap();
        std::fs::write(home.join("playbooks/drafts/README.md"), "guide\n").unwrap();

        assert!(list(&home).unwrap().is_empty());
    }

    #[test]
    fn delete_removes_the_approved_file_and_any_pending_revision() {
        let (_d, home) = home();
        save(&home, "fleet-sweep", "v1\n").unwrap();
        approve(&home, "fleet-sweep").unwrap();
        save(&home, "fleet-sweep", "v2\n").unwrap();

        delete(&home, "fleet-sweep").unwrap();

        assert!(!approved_path(&home, "fleet-sweep").exists());
        assert!(!draft_path(&home, "fleet-sweep").exists());
        assert!(delete(&home, "fleet-sweep").is_err(), "gone means not found");
    }

    #[test]
    fn approving_an_unknown_name_is_not_found() {
        let (_d, home) = home();
        assert!(approve(&home, "nothing-here").is_err());
    }

    #[test]
    fn save_rejects_content_over_the_cap() {
        let (_d, home) = home();
        let err = save(&home, "big", &"x".repeat(MAX_PLAYBOOK_BYTES + 1)).unwrap_err();
        assert!(err.to_string().contains("cap"), "{err}");
    }

    fn row(name: &str, status: &str, content: &str, draft: Option<&str>) -> Playbook {
        Playbook {
            name: name.into(),
            content: content.into(),
            status: status.into(),
            draft_content: draft.map(str::to_string),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            approved_at: (status == "approved").then(Utc::now),
        }
    }

    #[test]
    fn migration_writes_store_rows_as_files_once() {
        let (_d, home) = home();
        let rows = vec![
            row("approved-one", "approved", "live\n", None),
            row("draft-one", "draft", "wip\n", None),
            row("revised", "approved", "live\n", Some("pending\n")),
        ];

        let written = migrate(&home, &rows).unwrap();

        assert_eq!(written.len(), 4, "{written:?}");
        assert!(approved_path(&home, "approved-one").is_file());
        assert!(draft_path(&home, "draft-one").is_file());
        assert!(approved_path(&home, "revised").is_file());
        assert!(draft_path(&home, "revised").is_file());
        assert_eq!(search(&home, "live", 10).unwrap().len(), 2);
        assert!(search(&home, "wip", 10).unwrap().is_empty());

        assert!(
            migrate(&home, &rows).unwrap().is_empty(),
            "a second start migrates nothing"
        );
    }

    #[test]
    fn migration_never_overwrites_a_file_already_in_the_home() {
        let (_d, home) = home();
        save(&home, "keeper", "mine\n").unwrap();

        assert!(migrate(&home, &[row("keeper", "draft", "stale\n", None)])
            .unwrap()
            .is_empty());
        assert_eq!(get(&home, "keeper").unwrap().unwrap().content, "mine\n");
    }
}
