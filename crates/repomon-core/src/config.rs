//! Resolves XDG-style configuration, platform data directories, and Unix socket or Windows
//! named-pipe endpoints.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const DEFAULT_WORKTREE_TEMPLATE: &str = "~/code/{repo}-wt/{branch}";
pub const DEFAULT_TMUX_SESSION: &str = "repomon";

/// Accepts only a nonempty alphanumeric, underscore, or dash tmux session name so socket labels and
/// target syntax remain unambiguous.
pub fn valid_tmux_session(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
pub const DEFAULT_TIME_FORMAT: &str = "%H:%M %a %d %b %Y";

/// Where the repomind home repo lives by default. A `~/`-prefixed path in the config file;
/// [`expand_tilde`] turns it into an absolute path everywhere else.
pub const DEFAULT_REPOMIND_HOME: &str = "~/repomind";

/// The default token budget for the assembled boot context. The design spec's "about 12k".
pub const DEFAULT_BOOT_BUDGET_TOKENS: usize = 12_000;

/// How many controller agents may run in the repomind lane at once by default.
pub const DEFAULT_MAX_CONTROLLERS: usize = 2;

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
    /// Selects a named or hexadecimal TUI accent, defaulting to cyan, with mono/none/off disabling
    /// colors.
    pub accent: Option<String>,
    /// Theme preset for the GUI ("system", "dark", "midnight", "nord", "dracula", "sepia", "light").
    pub theme: Option<String>,
    /// The agent preselected in New Lane (a built-in kind like "claude-code" or a custom
    /// name). `None` falls back to the first listed agent.
    pub default_agent: Option<String>,
    /// Custom agents, keyed by display name -> launch command line. These appear in the
    /// New Lane picker alongside the auto-detected built-ins, e.g.:
    /// `[agents]` then `claude-yolo = "claude --dangerously-skip-permissions"`.
    pub agents: HashMap<String, String>,
    /// Custom icon assignment for agent kinds or custom agent names, mapping agent name -> icon key
    /// (e.g. "my-agent" -> "compass", "codex" -> "bolt").
    pub agent_icons: HashMap<String, String>,
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
    /// Notify when an agent goes idle / its session ends (off by default - can be noisy).
    pub notify_idle: bool,
    /// Master sound switch. The desktop uses it for custom cues and the daemon uses it for its
    /// native fallback when no GUI covers an event.
    pub notify_sound: bool,
    /// Desktop cue volume in the inclusive 0.0 to 1.0 range.
    pub notify_sound_volume: f32,
    /// Play desktop cues only while the Mission Control window is unfocused.
    pub notify_sound_unfocused_only: bool,
    /// Play the desktop cue for an agent permission or decision request.
    pub notify_sound_agent_needs_you: bool,
    /// Play the desktop cue when an agent finishes a turn or becomes idle.
    pub notify_sound_agent_finished: bool,
    /// Play the desktop cue when repomind changes from no attention to needing attention.
    pub notify_sound_repomind_needs_you: bool,
    /// Play the desktop cue when an agent stalls or reaches a rate limit.
    pub notify_sound_error_or_stall: bool,
    /// Play the desktop cue for newly stored fleet mail.
    pub notify_sound_incoming_message: bool,
    /// Play the desktop cue when a newly discovered update version is ready.
    pub notify_sound_update_ready: bool,
    /// Allow one managed agent to receive durable mail by terminal injection from another agent.
    /// Storage and inbox access are unaffected.
    pub message_inject_agents: bool,
    /// Allow operator and repomind mail to use safe terminal injection when the recipient is idle.
    /// Storage and inbox access are unaffected.
    pub message_inject_operator: bool,
    /// Exact canonical addresses, or `lane-<id>/*` patterns, whose replies refresh the fleet-mail
    /// thread hop budget like the human operator. This is for explicitly designated,
    /// human-supervised coordinator sessions; ordinary agent-to-agent threads still exhaust.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_hop_refresh_senders: Vec<String>,
    /// Include the agent's actual last message (what it said/asked) in notification bodies,
    /// instead of just the original task title.
    pub notify_show_why: bool,
    /// Collapse a burst of simultaneous alerts into one "N agents need attention" popup
    /// (each event still lands individually in the in-app feed).
    pub notify_coalesce: bool,
    /// Make desktop popups click-to-focus the terminal (uses `terminal-notifier` when
    /// installed; falls back to plain popups otherwise).
    pub notify_click_focus: bool,
    /// Allows daemon OS notifications when no UI covers delivery; disabling this silences alerts
    /// when neither desktop nor TUI is open.
    pub notify_desktop_fallback: bool,
    /// Enables notifications for inferred worktree subagents, disabled by default to avoid alerting
    /// for every delegated task.
    pub notify_subagents: bool,
    /// Per-repo overrides, keyed by repo display name.
    pub repos: HashMap<String, RepoConfig>,
    /// Remote access: the WebSocket JSON-RPC bridge that companion apps (iOS) connect
    /// through. Off by default; `repomon remote enable` fills it in.
    pub remote: RemoteConfig,
    /// APNs push for the iOS companion: alerts reach the phone even with the app closed.
    pub push: PushConfig,
    /// Enables hidden CLI usage probes whose background processes and transcript writes make this
    /// opt-in.
    pub usage_probe: bool,
    /// In the sidebars, expand a lane running several agents into one row per agent (a tree under
    /// the lane) instead of a single row with an `×N` badge. Off by default.
    pub expand_agents: bool,
    /// Keeps the legacy repository-activity sort flag synchronized with sort_mode for clients that
    /// still read the boolean.
    pub sort_repos_by_activity: bool,
    /// Selects sidebar repository order, falling back to sort_repos_by_activity when unset.
    pub sort_mode: Option<SortMode>,
    /// Orders agents within a lane independently of repository sorting, defaulting to Activity when
    /// unset.
    pub tab_sort_mode: Option<TabSortMode>,
    /// Selects a Claude account, Claude-compatible custom command, Codex, Antigravity, or OpenCode
    /// for orchestration, with per-start overrides taking precedence.
    pub orchestrator_agent: Option<String>,
    /// The model the orchestrator session runs (e.g. `opus`, `sonnet`). `None` lets `claude` pick
    /// its default. An explicit override on `orchestrator.start` takes precedence.
    pub orchestrator_model: Option<String>,
    /// Enables byte-stream terminal rendering with capture-based fallback.
    pub embedded_pty: bool,
    /// Wall-clock limit for one headless standing/triage orchestration run, in seconds.
    pub standing_timeout_secs: u64,
    /// Minutes an agent may sit in needs-you with no UI attached before the daemon fires a
    /// bounded triage orchestration (context + recommendation in the push). `None` disables the
    /// trigger (the default): an unattended run costs real tokens and must be opted into.
    pub triage_after_mins: Option<u64>,
    /// Agent supervision configuration (autonomous auto-approval and intervention policies).
    #[serde(default)]
    pub supervision: crate::agent::supervision::SupervisionConfig,
    /// The repomind home repo and its controller lane. Serialized last (after every scalar) so
    /// the emitted TOML stays valid.
    #[serde(default)]
    pub repomind: RepomindConfig,
    /// The `[usage]` table: the token ledger, its scan cadence and its price corrections.
    /// Serialized after [`RepomindConfig`] for the same reason: TOML wants every scalar first.
    #[serde(default)]
    pub usage: UsageConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            worktree_template: DEFAULT_WORKTREE_TEMPLATE.to_string(),
            socket_path: None,
            time_format: DEFAULT_TIME_FORMAT.to_string(),
            tmux_session: DEFAULT_TMUX_SESSION.to_string(),
            accent: None,
            theme: None,
            default_agent: None,
            agents: HashMap::new(),
            agent_icons: HashMap::new(),
            auto_continue: true,
            auto_continue_message: "continue".to_string(),
            spawn_prompt: true,
            notify_enabled: true,
            notify_needs_you: true,
            notify_rate_limited: true,
            notify_resumed: true,
            notify_idle: false,
            notify_sound: true,
            notify_sound_volume: 0.25,
            notify_sound_unfocused_only: true,
            notify_sound_agent_needs_you: true,
            notify_sound_agent_finished: true,
            notify_sound_repomind_needs_you: true,
            notify_sound_error_or_stall: true,
            notify_sound_incoming_message: true,
            notify_sound_update_ready: true,
            message_inject_agents: false,
            message_inject_operator: true,
            message_hop_refresh_senders: Vec::new(),
            notify_show_why: true,
            notify_coalesce: true,
            notify_click_focus: true,
            notify_desktop_fallback: true,
            notify_subagents: false,
            repos: HashMap::new(),
            remote: RemoteConfig::default(),
            push: PushConfig::default(),
            usage_probe: false,
            expand_agents: false,
            sort_repos_by_activity: false,
            sort_mode: None,
            tab_sort_mode: None,
            orchestrator_agent: None,
            orchestrator_model: None,
            embedded_pty: true,
            standing_timeout_secs: 600,
            triage_after_mins: None,
            supervision: crate::agent::supervision::SupervisionConfig::default(),
            repomind: RepomindConfig::default(),
            usage: UsageConfig::default(),
        }
    }
}

