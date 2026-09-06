//! Registers the owned repomind project through the basic-memory CLI without rewriting
//! configuration, changing defaults, or removing other projects.

use std::path::{Path, PathBuf};

/// The basic-memory project name the home is registered under.
pub const PROJECT: &str = "repomind";

/// basic-memory's own name for its config file inside the data directory.
pub const CONFIG_FILE_NAME: &str = "config.json";

/// Isolates both CLI and daemon memory configuration from the operator’s account.
pub const CONFIG_DIR_ENV: &str = "BASIC_MEMORY_CONFIG_DIR";

/// What one registration pass decided. Returned so the caller logs a single line and a test can
/// assert the decision without a basic-memory install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Registration {
    /// basic-memory is not on PATH, so there is nothing to register with.
    CliMissing,
    /// The config already lists the project. Nothing to do, whatever path it points at: an
    /// operator who repointed it meant to.
    AlreadyRegistered,
    /// `basic-memory project add <PROJECT> <home>` ran.
    Added,
}

/// The file the CLI keeps its project list in, honoring both overrides in basic-memory's own
/// order: [`CONFIG_DIR_ENV`] first, then the `[repomind] basic_memory_config` path the caller
/// passes in, then `~/.basic-memory/config.json`.
pub fn resolve_config_path(env_dir: Option<PathBuf>, configured: Option<&Path>) -> PathBuf {
    if let Some(dir) = env_dir {
        return dir.join(CONFIG_FILE_NAME);
    }
    if let Some(path) = configured {
        return path.to_path_buf();
    }
    repomon_core::config::home()
        .join(".basic-memory")
        .join(CONFIG_FILE_NAME)
}

/// [`resolve_config_path`] reading [`CONFIG_DIR_ENV`] from the daemon's own environment.
pub fn config_path(configured: Option<&Path>) -> PathBuf {
    let env_dir = std::env::var_os(CONFIG_DIR_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    resolve_config_path(env_dir, configured)
}

/// The environment entry that points a child `basic-memory` at the same config file the daemon
/// resolved. basic-memory takes a *directory*, so a configured path's parent is what it gets.
pub fn cli_config_dir_env(config_path: &Path) -> (String, String) {
    let dir = config_path.parent().unwrap_or(Path::new("."));
    (
        CONFIG_DIR_ENV.to_string(),
        dir.to_string_lossy().into_owned(),
    )
}

/// Whether `config_path` already lists `PROJECT`. A missing or unparseable config counts as not
/// registered: the CLI creates and repairs its own config when it adds a project.
pub fn already_registered(config_path: &Path, project: &str) -> bool {
    let Ok(body) = std::fs::read_to_string(config_path) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("projects")?.get(project).cloned())
        .is_some()
}

/// Whether the basic-memory CLI is on PATH.
pub fn cli_present() -> bool {
    which_on_path("basic-memory")
}

/// A `which`-style PATH probe. `std::process::Command` would find the binary too, but running it
/// just to see whether it exists is a process spawn on every daemon start.
fn which_on_path(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(bin);
        candidate.is_file() || candidate.with_extension("exe").is_file()
    })
}

/// Decide and act: register the home unless the CLI is missing or the project is already there.
/// `run_add` receives the argv to run and reports whether it succeeded, so the decision is
/// testable without a basic-memory install.
pub fn ensure_project_with(
    config_path: &Path,
    home: &Path,
    cli_present: bool,
    run_add: impl FnOnce(&[String]) -> std::io::Result<bool>,
) -> std::io::Result<Registration> {
    if !cli_present {
        return Ok(Registration::CliMissing);
    }
    if already_registered(config_path, PROJECT) {
        return Ok(Registration::AlreadyRegistered);
    }
    let argv = vec![
        "project".to_string(),
        "add".to_string(),
        PROJECT.to_string(),
        home.to_string_lossy().into_owned(),
    ];
    if !run_add(&argv)? {
        return Err(std::io::Error::other(
            "basic-memory project add failed; see the daemon log",
        ));
    }
    Ok(Registration::Added)
}

