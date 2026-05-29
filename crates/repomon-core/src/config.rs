//! Configuration and on-disk path resolution.
//!
//! Config follows XDG on every platform (so the file lives at
//! `~/.config/repomon/config.toml` on macOS too, for portability). Data follows platform
//! conventions (`~/.local/share/repomon` on Linux, `~/Library/Application Support/repomon`
//! on macOS). The socket path matches the build spec: `/tmp/repomon-$USER.sock` on macOS,
//! `$XDG_RUNTIME_DIR/repomon.sock` on Linux.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const DEFAULT_WORKTREE_TEMPLATE: &str = "~/code/{repo}-wt/{branch}";
pub const DEFAULT_TMUX_SESSION: &str = "repomon";
pub const DEFAULT_TIME_FORMAT: &str = "%H:%M %a %d %b %Y";

/// Top-level user configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Worktree path template. `{repo}` and `{branch}` are substituted; `/` in a branch
    /// becomes `-` in the path.
    pub worktree_template: String,
    /// Override for the daemon socket path.
    pub socket_path: Option<PathBuf>,
    /// strftime format for the header clock.
    pub time_format: String,
    /// The tmux session repomon manages agents in.
    pub tmux_session: String,
    /// Per-repo overrides, keyed by repo display name.
    pub repos: HashMap<String, RepoConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            worktree_template: DEFAULT_WORKTREE_TEMPLATE.to_string(),
            socket_path: None,
            time_format: DEFAULT_TIME_FORMAT.to_string(),
            tmux_session: DEFAULT_TMUX_SESSION.to_string(),
            repos: HashMap::new(),
        }
    }
}

/// Per-repo configuration overrides.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoConfig {
    pub worktree_template: Option<String>,
}

impl Config {
    /// Load config from [`config_path`], returning defaults if the file is absent.
    pub fn load() -> Result<Config> {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).map_err(|e| Error::Config(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// The worktree template for a given repo, honoring per-repo overrides.
    pub fn worktree_template_for(&self, repo_name: &str) -> &str {
        self.repos
            .get(repo_name)
            .and_then(|r| r.worktree_template.as_deref())
            .unwrap_or(&self.worktree_template)
    }
}

fn home() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The XDG-style config directory (`~/.config/repomon` on every platform).
pub fn config_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("repomon");
        }
    }
    home().join(".config").join("repomon")
}

/// Path to `config.toml`.
pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// The platform data directory for the SQLite database.
pub fn data_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "repomon")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| home().join(".local").join("share").join("repomon"))
}

/// Path to the SQLite database.
pub fn db_path() -> PathBuf {
    data_dir().join("repomon.db")
}

/// The daemon socket path, honoring an explicit config override.
pub fn socket_path(cfg: &Config) -> PathBuf {
    if let Some(p) = &cfg.socket_path {
        return p.clone();
    }
    default_socket_path()
}

fn current_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "user".to_string())
}

#[cfg(target_os = "macos")]
fn default_socket_path() -> PathBuf {
    PathBuf::from(format!("/tmp/repomon-{}.sock", current_user()))
}

#[cfg(not(target_os = "macos"))]
fn default_socket_path() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_RUNTIME_DIR") {
        if !x.is_empty() {
            return PathBuf::from(x).join("repomon.sock");
        }
    }
    std::env::temp_dir().join(format!("repomon-{}.sock", current_user()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert_eq!(c.worktree_template, DEFAULT_WORKTREE_TEMPLATE);
        assert_eq!(c.tmux_session, "repomon");
        assert!(c.socket_path.is_none());
    }

    #[test]
    fn per_repo_template_override() {
        let mut c = Config::default();
        c.repos.insert(
            "pos-saas".into(),
            RepoConfig {
                worktree_template: Some("~/wt/{branch}".into()),
            },
        );
        assert_eq!(c.worktree_template_for("pos-saas"), "~/wt/{branch}");
        assert_eq!(c.worktree_template_for("other"), DEFAULT_WORKTREE_TEMPLATE);
    }

    #[test]
    fn parses_partial_toml() {
        let c: Config = toml::from_str("tmux_session = \"work\"\n").unwrap();
        assert_eq!(c.tmux_session, "work");
        // Unspecified fields fall back to defaults.
        assert_eq!(c.worktree_template, DEFAULT_WORKTREE_TEMPLATE);
    }

    #[test]
    fn socket_path_respects_override() {
        let c = Config {
            socket_path: Some(PathBuf::from("/tmp/custom.sock")),
            ..Default::default()
        };
        assert_eq!(socket_path(&c), PathBuf::from("/tmp/custom.sock"));
    }
}