/// The `[repomind]` table: where the repomind home repo lives, which agent runs as its primary
/// controller, and how many controllers may share its lane.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RepomindConfig {
    /// The home repo path, `~/`-expandable. See [`Config::repomind_home`].
    pub home: String,
    /// The agent that `orchestrator.start` spawns as the primary controller. `None` falls back
    /// to [`Config::orchestrator_agent`], so an operator who already picked an orchestrator
    /// agent does not have to pick it twice.
    pub primary_agent: Option<String>,
    /// How many controller agents may run in the controller lane at once.
    pub max_controllers: usize,
    /// Selects the basic-memory configuration for project registration, unless
    /// BASIC_MEMORY_CONFIG_DIR overrides it.
    pub basic_memory_config: Option<String>,
    /// The hard token budget for the assembled boot context (`.repomind/boot.md`). Estimated at
    /// four characters to a token; over budget, the journal is dropped first, then profile
    /// notes, then plans, and the document names what it cut.
    pub boot_budget_tokens: usize,
}

impl Default for RepomindConfig {
    fn default() -> Self {
        RepomindConfig {
            home: DEFAULT_REPOMIND_HOME.to_string(),
            primary_agent: None,
            max_controllers: DEFAULT_MAX_CONTROLLERS,
            basic_memory_config: None,
            boot_budget_tokens: DEFAULT_BOOT_BUDGET_TOKENS,
        }
    }
}

