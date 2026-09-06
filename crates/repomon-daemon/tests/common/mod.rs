use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use repomon_core::{Config, Store};
use repomon_daemon::Ctx;

static NEXT_NAMESPACE: AtomicU64 = AtomicU64::new(0);

pub struct Fixture {
    pub ctx: Arc<Ctx>,
    _root: tempfile::TempDir,
}

impl Fixture {
    pub fn new(store: Store, config: Config, db: Option<PathBuf>) -> Self {
        Self::with_paths(store, config, db, None, None)
    }

    pub fn with_paths(
        store: Store,
        mut config: Config,
        db: Option<PathBuf>,
        config_path: Option<PathBuf>,
        notes: Option<PathBuf>,
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        let defaults = Config::default();
        if config.tmux_session == defaults.tmux_session {
            config.tmux_session = format!(
                "fixture-{}-{}",
                std::process::id(),
                NEXT_NAMESPACE.fetch_add(1, Ordering::Relaxed)
            );
        }
        if config.repomind.home == defaults.repomind.home {
            config.repomind.home = root.path().join("repomind").to_string_lossy().into_owned();
        }
        if config.repomind.basic_memory_config.is_none() {
            config.repomind.basic_memory_config = Some(
                root.path()
                    .join("basic-memory.json")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        let ctx = Ctx::new_with_paths(
            store,
            config,
            db,
            config_path.unwrap_or_else(|| root.path().join("config.toml")),
            notes.unwrap_or_else(|| root.path().join("notes")),
        );
        Self { ctx, _root: root }
    }
}
