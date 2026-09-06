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

pub mod basic_memory;
pub mod boot;
pub mod export;
pub mod md;
pub mod notes;
pub mod playbooks;

use std::path::{Path, PathBuf};

use repomon_core::agent::supervision::{DialogClass, PolicyAction, SupervisionOverrides};
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
    "playbooks/rejected",
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

    seed_controller_policy(ctx, lane_id).await?;

    Ok(RepomindHome {
        path,
        repo_id: repo.id,
        lane_id,
    })
}

/// Resolve the controller lane's primary window: the store's recorded window when a live session
/// still runs there, otherwise the lane's earliest live session (slot order), with the record
/// corrected to match so the next read is cheap and stays true. `None` when the controller lane
/// has no live session at all — `repomind.status` reports `window: null` and `repomind.instruct`
/// refuses rather than typing into, or reporting, a window nobody is in.
///
/// "Live" means a tmux window presently exists for one of the lane's agent slots (`lane-{id}` /
/// `lane-{id}-{slot}`) — the same liveness `agent.spawn`'s controller cap check already uses.
/// This is what catches the staleness `agent.spawn` leaves behind: every spawn into the
/// controller lane (`orchestrator.start`'s first spawn, then an operator's Spawn to add another
/// controller, or the next `orchestrator.start` after a restart) unconditionally records its own
/// window as "the" controller window, even when an earlier session in the same lane is still the
/// one actually running. When that later window's session ends, the record is left pointing at a
/// corpse while the earlier session lives on.
pub async fn primary_window(ctx: &Ctx, lane_id: LaneId) -> repomon_core::Result<Option<String>> {
    let backend = ctx.backend.clone();
    let names = tokio::task::spawn_blocking(move || backend.list_windows())
        .await
        .map_err(|e| repomon_core::Error::Other(e.to_string()))??;
    let live = repomon_core::TmuxRuntime::lane_windows_in(&names, lane_id);

    let recorded = ctx.controller_lane_window().await;
    if let Some(window) = recorded.as_deref() {
        if live.iter().any(|w| w == window) {
            return Ok(Some(window.to_string()));
        }
    }
    let Some(earliest) = live.into_iter().next() else {
        return Ok(None);
    };
    if recorded.as_deref() != Some(earliest.as_str()) {
        ctx.store
            .set_lane_tmux_window(lane_id, Some(earliest.clone()))
            .await?;
    }
    Ok(Some(earliest))
}

/// [`primary_window`], for callers that need a window to act on unconditionally — the deprecated
/// `orchestrator.*` aliases — rather than one that handles "no controller" itself.
///
/// The in-memory tracked session, when this process has one, takes priority over the lane
/// resolution below: it is what `orchestrator.stop`'s kill and `reconcile_orchestrator` actually
/// keep truthful, and it is the only record of a window the lane system doesn't recognize as its
/// own — an adopted legacy `orchestrator` window surviving from a pre-R1 daemon, whose adoption
/// deliberately does not write the lane's `tmux_window` (see `orchestrator.start`). Only when
/// nothing is tracked (typically: this process hasn't adopted or spawned a controller since it
/// started) does resolution fall to the controller lane's live session, then the legacy window
/// name as a last resort.
pub async fn primary_window_or_legacy(ctx: &Ctx) -> String {
    if let Some(session) = ctx.orchestrator.lock().await.as_ref() {
        return session.window.clone();
    }
    if let Ok(Some(lane_id)) = ctx.store.controller_lane().await {
        if let Ok(Some(window)) = primary_window(ctx, lane_id).await {
            return window;
        }
    }
    ctx.controller_lane_window()
        .await
        .unwrap_or_else(|| crate::ORCHESTRATOR_WINDOW.to_string())
}

/// The dialog classes a controller may not answer for itself. A controller holds the fleet
/// catalog, so a permission prompt in its lane is about the whole fleet rather than about one
/// worktree; these classes wait for the operator instead of being auto-answered.
const CONTROLLER_HOLD_CLASSES: &[DialogClass] = &[
    DialogClass::Deletion,
    DialogClass::PushRemote,
    DialogClass::CredentialAccess,
    DialogClass::Install,
    DialogClass::DeviceAccess,
];