/// How often a full ingest scan runs when nothing has changed on disk.
pub const DEFAULT_USAGE_SCAN_INTERVAL_SECS: u64 = 600;
/// How many source files one ingest pass reads, so a first run over years of transcripts does
/// not hold the store thread for minutes.
pub const DEFAULT_USAGE_MAX_FILES_PER_SCAN: usize = 200;
/// Where LiteLLM publishes its price snapshot.
pub const DEFAULT_USAGE_PRICE_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

/// The `[usage]` table: the token ledger's switches, cadence and price corrections.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageConfig {
    /// Whether the daemon ingests agent transcripts into the ledger at all.
    pub enabled: bool,
    /// Seconds between full scans. Watchers still pick up changes as they happen; this is the
    /// floor that catches anything a watcher missed.
    pub scan_interval_secs: u64,
    /// How many source files one pass reads, bounding the work per tick.
    pub max_files_per_scan: usize,
    /// Enables daily price refreshes, with built-in rates as fallback and operator overrides taking
    /// precedence.
    pub refresh_prices: bool,
    /// Where to refresh prices from, when `refresh_prices` is on.
    pub price_url: Option<String>,
    /// Per-model corrections to the built-in rates, keyed by model id or family prefix.
    pub price_overrides: HashMap<String, crate::pricing::PriceOverride>,
}

impl Default for UsageConfig {
    fn default() -> Self {
        UsageConfig {
            enabled: true,
            scan_interval_secs: DEFAULT_USAGE_SCAN_INTERVAL_SECS,
            max_files_per_scan: DEFAULT_USAGE_MAX_FILES_PER_SCAN,
            refresh_prices: true,
            price_url: None,
            price_overrides: HashMap::new(),
        }
    }
}

