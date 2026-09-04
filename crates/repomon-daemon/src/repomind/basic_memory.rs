//! Registering the repomind home as a basic-memory project.
//!
//! Claude controllers already carry basic-memory's `search_notes` / `read_note` / `write_note`
//! for the operator's mnemind vault. Adding the home as a second project means they reach the
//! fleet's own memory with the same tools instead of a bespoke path.
//!
//! The daemon is a guest in that config file. It only ever adds the one project it owns, by
//! asking the CLI to do it; it never rewrites the file itself, never changes the default
//! project, and never removes anything.

use std::path::{Path, PathBuf};

/// The basic-memory project name the home is registered under.
pub const PROJECT: &str = "repomind";

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

/// `~/.basic-memory/config.json`, the file the CLI keeps its project list in.
pub fn config_path() -> PathBuf {
    repomon_core::config::home()
        .join(".basic-memory")
        .join("config.json")
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
    let home = ctx.config.read().await.repomind_home();
    let decision = tokio::task::spawn_blocking(move || {
        ensure_project_with(&config_path(), &home, cli_present(), |argv| {
            let out = std::process::Command::new("basic-memory").args(argv).output()?;
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
        assert!(!already_registered(&config(dir.path(), "not json"), PROJECT));
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
