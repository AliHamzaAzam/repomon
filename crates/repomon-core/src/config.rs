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
    /// Accent color for headers, the selected lane, section dividers, and dirty marks. A named
    /// color (`cyan`, `green`, `magenta`, `amber`, …) or a `#rrggbb`/`#rgb` hex. Unset defaults to
    /// cyan; `"mono"` (or `"none"`/`"off"`) turns all color off for the original monochrome look.
    /// (Status colors — running=green, needs-you=amber, rate-limited=cyan — are fixed.)
    pub accent: Option<String>,
    /// The agent preselected in New Lane (a built-in kind like "claude-code" or a custom
    /// name). `None` falls back to the first listed agent.
    pub default_agent: Option<String>,
    /// Custom agents, keyed by display name -> launch command line. These appear in the
    /// New Lane picker alongside the auto-detected built-ins, e.g.:
    /// `[agents]` then `claude-yolo = "claude --dangerously-skip-permissions"`.
    pub agents: HashMap<String, String>,
    /// Auto-continue managed agents that pause on a usage limit (resume at the reset time).
    /// On by default; a per-lane key (`C`) can disable it for a single lane this session.
    pub auto_continue: bool,
    /// What to type when auto-continuing a rate-limited agent (sent with Enter).
    pub auto_continue_message: String,
    /// Prompt (with a quick agent picker) every time you spawn an agent on a lane (`e`). When
    /// off, `e` spawns the configured default agent immediately.
    pub spawn_prompt: bool,
    /// Master switch for desktop/in-app notifications on agent state changes. When off, no
    /// individual `notify_*` trigger fires.
    pub notify_enabled: bool,
    /// Notify when an agent finishes its turn / is waiting on you (`Running` → `Waiting`).
    pub notify_needs_you: bool,
    /// Notify when an agent pauses on a usage/rate limit.
    pub notify_rate_limited: bool,
    /// Notify when a rate-limited agent is auto-continued and resumes work.
    pub notify_resumed: bool,
    /// Notify when an agent goes idle / its session ends (off by default — can be noisy).
    pub notify_idle: bool,
    /// Play the system notification sound with each desktop notification.
    pub notify_sound: bool,
    /// Include the agent's actual last message (what it said/asked) in notification bodies,
    /// instead of just the original task title.
    pub notify_show_why: bool,
    /// Collapse a burst of simultaneous alerts into one "N agents need attention" popup
    /// (each event still lands individually in the in-app feed).
    pub notify_coalesce: bool,
    /// Make desktop popups click-to-focus the terminal (uses `terminal-notifier` when
    /// installed; falls back to plain popups otherwise).
    pub notify_click_focus: bool,
    /// Per-repo overrides, keyed by repo display name.
    pub repos: HashMap<String, RepoConfig>,
    /// Remote access: the WebSocket JSON-RPC bridge that companion apps (iOS) connect
    /// through. Off by default; `repomon remote enable` fills it in.
    pub remote: RemoteConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            worktree_template: DEFAULT_WORKTREE_TEMPLATE.to_string(),
            socket_path: None,
            time_format: DEFAULT_TIME_FORMAT.to_string(),
            tmux_session: DEFAULT_TMUX_SESSION.to_string(),
            accent: None,
            default_agent: None,
            agents: HashMap::new(),
            auto_continue: true,
            auto_continue_message: "continue".to_string(),
            spawn_prompt: true,
            notify_enabled: true,
            notify_needs_you: true,
            notify_rate_limited: true,
            notify_resumed: true,
            notify_idle: false,
            notify_sound: true,
            notify_show_why: true,
            notify_coalesce: true,
            notify_click_focus: true,
            repos: HashMap::new(),
            remote: RemoteConfig::default(),
        }
    }
}

/// Per-repo configuration overrides.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoConfig {
    pub worktree_template: Option<String>,
}

/// Remote-access (companion app) settings: a WebSocket listener speaking the same JSON-RPC
/// protocol as the Unix socket, gated by a bearer token. Bind it to a private address —
/// typically the machine's Tailscale IP — never the open internet.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteConfig {
    /// Serve the WebSocket bridge.
    pub enabled: bool,
    /// Bind address, e.g. the tailnet IP: `"100.101.102.103:7878"`.
    pub bind: Option<String>,
    /// The bearer token clients must present at the WebSocket handshake.
    pub token: Option<String>,
}

impl Config {
    /// Load config from [`config_path`], returning defaults if the file is absent.
    pub fn load() -> Result<Config> {
        Self::load_from(&config_path())
    }

    /// Load config from a specific file (used by tests; [`load`] wraps this).
    pub fn load_from(path: &std::path::Path) -> Result<Config> {
        match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s).map_err(|e| Error::Config(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// Persist the config to [`config_path`]. See [`save_to`](Self::save_to) for caveats.
    pub fn save(&self) -> Result<()> {
        self.save_to(&config_path())
    }

    /// Persist the config to a specific file, atomically and durably (write a unique temp
    /// file, fsync it, rename over the target, then fsync the directory). NOTE: this rewrites
    /// the whole file via serde, so any hand-added comments are not preserved — repomon owns
    /// the file once you manage agents in-app. `None` options and empty maps are omitted, and
    /// scalars serialize before tables so the output is always valid TOML.
    pub fn save_to(&self, path: &std::path::Path) -> Result<()> {
        use std::io::Write;
        let parent = path.parent();
        if let Some(p) = parent {
            std::fs::create_dir_all(p)?;
        }
        let body = toml::to_string(self).map_err(|e| Error::Config(e.to_string()))?;

        // Unique temp name (pid + nanos) so two writers never collide on a shared temp path;
        // the file is hidden and lives beside the target so the rename stays on one filesystem.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let base = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("config.toml");
        let tmp = path.with_file_name(format!(".{base}.{}.{nanos}.tmp", std::process::id()));

        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?; // flush data to disk before it becomes the live file
        drop(f);
        std::fs::rename(&tmp, path)?;
        // Best-effort: fsync the directory so the rename itself survives a crash.
        if let Some(p) = parent {
            if let Ok(dir) = std::fs::File::open(p) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
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
    fn save_round_trips_agents_and_default() {
        let dir = std::env::temp_dir().join(format!("repomon-cfg-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");

        let mut c = Config {
            tmux_session: "work".into(),
            default_agent: Some("claude-yolo".into()),
            ..Default::default()
        };
        c.agents.insert(
            "claude-yolo".into(),
            "claude --dangerously-skip-permissions".into(),
        );
        c.save_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.default_agent.as_deref(), Some("claude-yolo"));
        assert_eq!(
            loaded.agents.get("claude-yolo").map(String::as_str),
            Some("claude --dangerously-skip-permissions")
        );
        // Unrelated scalar fields survive the round-trip.
        assert_eq!(loaded.tmux_session, "work");
        assert_eq!(loaded.worktree_template, DEFAULT_WORKTREE_TEMPLATE);

        // Clearing the default and removing the agent persists too.
        let mut c2 = loaded;
        c2.default_agent = None;
        c2.agents.clear();
        c2.save_to(&path).unwrap();
        let reloaded = Config::load_from(&path).unwrap();
        assert!(reloaded.default_agent.is_none());
        assert!(reloaded.agents.is_empty());

        // The atomic write leaves no temp files behind.
        let leftover_tmp = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().ends_with(".tmp"));
        assert!(!leftover_tmp, "a .tmp file was left behind after save");

        let _ = std::fs::remove_dir_all(&dir);
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