/// Per-repo configuration overrides.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoConfig {
    pub worktree_template: Option<String>,
}

/// Configures APNs delivery when credentials and device registrations exist, referencing a private
/// signing-key file outside repositories.
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
/// protocol as the Unix socket, gated by a bearer token. Bind it to a private address -
/// typically the machine's Tailscale IP - never the open internet.
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

/// How sidebar repo groups are ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortMode {
    /// The daemon's default order (manual positions where set, then name).
    Default,
    /// Orders repositories by most recent lane activity.
    Activity,
    /// Pure manual order: exactly the positions persisted by `repo.reorder`, no auto-sorting.
    Manual,
}

/// Selects the order of agent tabs within a lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabSortMode {
    /// Orders the most recently active agents first.
    Activity,
    /// The persisted per-lane tab order (`agent.set_tab_order`), no auto-sorting; newly seen
    /// agents append.
    Manual,
}

/// Resolves an explicit tab order or defaults to activity.
pub fn resolve_tab_sort_mode(mode: Option<TabSortMode>) -> TabSortMode {
    mode.unwrap_or(TabSortMode::Activity)
}

/// Resolves repository ordering with an explicit sort_mode taking precedence over the boolean
/// fallback.
pub fn resolve_sort_mode(mode: Option<SortMode>, legacy_sort_by_activity: bool) -> SortMode {
    match mode {
        Some(m) => m,
        None if legacy_sort_by_activity => SortMode::Activity,
        None => SortMode::Default,
    }
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
                let mut cfg: Config =
                    toml::from_str(&s).map_err(|e| Error::Config(e.to_string()))?;
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

    /// Atomically and durably serializes the complete configuration, omitting empty options and
    /// maps and replacing any hand-written comments.
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

    /// Resolves sidebar repository order, falling back to the legacy boolean when sort_mode is
    /// unset.
    pub fn sort_mode(&self) -> SortMode {
        resolve_sort_mode(self.sort_mode, self.sort_repos_by_activity)
    }

    /// The effective per-lane agent tab sort mode.
    pub fn tab_sort_mode(&self) -> TabSortMode {
        resolve_tab_sort_mode(self.tab_sort_mode)
    }

    /// The repomind home as an absolute path (a leading `~/` expanded).
    pub fn repomind_home(&self) -> PathBuf {
        expand_tilde(&self.repomind.home)
    }

    /// The configured basic-memory config file as an absolute path, or `None` for the CLI's own
    /// default. Only the `[repomind]` setting: the environment override is read by the caller.
    pub fn repomind_basic_memory_config(&self) -> Option<PathBuf> {
        self.repomind
            .basic_memory_config
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(expand_tilde)
    }

    /// The agent that runs as repomind's primary controller: the `[repomind]` setting if given,
    /// otherwise the older `orchestrator_agent`. `None` means "let the launcher pick its default".
    pub fn repomind_primary_agent(&self) -> Option<String> {
        self.repomind
            .primary_agent
            .clone()
            .or_else(|| self.orchestrator_agent.clone())
    }

    /// Whether this resolved sender represents the human operator or an explicitly configured,
    /// human-supervised coordinator. Coordinator entries may be exact canonical addresses or a
    /// `lane-<id>/*` pattern covering every slot in one lane.
    pub fn message_sender_refreshes_hops(&self, address: &str) -> bool {
        address == "operator"
            || self
                .message_hop_refresh_senders
                .iter()
                .any(|pattern| message_address_pattern_matches(pattern, address))
    }
}

fn message_address_pattern_matches(pattern: &str, address: &str) -> bool {
    let pattern = pattern.trim();
    if pattern == address {
        return true;
    }
    let Some(pattern_lane) = pattern
        .strip_prefix("lane-")
        .and_then(|rest| rest.strip_suffix("/*"))
    else {
        return false;
    };
    let Some((address_lane, slot)) = address
        .strip_prefix("lane-")
        .and_then(|rest| rest.split_once('/'))
    else {
        return false;
    };
    !pattern_lane.is_empty()
        && pattern_lane == address_lane
        && !slot.is_empty()
        && !slot.contains('/')
}

