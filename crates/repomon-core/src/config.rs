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

/// A tmux session name is safe to use as the `-L <label>` socket and in `session:window` targets.
/// It's injected into many tmux command args, so restrict it to `[A-Za-z0-9_-]` — a name with a
/// `:`, `=`, whitespace, or other metachar would corrupt target resolution (`exact_target` builds
/// `{session}:={window}`). Empty is invalid.
pub fn valid_tmux_session(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
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
    /// Notify when a worktree-isolated *subagent* finishes (an inferred file-activity session,
    /// e.g. a Claude Code subagent that leaves no transcript or process of its own). Off by
    /// default: you're alerted only when the *main* agent finishes, not each subagent it spawns.
    /// Turn on to get a popup for every subagent too.
    pub notify_subagents: bool,
    /// Per-repo overrides, keyed by repo display name.
    pub repos: HashMap<String, RepoConfig>,
    /// Remote access: the WebSocket JSON-RPC bridge that companion apps (iOS) connect
    /// through. Off by default; `repomon remote enable` fills it in.
    pub remote: RemoteConfig,
    /// APNs push for the iOS companion: alerts reach the phone even with the app closed.
    pub push: PushConfig,
    /// Show Claude account usage (the `/usage` 5-hour + weekly windows) in the TUI's bottom-right
    /// corner. Off by default: subscription usage has no CLI/file/endpoint, so this works by
    /// running `/usage` in a hidden throwaway `claude` session per account every few minutes —
    /// which spawns a background process and writes a tiny transcript. See `docs/agents.md`.
    pub usage_probe: bool,
    /// In the sidebars, expand a lane running several agents into one row per agent (a tree under
    /// the lane) instead of a single row with an `×N` badge. Off by default.
    pub expand_agents: bool,
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
            notify_subagents: false,
            repos: HashMap::new(),
            remote: RemoteConfig::default(),
            push: PushConfig::default(),
            usage_probe: false,
            expand_agents: false,
        }
    }
}

/// Per-repo configuration overrides.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoConfig {
    pub worktree_template: Option<String>,
}

/// APNs (Apple push) credentials for the iOS companion. The daemon sends pushes directly to
/// Apple over HTTP/2 using a `.p8` signing key from the Apple Developer account — keep the key
/// file beside the config (e.g. `~/.config/repomon/AuthKey_XXXX.p8`), never in a repo. Push is
/// active only when every field is set and at least one device has registered.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PushConfig {
    /// Apple Developer team id (10 chars).
    pub team_id: Option<String>,
    /// The key id of the `.p8` APNs auth key.
    pub key_id: Option<String>,
    /// Path to the `.p8` key file.
    pub p8_path: Option<PathBuf>,
    /// The app's bundle id (the APNs topic), e.g. `com.azaleas.repomon`.
    pub bundle_id: Option<String>,
    /// Use the APNs sandbox endpoint (Xcode/development builds) instead of production.
    pub sandbox: bool,
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
            Ok(s) => {
                let mut cfg: Config = toml::from_str(&s).map_err(|e| Error::Config(e.to_string()))?;
                // A malformed tmux session name would corrupt every tmux target it's spliced into;
                // reset to the default rather than fail the daemon over a bad config char.
                if !valid_tmux_session(&cfg.tmux_session) {
                    tracing::warn!(
                        "invalid tmux_session {:?} in config; using {:?}",
                        cfg.tmux_session,
                        DEFAULT_TMUX_SESSION
                    );
                    cfg.tmux_session = DEFAULT_TMUX_SESSION.to_string();
                }
                Ok(cfg)
            }
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

/// The platform data directory for the SQLite database. `REPOMON_DATA_DIR` overrides it — handy
/// for tests and for running an isolated second instance (its own DB) alongside the real daemon.
pub fn data_dir() -> PathBuf {
    if let Ok(x) = std::env::var("REPOMON_DATA_DIR") {
        if !x.is_empty() {
            return PathBuf::from(x);
        }
    }
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
    fn tmux_session_name_validation() {
        assert!(valid_tmux_session("repomon"));
        assert!(valid_tmux_session("work-2_b"));
        assert!(!valid_tmux_session("")); // empty
        assert!(!valid_tmux_session("a:b")); // colon corrupts session:window targets
        assert!(!valid_tmux_session("a b")); // whitespace
        assert!(!valid_tmux_session("a=b"));
        assert!(!valid_tmux_session("a;rm -rf"));
    }

    #[test]
    fn invalid_tmux_session_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "tmux_session = \"bad:name\"\n").unwrap();
        let c = Config::load_from(&path).unwrap();
        assert_eq!(c.tmux_session, DEFAULT_TMUX_SESSION);
    }

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

    #[test]
    fn data_dir_respects_env_override() {
        // SAFETY: single-threaded test; nothing else reads the environment here.
        unsafe { std::env::set_var("REPOMON_DATA_DIR", "/tmp/repomon-data-override-test") };
        assert_eq!(data_dir(), PathBuf::from("/tmp/repomon-data-override-test"));
        // SAFETY: single-threaded test; nothing else reads the environment here.
        unsafe { std::env::remove_var("REPOMON_DATA_DIR") };
    }
}