/// Give the controller lane its supervision default the first time the home is ensured: `hold` on
/// every destructive class, supervision itself left off so the operator opts in exactly as they do
/// for any other lane.
///
/// Written once and never again. An operator who relaxes a class here keeps that choice across
/// daemon restarts, which is why this refuses to touch a lane that already has a policy row.
async fn seed_controller_policy(ctx: &Ctx, lane_id: LaneId) -> repomon_core::Result<()> {
    if ctx.store.lane_policy(lane_id).await?.is_some() {
        return Ok(());
    }
    let classes = CONTROLLER_HOLD_CLASSES
        .iter()
        .map(|class| (*class, PolicyAction::Hold))
        .collect();
    ctx.store
        .set_lane_policy(SupervisionOverrides {
            lane_id,
            enabled: false,
            classes,
            nudge_text: None,
            stall_mins: None,
            nudge_retries: None,
            expect_work: false,
            updated_at: chrono::Utc::now(),
        })
        .await?;
    tracing::info!("repomind home: lane {lane_id} seeded with hold-on-destructive supervision");
    Ok(())
}

/// Count what the home holds, for `repomind.status`. Directories that do not exist count zero,
/// and each directory's own `README.md` is the home's guide rather than a plan or a playbook.
pub fn home_counts(home: &Path) -> repomon_core::model::RepomindCounts {
    repomon_core::model::RepomindCounts {
        active_plans: markdown_files(&home.join("plans").join("active")),
        standing: markdown_files(&home.join("plans").join("standing")),
        playbooks: markdown_files(&home.join("playbooks")),
        drafts: markdown_files(&home.join("playbooks").join("drafts")),
    }
}

/// Markdown files directly in `dir`, excluding its `README.md`. A missing directory is zero.
fn markdown_files(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.ends_with(".md") && !name.eq_ignore_ascii_case("README.md")
        })
        .count()
}

/// Everything the repomind home needs at daemon start, in order: make the home and its
/// controller lane exist, migrate records that still only live in the daemon's own storage,
/// register the home as a basic-memory project, roll expired journal days into the archive, and
/// queue one export.
///
/// That last step is what makes a fresh install (or a restart that missed a burst of rows) catch
/// up: exports are otherwise only triggered by a new store write or the `repomind.export` RPC,
/// so rows written before the home existed would sit unexported forever. It goes through the
/// ordinary debounced path, so it costs one commit, batched with anything else in flight.
///
/// Never fails the daemon: each step logs its own failure and the next one still runs.
pub async fn start(ctx: &Ctx) {
    if let Err(e) = ensure_home(ctx).await {
        tracing::warn!("repomind home unavailable: {e}");
        return;
    }
    // File-first records (repo notes, playbooks): copy anything that still only lives in the
    // daemon's own storage into the home. Idempotent, and never deletes the original.
    if let Err(e) = migrate_records(ctx).await {
        tracing::warn!("repomind record migration failed: {e}");
    }
    if let Err(e) = basic_memory::ensure_project(ctx).await {
        tracing::warn!("basic-memory project registration failed: {e}");
    }
    if let Err(e) = export::archive_now(ctx).await {
        tracing::warn!("repomind journal archive rollup failed: {e}");
    }
    export::request(ctx).await;
}