/// Expand a leading `~/` against the user's home directory. Any other shape (absolute, relative,
/// or a bare `~`) is taken verbatim, and so is `~/...` on a machine with no resolvable home.
pub fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(base) = directories::BaseDirs::new() {
            return base.home_dir().join(rest);
        }
    }
    PathBuf::from(s)
}

/// Returns the user’s home directory on the current platform.
pub fn home() -> PathBuf {
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

/// The platform data directory for the SQLite database. `REPOMON_DATA_DIR` overrides it - handy
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
    current_user_from(
        std::env::var("USER").ok(),
        std::env::var("LOGNAME").ok(),
        std::env::var("USERNAME").ok(),
    )
}

/// `USER`/`LOGNAME` are the unix conventions; `USERNAME` is the Windows one. Factored pure so
/// the fallback order is unit-testable without mutating the process environment.
fn current_user_from(
    user: Option<String>,
    logname: Option<String>,
    username: Option<String>,
) -> String {
    user.or(logname)
        .or(username)
        .unwrap_or_else(|| "user".to_string())
}

#[cfg(target_os = "macos")]
fn default_socket_path() -> PathBuf {
    PathBuf::from(format!("/tmp/repomon-{}.sock", current_user()))
}

#[cfg(windows)]
fn default_socket_path() -> PathBuf {
    // Interpreted as a named-pipe name by `crate::transport` (already in canonical
    // `\\.\pipe\` form, so it passes through `pipe_name_from_path` verbatim).
    PathBuf::from(format!(r"\\.\pipe\repomon-{}", current_user()))
}

