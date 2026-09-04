//! The repomind home repo and its controller lane.
//!
//! Repomind's memory lives in a git repo on disk (`~/repomind` by default, `[repomind] home` in
//! the config) rather than only in the daemon's SQLite. This module makes sure that repo exists,
//! is registered like any other repo, and has exactly one lane marked `role = "controller"`:
//! the lane every controller agent runs in.
//!
//! Two rules shape everything here:
//!
//! - **Never overwrite.** The operator (and, in phase R0, another agent) authors the real
//!   `AGENTS.md` / `REPOMIND.md` content. Ensure-home only ever fills in what is missing, so a
//!   daemon restart can never clobber a hand-written protocol file.
//! - **Idempotent.** It runs on every daemon start and on every `orchestrator.start`, and a
//!   second run does nothing and logs nothing.

use std::path::{Path, PathBuf};

use repomon_core::model::{LaneId, RepoId};

use crate::Ctx;

/// The role string on the repomind lane. Nothing else in the fleet carries a role today.
pub const CONTROLLER_ROLE: &str = "controller";

/// Directories created when the home is missing, from the design spec's layout. Nested paths are
/// created in full, so `plans/` and `playbooks/` need no separate entry.
const HOME_DIRS: &[&str] = &[
    "profile",
    "plans/active",
    "plans/standing",
    "plans/done",
    "playbooks/drafts",
    "fleet",
    "journal",
    "knowledge",
    "sessions",
    ".repomind",
];

/// Seed files written only when absent. These are deliberately thin: they exist so a fresh home
/// is a usable git repo from the first commit, and they are replaced wholesale by the operator's
/// own protocol and persona files.
const SEED_FILES: &[(&str, &str)] = &[
    (
        ".gitignore",
        "# Daemon-owned scratch: assembled boot context, export state, locks.\n.repomind/\n",
    ),
    (
        "AGENTS.md",
        "# Agent protocol\n\n\
         This repo is repomind's memory. Every agent that runs here follows the same rules:\n\n\
         - Search before you write, and edit the existing note instead of adding a duplicate.\n\
         - One fact per observation, one concept per note.\n\
         - Scope: `profile/` for standing facts, `fleet/<repo>/` for per-repo notes,\n\
         `knowledge/` for cross-cutting facts, `plans/` for goals in flight.\n\
         - Never store credentials, tokens, or transient state.\n\
         - Live fleet state beats memory: read it with the fleet tools, never from a note.\n\n\
         This file is a placeholder written by the daemon because none existed. Replace it.\n",
    ),
    (
        "REPOMIND.md",
        "# Repomind\n\n\
         The operator's overlay on the shipped repomind persona: voice, defaults, and house\n\
         rules. Repomind is a tech lead, not an implementer - code work goes to workers in\n\
         project lanes, and files are written only inside this repo.\n\n\
         This file is a placeholder written by the daemon because none existed. Replace it.\n",
    ),
];

/// What ensure-home actually did. Empty on every run after the first, which is what keeps the
/// startup log quiet.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct HomeActions {
    /// Directories created, relative to the home.
    pub dirs: Vec<String>,
    /// Seed files written, relative to the home.
    pub files: Vec<String>,
    /// Whether `git init` ran.
    pub git_init: bool,
}

impl HomeActions {
    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty() && self.files.is_empty() && !self.git_init
    }
}

/// The resolved home: where it lives and which repo/lane represent it in the fleet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepomindHome {
    pub path: PathBuf,
    pub repo_id: RepoId,
    pub lane_id: LaneId,
}