/// The daemon's pass: probe PATH, read the real config, and shell out to the CLI when the home
/// is not registered yet. Logs one line and never fails the caller's startup.
pub async fn ensure_project(ctx: &crate::Ctx) -> repomon_core::Result<Registration> {
    let (home, configured) = {
        let cfg = ctx.config.read().await;
        (cfg.repomind_home(), cfg.repomind_basic_memory_config())
    };
    let decision = tokio::task::spawn_blocking(move || {
        let path = config_path(configured.as_deref());
        let (env_key, env_dir) = cli_config_dir_env(&path);
        ensure_project_with(&path, &home, cli_present(), |argv| {
            let out = std::process::Command::new("basic-memory")
                .args(argv)
                .env(&env_key, &env_dir)
                .output()?;
            if !out.status.success() {
                tracing::warn!(
                    "basic-memory {}: {}",
                    argv.join(" "),
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            Ok(out.status.success())
        })
    })
    .await
    .map_err(|e| repomon_core::Error::Other(e.to_string()))?
    .map_err(repomon_core::Error::Io)?;

    match decision {
        Registration::Added => {
            tracing::info!("basic-memory: registered project {PROJECT} at the repomind home");
        }
        Registration::CliMissing => {
            tracing::debug!("basic-memory: CLI not on PATH; skipping project registration");
        }
        Registration::AlreadyRegistered => {}
    }
    Ok(decision)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn config(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("config.json");
        std::fs::write(&path, body).unwrap();
        path
    }

    /// basic-memory's own documented override (`BASIC_MEMORY_CONFIG_DIR`, `resolve_data_dir`)
    /// wins over everything: an isolated daemon that exports it gets the same config file the
    /// CLI it shells out to will use.
    #[test]
    fn the_basic_memory_config_dir_environment_override_wins() {
        assert_eq!(
            resolve_config_path(
                Some(PathBuf::from("/tmp/iso/bm")),
                Some(Path::new("/cfg.json"))
            ),
            PathBuf::from("/tmp/iso/bm").join(CONFIG_FILE_NAME)
        );
    }

    #[test]
    fn the_configured_path_is_used_when_the_environment_is_unset() {
        assert_eq!(
            resolve_config_path(None, Some(Path::new("/tmp/iso/bm/config.json"))),
            PathBuf::from("/tmp/iso/bm/config.json")
        );
    }

    #[test]
    fn without_an_override_the_operators_own_config_file_is_used() {
        assert_eq!(
            resolve_config_path(None, None),
            repomon_core::config::home()
                .join(".basic-memory")
                .join(CONFIG_FILE_NAME)
        );
    }

    /// The CLI reads its own config through `BASIC_MEMORY_CONFIG_DIR`, so an override that came
    /// from the daemon's config file has to be handed to the child as that variable, or
    /// `project add` would write the operator's real config while we read the isolated one.
    #[test]
    fn the_child_cli_is_pointed_at_the_same_config_directory() {
        assert_eq!(
            cli_config_dir_env(Path::new("/tmp/iso/bm/config.json")),
            (
                "BASIC_MEMORY_CONFIG_DIR".to_string(),
                "/tmp/iso/bm".to_string()
            )
        );
    }

    #[test]
    fn a_config_listing_the_project_counts_as_registered() {
        let dir = tempfile::tempdir().unwrap();
        let path = config(
            dir.path(),
            r#"{"projects":{"main":"/m","repomind":"/r"},"default_project":"main"}"#,
        );
        assert!(already_registered(&path, PROJECT));
    }

    #[test]
    fn a_config_without_the_project_is_not_registered() {
        let dir = tempfile::tempdir().unwrap();
        let path = config(
            dir.path(),
            r#"{"projects":{"main":"/m"},"default_project":"main"}"#,
        );
        assert!(!already_registered(&path, PROJECT));
    }

    #[test]
    fn a_missing_or_broken_config_is_not_registered() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!already_registered(&dir.path().join("nope.json"), PROJECT));
        assert!(!already_registered(
            &config(dir.path(), "not json"),
            PROJECT
        ));
    }

    #[test]
    fn a_missing_cli_registers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = config(dir.path(), r#"{"projects":{}}"#);
        let ran = RefCell::new(Vec::<String>::new());

        let decision = ensure_project_with(&path, Path::new("/home/repomind"), false, |argv| {
            ran.borrow_mut().extend_from_slice(argv);
            Ok(true)
        })
        .unwrap();

        assert_eq!(decision, Registration::CliMissing);
        assert!(ran.borrow().is_empty());
    }

    #[test]
    fn an_absent_project_is_added_with_the_home_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = config(dir.path(), r#"{"projects":{"main":"/m"}}"#);
        let ran = RefCell::new(Vec::<String>::new());

        let decision = ensure_project_with(&path, Path::new("/home/repomind"), true, |argv| {
            ran.borrow_mut().extend_from_slice(argv);
            Ok(true)
        })
        .unwrap();

        assert_eq!(decision, Registration::Added);
        assert_eq!(
            *ran.borrow(),
            vec![
                "project".to_string(),
                "add".to_string(),
                "repomind".to_string(),
                "/home/repomind".to_string(),
            ]
        );
    }

    #[test]
    fn an_already_registered_project_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = config(dir.path(), r#"{"projects":{"repomind":"/elsewhere"}}"#);
        let ran = RefCell::new(false);

        let decision = ensure_project_with(&path, Path::new("/home/repomind"), true, |_| {
            *ran.borrow_mut() = true;
            Ok(true)
        })
        .unwrap();

        assert_eq!(decision, Registration::AlreadyRegistered);
        assert!(!*ran.borrow(), "a repointed project is the operator's call");
    }

    #[test]
    fn the_operators_other_projects_and_default_are_never_touched() {
        let dir = tempfile::tempdir().unwrap();
        let before = r#"{"projects":{"main":"/m","mnemind":"/n"},"default_project":"mnemind"}"#;
        let path = config(dir.path(), before);

        ensure_project_with(&path, Path::new("/home/repomind"), true, |_| Ok(true)).unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "the daemon must never rewrite the config itself"
        );
    }
}