#[cfg(not(any(target_os = "macos", windows)))]
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
        assert!(!valid_tmux_session(""));
        assert!(!valid_tmux_session("a:b")); // colon corrupts session:window targets
        assert!(!valid_tmux_session("a b"));
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
        assert!(c.notify_sound);
        assert_eq!(c.notify_sound_volume, 0.25);
        assert!(c.notify_sound_unfocused_only);
        assert!(c.notify_sound_agent_needs_you);
        assert!(c.notify_sound_agent_finished);
        assert!(c.notify_sound_repomind_needs_you);
        assert!(c.notify_sound_error_or_stall);
        assert!(c.notify_sound_incoming_message);
        assert!(c.notify_sound_update_ready);
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
    fn sort_mode_resolution_precedence() {
        assert_eq!(resolve_sort_mode(None, false), SortMode::Default);
        assert_eq!(resolve_sort_mode(None, true), SortMode::Activity);

        assert_eq!(
            resolve_sort_mode(Some(SortMode::Manual), true),
            SortMode::Manual
        );
        assert_eq!(
            resolve_sort_mode(Some(SortMode::Default), true),
            SortMode::Default
        );
        assert_eq!(
            resolve_sort_mode(Some(SortMode::Activity), false),
            SortMode::Activity
        );
    }

    #[test]
    fn tab_sort_mode_resolution_and_round_trip() {
        assert_eq!(resolve_tab_sort_mode(None), TabSortMode::Activity);
        assert_eq!(Config::default().tab_sort_mode(), TabSortMode::Activity);

        let dir = std::env::temp_dir().join(format!("repomon-tabsort-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");
        let c = Config {
            tab_sort_mode: Some(TabSortMode::Manual),
            ..Default::default()
        };
        c.save_to(&path).unwrap();
        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.tab_sort_mode(), TabSortMode::Manual);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sort_mode_defaults_and_serializes_lowercase() {
        let c = Config::default();
        assert_eq!(c.sort_mode(), SortMode::Default);

        let c: Config = toml::from_str("sort_repos_by_activity = true\n").unwrap();
        assert_eq!(c.sort_mode(), SortMode::Activity);

        let c: Config = toml::from_str("sort_mode = \"manual\"\n").unwrap();
        assert_eq!(c.sort_mode(), SortMode::Manual);
    }

    #[test]
    fn sort_mode_round_trips_through_toml() {
        let dir =
            std::env::temp_dir().join(format!("repomon-sortmode-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");

        let c = Config {
            sort_mode: Some(SortMode::Manual),
            ..Default::default()
        };
        c.save_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.sort_mode(), SortMode::Manual);

        std::fs::write(&path, "sort_repos_by_activity = true\n").unwrap();
        let legacy = Config::load_from(&path).unwrap();
        assert_eq!(legacy.sort_mode(), SortMode::Activity);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_partial_toml() {
        let c: Config = toml::from_str("tmux_session = \"work\"\n").unwrap();
        assert_eq!(c.tmux_session, "work");

        assert_eq!(c.worktree_template, DEFAULT_WORKTREE_TEMPLATE);
        assert_eq!(c.notify_sound_volume, 0.25);
        assert!(c.notify_sound_unfocused_only);
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
        c.agent_icons.insert("codex".into(), "bolt".into());
        c.save_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.default_agent.as_deref(), Some("claude-yolo"));
        assert_eq!(
            loaded.agents.get("claude-yolo").map(String::as_str),
            Some("claude --dangerously-skip-permissions")
        );
        assert_eq!(
            loaded.agent_icons.get("codex").map(String::as_str),
            Some("bolt")
        );

        assert_eq!(loaded.tmux_session, "work");
        assert_eq!(loaded.worktree_template, DEFAULT_WORKTREE_TEMPLATE);

        let mut c2 = loaded;
        c2.default_agent = None;
        c2.agents.clear();
        c2.save_to(&path).unwrap();
        let reloaded = Config::load_from(&path).unwrap();
        assert!(reloaded.default_agent.is_none());
        assert!(reloaded.agents.is_empty());

        let leftover_tmp = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().ends_with(".tmp"));
        assert!(!leftover_tmp, "a .tmp file was left behind after save");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn current_user_fallback_order() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(current_user_from(s("ali"), s("log"), s("win")), "ali");
        assert_eq!(current_user_from(None, s("log"), s("win")), "log");
        assert_eq!(current_user_from(None, None, s("win")), "win");
        assert_eq!(current_user_from(None, None, None), "user");
    }

    #[cfg(windows)]
    #[test]
    fn windows_default_socket_path_is_a_pipe_name() {
        let p = default_socket_path();
        let s = p.to_string_lossy();
        assert!(
            s.starts_with(r"\\.\pipe\repomon-"),
            "expected a \\\\.\\pipe\\repomon-<user> name, got {s}"
        );
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

    #[test]
    fn config_without_supervision_table_gets_defaults() {
        let toml_str = r#"
            tmux_session = "custom"
        "#;
        let cfg: Config = toml::from_str(toml_str).expect("parse config");
        assert!(!cfg.supervision.enabled);
        assert_eq!(
            cfg.supervision.nudge_text,
            "Check your repomail and act on it."
        );
        assert_eq!(cfg.supervision.stall_mins, 20);
        assert_eq!(cfg.supervision.nudge_retries, 2);
        assert_eq!(
            cfg.supervision.classes,
            crate::agent::supervision::SupervisionConfig::default().classes
        );
    }

    #[test]
    fn partial_supervision_classes_fill_from_defaults() {
        let toml_str = r#"
            [supervision]
            enabled = true

            [supervision.classes]
            command_exec = "hold"
        "#;
        let cfg: Config = toml::from_str(toml_str).expect("parse partial config");
        assert!(cfg.supervision.enabled);

        assert_eq!(
            cfg.supervision
                .classes
                .get(&crate::agent::supervision::DialogClass::CommandExec),
            Some(&crate::agent::supervision::PolicyAction::Hold)
        );
        assert_eq!(cfg.supervision.classes.len(), 1);

        // After resolve(), totality is established: CommandExec is Hold, FileWrite is AutoApprove, rest are Hold
        let effective = crate::agent::supervision::resolve(
            &cfg.supervision,
            Some(&crate::agent::supervision::SupervisionOverrides {
                lane_id: 1,
                enabled: true,
                classes: std::collections::BTreeMap::new(),
                nudge_text: None,
                stall_mins: None,
                nudge_retries: None,
                expect_work: false,
                updated_at: chrono::Utc::now(),
            }),
        );
        assert_eq!(effective.classes.len(), 9);
        assert_eq!(
            effective
                .classes
                .get(&crate::agent::supervision::DialogClass::CommandExec),
            Some(&crate::agent::supervision::PolicyAction::Hold)
        );
        assert_eq!(
            effective
                .classes
                .get(&crate::agent::supervision::DialogClass::FileWrite),
            Some(&crate::agent::supervision::PolicyAction::AutoApprove)
        );
        assert_eq!(
            effective
                .classes
                .get(&crate::agent::supervision::DialogClass::Deletion),
            Some(&crate::agent::supervision::PolicyAction::Hold)
        );
    }

    #[test]
    fn supervision_config_roundtrips() {
        let dir = std::env::temp_dir().join(format!(
            "repomon-supervision-cfg-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");

        let mut c = Config::default();
        c.supervision.enabled = true;
        c.supervision.stall_mins = 45;
        c.supervision.nudge_retries = 5;
        c.supervision.nudge_text = "Custom nudge message".to_string();
        c.message_hop_refresh_senders = vec!["lane-81/3".to_string()];
        c.supervision.classes.insert(
            crate::agent::supervision::DialogClass::Deletion,
            crate::agent::supervision::PolicyAction::AutoDeny,
        );

        c.save_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap();
        assert!(loaded.supervision.enabled);
        assert_eq!(loaded.supervision.stall_mins, 45);
        assert_eq!(loaded.supervision.nudge_retries, 5);
        assert_eq!(loaded.supervision.nudge_text, "Custom nudge message");
        assert_eq!(
            loaded
                .supervision
                .classes
                .get(&crate::agent::supervision::DialogClass::Deletion),
            Some(&crate::agent::supervision::PolicyAction::AutoDeny)
        );
        assert_eq!(loaded.supervision, c.supervision);
        assert_eq!(
            loaded.message_hop_refresh_senders,
            c.message_hop_refresh_senders
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn message_hop_refresh_senders_require_an_explicit_address_or_lane_pattern() {
        let mut config = Config::default();
        assert!(config.message_sender_refreshes_hops("operator"));
        assert!(!config.message_sender_refreshes_hops("lane-81/3"));

        config.message_hop_refresh_senders =
            vec!["lane-81/3".into(), " lane-92/* ".into(), "repomind".into()];
        assert!(config.message_sender_refreshes_hops("lane-81/3"));
        assert!(!config.message_sender_refreshes_hops("lane-81/4"));
        assert!(config.message_sender_refreshes_hops("lane-92/1"));
        assert!(config.message_sender_refreshes_hops("lane-92/12"));
        assert!(!config.message_sender_refreshes_hops("lane-921/1"));
        assert!(config.message_sender_refreshes_hops("repomind"));
    }

    /// An existing config file that predates the `[repomind]` table must still load, with every
    /// repomind setting at its documented default.
    #[test]
    fn repomind_settings_default_when_the_table_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "tmux_session = \"repomon\"\n").unwrap();
        let c = Config::load_from(&path).unwrap();
        assert_eq!(c.repomind.home, DEFAULT_REPOMIND_HOME);
        assert_eq!(c.repomind.max_controllers, 2);
        assert_eq!(c.repomind.primary_agent, None);
    }

    /// `repomind.home` is a `~`-prefixed path in the file and an absolute path everywhere else.
    #[test]
    fn repomind_home_expands_a_leading_tilde() {
        let c = Config::default();
        assert_eq!(c.repomind_home(), home().join("repomind"));

        let mut c = Config::default();
        c.repomind.home = "/srv/repomind".into();
        assert_eq!(c.repomind_home(), PathBuf::from("/srv/repomind"));
    }

    /// The primary controller agent falls back to the existing `orchestrator_agent` setting, so
    /// an operator who already picked one does not have to pick it twice.
    #[test]
    fn repomind_primary_agent_falls_back_to_the_orchestrator_agent() {
        let mut c = Config::default();
        assert_eq!(c.repomind_primary_agent(), None);

        c.orchestrator_agent = Some("claude-work".into());
        assert_eq!(c.repomind_primary_agent(), Some("claude-work".to_string()));

        c.repomind.primary_agent = Some("codex".into());
        assert_eq!(c.repomind_primary_agent(), Some("codex".to_string()));
    }

    #[test]
    fn repomind_table_round_trips_through_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut c = Config::default();
        c.repomind.home = "~/elsewhere".into();
        c.repomind.primary_agent = Some("codex".into());
        c.repomind.max_controllers = 5;
        c.save_to(&path).unwrap();
        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.repomind.home, "~/elsewhere");
        assert_eq!(loaded.repomind.primary_agent, Some("codex".to_string()));
        assert_eq!(loaded.repomind.max_controllers, 5);
    }

    /// An isolated daemon must be able to point basic-memory somewhere other than the operator's
    /// real `~/.basic-memory/config.json`. Unset means "the default", which the caller resolves.
    #[test]
    fn repomind_basic_memory_config_is_none_by_default_and_expands_a_tilde() {
        let mut c = Config::default();
        assert_eq!(c.repomind_basic_memory_config(), None);

        c.repomind.basic_memory_config = Some("~/isolated/config.json".into());
        assert_eq!(
            c.repomind_basic_memory_config(),
            Some(home().join("isolated").join("config.json"))
        );
    }

    #[test]
    fn repomind_boot_budget_defaults_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut c = Config::default();
        assert_eq!(c.repomind.boot_budget_tokens, DEFAULT_BOOT_BUDGET_TOKENS);
        c.repomind.boot_budget_tokens = 4000;
        c.save_to(&path).unwrap();
        assert_eq!(
            Config::load_from(&path)
                .unwrap()
                .repomind
                .boot_budget_tokens,
            4000
        );
    }

    #[test]
    fn repomind_basic_memory_config_round_trips_through_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut c = Config::default();
        c.repomind.basic_memory_config = Some("/srv/bm/config.json".into());
        c.save_to(&path).unwrap();
        assert_eq!(
            Config::load_from(&path)
                .unwrap()
                .repomind
                .basic_memory_config,
            Some("/srv/bm/config.json".to_string())
        );
    }
    #[test]
    fn usage_defaults_are_on_including_the_daily_price_refresh() {
        let c = Config::default();
        assert!(c.usage.enabled, "the ledger reads files that already exist");
        assert!(
            c.usage.refresh_prices,
            "a daily GET to a static file is a reasonable default; set refresh_prices = false to opt out"
        );
        assert!(c.usage.price_overrides.is_empty());
        assert!(c.usage.scan_interval_secs >= 60);
        assert!(c.usage.max_files_per_scan > 0);
    }

    #[test]
    fn refresh_prices_can_be_turned_off_from_toml() {
        let toml = "[usage]\nrefresh_prices = false\n";
        let c: Config = toml::from_str(toml).unwrap();
        assert!(!c.usage.refresh_prices);
    }

    #[test]
    fn a_usage_price_override_round_trips_through_toml() {
        let toml = r#"
[usage]
enabled = true

[usage.price_overrides."claude-sonnet-5"]
input_per_mtok = 1.5
output_per_mtok = 7.5
"#;
        let c: Config = toml::from_str(toml).unwrap();
        let over = c.usage.price_overrides.get("claude-sonnet-5").unwrap();
        assert_eq!(over.input_per_mtok, Some(1.5));
        assert_eq!(over.output_per_mtok, Some(7.5));
        assert_eq!(over.cache_read_per_mtok, None);
    }

    #[test]
    fn a_config_carrying_usage_tables_still_serializes_to_valid_toml() {
        let mut c = Config::default();
        c.usage.price_overrides.insert(
            "claude-opus-5".to_string(),
            crate::pricing::PriceOverride {
                input_per_mtok: Some(4.0),
                ..Default::default()
            },
        );
        let text = toml::to_string(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(
            back.usage.price_overrides["claude-opus-5"].input_per_mtok,
            Some(4.0)
        );
    }
}
