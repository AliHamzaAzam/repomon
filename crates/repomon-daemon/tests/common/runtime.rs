//! Test-only context construction. Socket paths are private even when ambient config is production.
use repomon_core::{Config, Store};
use repomon_daemon::Ctx;
use std::path::PathBuf;
use std::sync::Arc;

pub struct TestCtx;

#[allow(dead_code)]
impl TestCtx {
    pub fn create(store: Store, config: Config, db: Option<PathBuf>) -> Arc<Ctx> {
        Self::build(store, config, db, None, None)
    }
    pub fn new_with_config_path(
        store: Store,
        config: Config,
        db: Option<PathBuf>,
        path: PathBuf,
    ) -> Arc<Ctx> {
        Self::build(store, config, db, Some(path), None)
    }
    pub fn new_with_paths(
        store: Store,
        config: Config,
        db: Option<PathBuf>,
        path: PathBuf,
        notes: PathBuf,
    ) -> Arc<Ctx> {
        Self::build(store, config, db, Some(path), Some(notes))
    }
    fn build(
        store: Store,
        mut config: Config,
        db: Option<PathBuf>,
        path: Option<PathBuf>,
        notes: Option<PathBuf>,
    ) -> Arc<Ctx> {
        #[cfg(unix)]
        {
            let backend = Arc::new(repomon_core::TmuxRuntime::isolated(
                config.tmux_session.clone(),
            ));
            let root = backend.socket_path().parent().unwrap().to_path_buf();
            let defaults = Config::default();
            if config.repomind.home == defaults.repomind.home {
                config.repomind.home = root.join("repomind").to_string_lossy().into_owned();
            }
            if config.repomind.basic_memory_config.is_none() {
                config.repomind.basic_memory_config = Some(
                    root.join("basic-memory.json")
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            Ctx::new_with_backend(
                store,
                config,
                db,
                path.unwrap_or_else(|| root.join("config.toml")),
                notes.unwrap_or_else(|| root.join("notes")),
                backend,
            )
        }
        #[cfg(not(unix))]
        {
            let _ = &mut config;
            Ctx::new_with_paths(
                store,
                config,
                db,
                path.unwrap_or_else(repomon_core::config::config_path),
                notes.unwrap_or_else(|| repomon_core::config::data_dir().join("notes")),
            )
        }
    }
}