/// Create the spec's directory layout and seed files under `home`, without touching anything that
/// already exists. Creating the home directory itself is part of this.
pub fn ensure_layout(home: &Path) -> std::io::Result<HomeActions> {
    let mut actions = HomeActions::default();
    if !home.exists() {
        std::fs::create_dir_all(home)?;
        actions.dirs.push(".".to_string());
    }
    for dir in HOME_DIRS {
        let path = home.join(dir);
        if !path.exists() {
            std::fs::create_dir_all(&path)?;
            actions.dirs.push((*dir).to_string());
        }
    }
    for (name, body) in SEED_FILES {
        let path = home.join(name);
        if !path.exists() {
            std::fs::write(&path, body)?;
            actions.files.push((*name).to_string());
        }
    }
    Ok(actions)
}

/// `git init -b main` unless `home` is already a git repo. Returns whether init ran.
pub fn ensure_git_repo(home: &Path) -> std::io::Result<bool> {
    if home.join(".git").exists() {
        return Ok(false);
    }
    let out = std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(home)
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::other(format!(
            "git init in {} failed: {}",
            home.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(true)
}

/// Make sure the repomind home exists on disk, is registered as a repo, and has exactly one lane
/// marked `role = "controller"`. Idempotent: safe to call on every daemon start and on every
/// `orchestrator.start`.
pub async fn ensure_home(ctx: &Ctx) -> repomon_core::Result<RepomindHome> {
    // One ensure at a time: concurrent starts would otherwise race on `git init` in the same
    // folder, and git fails the loser rather than treating the repo as already there.
    let _guard = ctx.repomind_lock.lock().await;
    let path = ctx.config.read().await.repomind_home();

    let for_fs = path.clone();
    let (actions, git_init) = tokio::task::spawn_blocking(move || -> std::io::Result<_> {
        let actions = ensure_layout(&for_fs)?;
        let git_init = ensure_git_repo(&for_fs)?;
        Ok((actions, git_init))
    })
    .await
    .map_err(|e| repomon_core::Error::Other(e.to_string()))??;

    for dir in &actions.dirs {
        tracing::info!("repomind home: created {}", path.join(dir).display());
    }
    for file in &actions.files {
        tracing::info!("repomind home: seeded {}", path.join(file).display());
    }
    if git_init {
        tracing::info!("repomind home: git init -b main in {}", path.display());
    }

    let known = ctx.store.find_repo_by_path(path.clone()).await?.is_some();
    let repo = ctx.registry.add(&path).await?;
    if !known {
        tracing::info!(
            "repomind home: registered repo {} at {}",
            repo.name,
            repo.path.display()
        );
    }

    // Take the lane id from `lanes.list` rather than minting one directly, so the controller lane
    // is the exact same row `lane.list` surfaces for the home's main worktree (path spellings can
    // otherwise differ between what git reports and what was registered).
    let lanes = ctx.lanes.list().await?;
    let lane_id = lanes
        .iter()
        .find(|lane| lane.repo.id == repo.id && lane.worktree.is_main)
        .map(|lane| lane.id)
        .ok_or_else(|| {
            repomon_core::Error::Other(format!(
                "repomind home {} has no main worktree lane",
                path.display()
            ))
        })?;

    if ctx.store.controller_lane().await? != Some(lane_id) {
        ctx.store
            .set_lane_role(lane_id, Some(CONTROLLER_ROLE.to_string()))
            .await?;
        tracing::info!("repomind home: lane {lane_id} marked as the controller lane");
    }

    Ok(RepomindHome {
        path,
        repo_id: repo.id,
        lane_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use repomon_core::{Config, Store};
    use std::sync::Arc;

    #[test]
    fn ensure_layout_creates_the_spec_directories_and_seed_files() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        let actions = ensure_layout(&home).unwrap();

        for expected in HOME_DIRS {
            assert!(
                home.join(expected).is_dir(),
                "{expected} should have been created"
            );
        }
        for (name, _) in SEED_FILES {
            assert!(home.join(name).is_file(), "{name} should have been written");
        }
        assert!(
            std::fs::read_to_string(home.join(".gitignore"))
                .unwrap()
                .contains(".repomind/"),
            "the daemon's scratch dir must be ignored"
        );
        assert!(!actions.is_empty());
    }

    #[test]
    fn ensure_layout_never_overwrites_an_existing_file() {
        // The real AGENTS.md and REPOMIND.md are authored by hand; a daemon restart must not
        // replace them with the placeholders.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        std::fs::write(home.join("AGENTS.md"), "the real protocol\n").unwrap();

        let actions = ensure_layout(&home).unwrap();

        assert_eq!(
            std::fs::read_to_string(home.join("AGENTS.md")).unwrap(),
            "the real protocol\n"
        );
        assert!(!actions.files.contains(&"AGENTS.md".to_string()));
        assert!(actions.files.contains(&"REPOMIND.md".to_string()));
    }

    #[test]
    fn ensure_layout_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        ensure_layout(&home).unwrap();
        let second = ensure_layout(&home).unwrap();
        assert!(
            second.is_empty(),
            "a second run should report no actions, got {second:?}"
        );
    }

    #[test]
    fn ensure_git_repo_initializes_once_on_main() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        assert!(ensure_git_repo(&home).unwrap());
        assert!(home.join(".git").exists());

        let head = std::fs::read_to_string(home.join(".git").join("HEAD")).unwrap();
        assert!(head.contains("refs/heads/main"), "HEAD was {head:?}");

        assert!(
            !ensure_git_repo(&home).unwrap(),
            "a second run must not re-init"
        );
    }

    async fn test_ctx(home: &Path) -> Arc<Ctx> {
        let store = Store::open_in_memory().unwrap();
        let mut config = Config::default();
        config.repomind.home = home.to_string_lossy().into_owned();
        Ctx::new(store, config, None)
    }

    #[tokio::test]
    async fn ensure_home_registers_the_repo_and_marks_one_controller_lane() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        let ctx = test_ctx(&home).await;

        let first = ensure_home(&ctx).await.unwrap();
        assert!(home.join("plans").join("active").is_dir());
        assert!(home.join(".git").exists());

        let repos = ctx.registry.list().await.unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "repomind");

        assert_eq!(
            ctx.store.controller_lane().await.unwrap(),
            Some(first.lane_id)
        );

        // Second run: same repo, same lane, still exactly one controller.
        let second = ensure_home(&ctx).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(ctx.registry.list().await.unwrap().len(), 1);
        let controllers = ctx
            .store
            .list_lane_meta()
            .await
            .unwrap()
            .into_iter()
            .filter(|m| m.role.as_deref() == Some(CONTROLLER_ROLE))
            .count();
        assert_eq!(controllers, 1);
    }

    /// Two `orchestrator.start` calls can land at once (the TUI's auto-start and
    /// `repomon orchestrate` both fire at startup). Both ensure the home, and a `git init` race
    /// between them must not fail either one.
    #[tokio::test]
    async fn concurrent_ensure_home_calls_both_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        let ctx = test_ctx(&home).await;

        let a = { let ctx = ctx.clone(); tokio::spawn(async move { ensure_home(&ctx).await }) };
        let b = { let ctx = ctx.clone(); tokio::spawn(async move { ensure_home(&ctx).await }) };
        let (a, b) = (a.await.unwrap(), b.await.unwrap());
        let a = a.expect("first ensure_home");
        let b = b.expect("second ensure_home");
        assert_eq!(a, b);
    }

    /// A home the operator already wrote (R0's template files, a git repo of their own) is
    /// adopted as-is: no file is rewritten and no second `git init` runs.
    #[tokio::test]
    async fn ensure_home_adopts_a_home_the_operator_already_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("REPOMIND.md"), "hand written persona\n").unwrap();
        ensure_git_repo(&home).unwrap();

        let ctx = test_ctx(&home).await;
        ensure_home(&ctx).await.unwrap();

        assert_eq!(
            std::fs::read_to_string(home.join("REPOMIND.md")).unwrap(),
            "hand written persona\n"
        );
    }
}