/// One-time file-first migration, run once per daemon start right after [`ensure_home`]: records
/// that only exist in the daemon's own storage are written into the home so an agent can read
/// them as files. Nothing is ever deleted from the old location, so a rollback still finds it.
/// Idempotent: a repo or playbook already present in the home is left exactly as it is.
pub async fn migrate_records(ctx: &Ctx) -> repomon_core::Result<Vec<String>> {
    let home = ctx.config.read().await.repomind_home();
    if !home.is_dir() {
        return Ok(Vec::new());
    }
    let repos = ctx.registry.list().await?;
    let legacy = ctx.notes_dir.clone();

    let rows = ctx.store.list_playbooks().await?;

    let for_fs = home.clone();
    let (notes_written, books_written) =
        tokio::task::spawn_blocking(move || -> repomon_core::Result<_> {
            Ok((
                notes::migrate(&for_fs, &legacy, &repos)?,
                playbooks::migrate(&for_fs, &rows)?,
            ))
        })
        .await
        .map_err(|e| repomon_core::Error::Other(e.to_string()))??;

    let mut all = Vec::new();
    for (kind, written) in [("notes", notes_written), ("playbooks", books_written)] {
        let rel: Vec<String> = written.iter().map(|p| notes::rel_path(&home, p)).collect();
        for path in &rel {
            tracing::info!("repomind home: migrated {kind} into {path}");
        }
        export::request_files(ctx, kind, rel.clone()).await;
        all.extend(rel);
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use repomon_core::agent::backend::SpawnSpec;
    use repomon_core::{Config, Store, TmuxRuntime};
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

    /// Like [`test_ctx`] but pinned to a throwaway tmux `-L` session, for tests that spawn real
    /// windows: `Config::default()`'s `tmux_session` is `"repomon"`, the real daemon's session, so
    /// any test that actually talks to tmux must never use it.
    async fn test_ctx_with_tmux(home: &Path, tmux_session: String) -> Arc<Ctx> {
        let store = Store::open_in_memory().unwrap();
        let mut config = Config {
            tmux_session,
            ..Default::default()
        };
        config.repomind.home = home.to_string_lossy().into_owned();
        Ctx::new(store, config, None)
    }

    /// A tmux `-L` session name unique to this test run, so parallel tests (and parallel CI
    /// runs) never collide or touch the operator's real `repomon` session.
    fn unique_tmux_session(tag: &str) -> String {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!(
            "repomon-primary-window-it-{tag}-{}-{seq}",
            std::process::id()
        )
    }

    fn kill_tmux_session(session: &str) {
        let _ = std::process::Command::new(repomon_core::agent::tmux_program())
            .args(["-L", session, "kill-server"])
            .output();
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

    #[tokio::test]
    async fn ensure_home_holds_destructive_dialogs_in_the_controller_lane() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        let ctx = test_ctx(&home).await;

        let lane_id = ensure_home(&ctx).await.unwrap().lane_id;
        let policy = ctx
            .store
            .lane_policy(lane_id)
            .await
            .unwrap()
            .expect("the controller lane is seeded with a policy");

        for class in CONTROLLER_HOLD_CLASSES {
            assert_eq!(
                policy.classes.get(class),
                Some(&PolicyAction::Hold),
                "{class:?} must hold for a controller"
            );
        }
        // Supervision itself stays opt-in, exactly as it is for any other lane.
        assert!(!policy.enabled);

        // The operator's own relaxation survives the next ensure: this is a seed, not a policy the
        // daemon re-imposes on every start.
        let mut relaxed = policy.clone();
        relaxed
            .classes
            .insert(DialogClass::Install, PolicyAction::AutoApprove);
        ctx.store.set_lane_policy(relaxed).await.unwrap();

        ensure_home(&ctx).await.unwrap();
        let after = ctx.store.lane_policy(lane_id).await.unwrap().unwrap();
        assert_eq!(
            after.classes.get(&DialogClass::Install),
            Some(&PolicyAction::AutoApprove)
        );
    }

    /// Two `orchestrator.start` calls can land at once (the TUI's auto-start and
    /// `repomon orchestrate` both fire at startup). Both ensure the home, and a `git init` race
    /// between them must not fail either one.
    #[tokio::test]
    async fn concurrent_ensure_home_calls_both_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        let ctx = test_ctx(&home).await;

        let a = {
            let ctx = ctx.clone();
            tokio::spawn(async move { ensure_home(&ctx).await })
        };
        let b = {
            let ctx = ctx.clone();
            tokio::spawn(async move { ensure_home(&ctx).await })
        };
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

    /// Repo notes that only exist in the app-support directory are copied into the home on the
    /// first start after R2, and the old file is left where it was.
    #[tokio::test]
    async fn migrate_records_copies_legacy_repo_notes_into_the_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        let legacy = dir.path().join("repo-notes");
        std::fs::create_dir_all(&legacy).unwrap();

        let store = Store::open_in_memory().unwrap();
        let mut config = Config::default();
        config.repomind.home = home.to_string_lossy().into_owned();
        let ctx = Ctx::new_with_paths(
            store,
            config,
            None,
            dir.path().join("config.toml"),
            legacy.clone(),
        );
        ensure_home(&ctx).await.unwrap();

        let repos = ctx.registry.list().await.unwrap();
        let legacy_file = repomon_core::notes::notes_path(&legacy, &repos[0], &repos);
        std::fs::write(&legacy_file, "old knowledge\n").unwrap();

        let written = migrate_records(&ctx).await.unwrap();

        assert_eq!(written, vec!["fleet/repomind/notes.md".to_string()]);
        assert!(
            std::fs::read_to_string(home.join("fleet/repomind/notes.md"))
                .unwrap()
                .contains("old knowledge")
        );
        assert!(legacy_file.exists(), "the old file must survive");
        assert!(
            migrate_records(&ctx).await.unwrap().is_empty(),
            "a second start migrates nothing"
        );
    }

    /// Playbook rows that only exist in SQLite become files on the first start after R2: an
    /// approved one at the root, a draft under `drafts/`, and the approval gate is unchanged.
    #[tokio::test]
    async fn migrate_records_writes_playbook_rows_as_files() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        let db = dir.path().join("legacy.db");
        let store = Store::open(&db).unwrap();
        let connection = rusqlite::Connection::open(&db).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        connection
            .execute(
                "INSERT INTO playbooks(name, content, status, created_at, updated_at, approved_at)
             VALUES ('blessed', 'live' || char(10), 'approved', ?1, ?1, ?1),
                    ('pending', 'wip' || char(10), 'draft', ?1, ?1, NULL)",
                [&now],
            )
            .unwrap();
        drop(connection);

        let mut config = Config::default();
        config.repomind.home = home.to_string_lossy().into_owned();
        let ctx = Ctx::new_with_paths(
            store,
            config,
            Some(db),
            dir.path().join("config.toml"),
            dir.path().join("legacy-notes"),
        );
        ensure_home(&ctx).await.unwrap();

        let written = migrate_records(&ctx).await.unwrap();

        assert!(
            written.contains(&"playbooks/blessed.md".to_string()),
            "{written:?}"
        );
        assert!(
            written.contains(&"playbooks/drafts/pending.md".to_string()),
            "{written:?}"
        );
        assert_eq!(playbooks::search(&home, "live", 10).unwrap().len(), 1);
        assert!(
            playbooks::search(&home, "wip", 10).unwrap().is_empty(),
            "a migrated draft stays inert"
        );
        assert_eq!(
            ctx.store.list_playbooks().await.unwrap().len(),
            2,
            "legacy rows survive migration"
        );
        std::fs::write(
            home.join("playbooks/blessed.md"),
            "edited after migration\n",
        )
        .unwrap();
        assert!(migrate_records(&ctx).await.unwrap().is_empty());
        assert_eq!(
            std::fs::read_to_string(home.join("playbooks/blessed.md")).unwrap(),
            "edited after migration\n"
        );
    }

    #[test]
    fn home_counts_reads_the_plans_and_playbook_directories() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        ensure_layout(&home).unwrap();
        std::fs::write(home.join("plans/active/ship-r2.md"), "x").unwrap();
        std::fs::write(home.join("plans/active/README.md"), "guide").unwrap();
        std::fs::write(home.join("plans/standing/nightly.md"), "x").unwrap();
        std::fs::write(home.join("playbooks/blessed.md"), "x").unwrap();
        std::fs::write(home.join("playbooks/drafts/wip.md"), "x").unwrap();
        std::fs::write(home.join("playbooks/drafts/notes.txt"), "x").unwrap();

        let counts = home_counts(&home);

        assert_eq!(
            counts,
            repomon_core::model::RepomindCounts {
                active_plans: 1,
                standing: 1,
                playbooks: 1,
                drafts: 1,
            }
        );
    }

    #[test]
    fn home_counts_are_zero_when_the_home_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            home_counts(&dir.path().join("nope")),
            repomon_core::model::RepomindCounts::default()
        );
    }

    /// The exact production scenario: Start recorded window one, a later Spawn (or a second
    /// Start after a restart) recorded window two, and window two's session has since ended
    /// while window one is still the live controller. `primary_window` must resolve to the live
    /// window one, not the stale recorded window two, and must correct the record.
    #[tokio::test]
    async fn primary_window_resolves_a_stale_recorded_window_to_the_live_session() {
        if !TmuxRuntime::available() {
            eprintln!("tmux not available; skipping primary_window liveness test");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        let session = unique_tmux_session("stale");
        let ctx = test_ctx_with_tmux(&home, session.clone()).await;
        let lane_id = ensure_home(&ctx).await.unwrap().lane_id;
        let tmp = std::env::temp_dir();

        let w1 = ctx
            .backend
            .spawn(lane_id, &SpawnSpec::new("sleep 60", &tmp))
            .expect("spawn window one");
        let w2 = ctx
            .backend
            .spawn(lane_id, &SpawnSpec::new("sleep 60", &tmp))
            .expect("spawn window two");
        assert_ne!(w1, w2);
        // Mirrors what `agent.spawn` does unconditionally: the newest window becomes "the"
        // recorded controller window, even though window one is still running.
        ctx.store
            .set_lane_tmux_window(lane_id, Some(w2.clone()))
            .await
            .unwrap();
        // Window two's session ends; window one is still live.
        ctx.backend.kill_named(&w2).expect("kill window two");

        let resolved = primary_window(&ctx, lane_id).await.unwrap();
        assert_eq!(
            resolved,
            Some(w1.clone()),
            "must resolve to the live window"
        );
        assert_eq!(
            ctx.controller_lane_window().await,
            Some(w1),
            "the record must be corrected to the live window"
        );

        kill_tmux_session(&session);
    }

    /// Two live controller sessions in the lane (the operator deliberately ran Spawn to add a
    /// second controller, under the configured cap): the recorded window is kept as-is rather
    /// than being switched to the earliest one just because both are live.
    #[tokio::test]
    async fn primary_window_keeps_the_recorded_window_when_two_sessions_are_live() {
        if !TmuxRuntime::available() {
            eprintln!("tmux not available; skipping primary_window liveness test");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        let session = unique_tmux_session("two-live");
        let ctx = test_ctx_with_tmux(&home, session.clone()).await;
        let lane_id = ensure_home(&ctx).await.unwrap().lane_id;
        let tmp = std::env::temp_dir();

        let w1 = ctx
            .backend
            .spawn(lane_id, &SpawnSpec::new("sleep 60", &tmp))
            .expect("spawn window one");
        let w2 = ctx
            .backend
            .spawn(lane_id, &SpawnSpec::new("sleep 60", &tmp))
            .expect("spawn window two");
        ctx.store
            .set_lane_tmux_window(lane_id, Some(w2.clone()))
            .await
            .unwrap();

        let resolved = primary_window(&ctx, lane_id).await.unwrap();
        assert_eq!(
            resolved,
            Some(w2.clone()),
            "both live: the recorded window must win over the earliest"
        );
        assert_eq!(ctx.controller_lane_window().await, Some(w2));

        let _ = w1;
        kill_tmux_session(&session);
    }

    /// No live session in the controller lane at all — the recorded window is stale and nothing
    /// replaces it — resolves to `None`, which is what makes `repomind.status` report
    /// `window: null` and `repomind.instruct` refuse.
    #[tokio::test]
    async fn primary_window_is_none_when_no_session_is_live() {
        if !TmuxRuntime::available() {
            eprintln!("tmux not available; skipping primary_window liveness test");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        let session = unique_tmux_session("none-live");
        let ctx = test_ctx_with_tmux(&home, session.clone()).await;
        let lane_id = ensure_home(&ctx).await.unwrap().lane_id;
        let tmp = std::env::temp_dir();

        let w1 = ctx
            .backend
            .spawn(lane_id, &SpawnSpec::new("sleep 60", &tmp))
            .expect("spawn window one");
        ctx.store
            .set_lane_tmux_window(lane_id, Some(w1.clone()))
            .await
            .unwrap();
        ctx.backend.kill_named(&w1).expect("kill window one");

        let resolved = primary_window(&ctx, lane_id).await.unwrap();
        assert_eq!(resolved, None, "no live session must resolve to None");

        kill_tmux_session(&session);
    }
}
