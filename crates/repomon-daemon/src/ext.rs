//! Claude Code extension management: config scanning, enabledPlugins toggles, and repo-to-worktree
//! fan-out. The daemon is the single authority; the GUI and TUI only speak the ext RPCs.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use repomon_core::agent::claude::{account_key, account_label, config_bases, default_config_base};
use repomon_core::model::{
    AgentKind, EnabledSource, ExtAccount, ExtSnapshot, FanoutSummary, MarketplaceInfo, PluginInfo,
    PluginProvides, SkillInfo, SkillSource, SkippedLane,
};
use serde_json::Value;

/// The Claude Code home this daemon manages. `REPOMON_CLAUDE_HOME` overrides for tests; the
/// default `~/.claude` is the only account v1 manages (multi-account layers on later).
pub fn claude_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("REPOMON_CLAUDE_HOME") {
        return Some(PathBuf::from(dir));
    }
    directories::BaseDirs::new().map(|b| b.home_dir().join(".claude"))
}

/// Accounts and agent ecosystem scopes to offer in the extensions picker:
/// - Claude config dirs (main and variants)
/// - Antigravity (`~/.gemini`)
/// - Codex (`~/.codex`)
/// - OpenCode (`~/.config/opencode`)
/// - Cursor (`~/.cursor`)
pub fn ext_accounts() -> Vec<ExtAccount> {
    let default = default_config_base();
    let mut out: Vec<ExtAccount> = config_bases()
        .into_iter()
        .filter(|base| base.join("projects").is_dir())
        .map(|base| {
            let cfg = (base != default).then(|| base.clone());
            ExtAccount {
                key: account_key(cfg.as_deref()),
                label: account_label(cfg.as_deref()),
                claude: true,
                agent_kind: Some(AgentKind::ClaudeCode),
            }
        })
        .collect();

    if let Some(dirs) = directories::BaseDirs::new() {
        let home = dirs.home_dir();

        if home.join(".gemini").is_dir() {
            out.push(ExtAccount {
                key: "antigravity".to_string(),
                label: "Antigravity".to_string(),
                claude: false,
                agent_kind: Some(AgentKind::Antigravity),
            });
        }

        if home.join(".codex").is_dir() {
            out.push(ExtAccount {
                key: "codex".to_string(),
                label: "Codex".to_string(),
                claude: false,
                agent_kind: Some(AgentKind::Codex),
            });
        }

        if home.join(".config/opencode").is_dir() {
            out.push(ExtAccount {
                key: "opencode".to_string(),
                label: "OpenCode".to_string(),
                claude: false,
                agent_kind: Some(AgentKind::OpenCode),
            });
        }

        if home.join(".cursor").is_dir() {
            out.push(ExtAccount {
                key: "cursor".to_string(),
                label: "Cursor".to_string(),
                claude: false,
                agent_kind: Some(AgentKind::Cursor),
            });
        }
    }
    out
}

/// Resolve an account key (see `ExtAccount::key`) to the Claude config home to scan and mutate.
/// `None`/`"default"` -> the default `~/.claude` (honoring `REPOMON_CLAUDE_HOME`). A config-dir
/// path -> that path. Non-Claude agent scopes -> `None`.
pub fn claude_home_for(account: Option<&str>) -> Option<PathBuf> {
    match account {
        None | Some("default") => claude_home(),
        Some("codex") | Some("antigravity") | Some("opencode") | Some("cursor") => None,
        Some(path) => Some(PathBuf::from(path)),
    }
}

/// The `CLAUDE_CONFIG_DIR` a `claude` CLI op should run under for an account. `None` = the default
/// account, where the variable is *unset* so a bare `claude` cannot inherit the daemon's own leaked
/// `CLAUDE_CONFIG_DIR`. `Some(dir)` pins a variant account.
pub fn account_config_dir(account: Option<&str>) -> Option<PathBuf> {
    match account {
        Some(path)
            if path != "default"
                && path != "codex"
                && path != "antigravity"
                && path != "opencode"
                && path != "cursor" =>
        {
            Some(PathBuf::from(path))
        }
        _ => None,
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// `enabledPlugins` from one settings file; missing file or key is just an empty map.
fn enabled_map(settings: &Path) -> BTreeMap<String, bool> {
    read_json(settings)
        .as_ref()
        .and_then(|v| v.get("enabledPlugins"))
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_bool().map(|b| (k.clone(), b)))
                .collect()
        })
        .unwrap_or_default()
}

/// A YAML block scalar indicator (`|`, `>`, `|-`, `>-`) introducing a multi-line value: the real
/// value is the following, more-indented lines, not the indicator itself.
fn is_block_scalar_indicator(v: &str) -> bool {
    matches!(v, "|" | ">" | "|-" | ">-")
}

/// Collect a YAML block scalar's body: consecutive lines indented deeper than `key_indent`,
/// trimmed and joined with a single space. Stops at the first line at or below that indent
/// (another key, or the closing `---`). Returns the joined value and how many lines were consumed.
fn collect_block_scalar(lines: &[&str], key_indent: usize) -> (String, usize) {
    let mut parts = Vec::new();
    let mut consumed = 0;
    for line in lines {
        let indent = line.len() - line.trim_start().len();
        if line.trim().is_empty() || indent <= key_indent {
            break;
        }
        parts.push(line.trim());
        consumed += 1;
    }
    (parts.join(" "), consumed)
}

/// Parse the `name:`/`description:` frontmatter lines from a SKILL.md. Handles both plain
/// single-line values and YAML block scalars (`|`, `>`, `|-`, `>-`) commonly used for multi-line
/// descriptions.
fn skill_frontmatter(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(text) = fs::read_to_string(path) else {
        return (None, None);
    };
    let all_lines: Vec<&str> = text.lines().collect();
    let (mut name, mut description, mut in_fm) = (None, None, false);
    let mut i = 0;
    while i < all_lines.len() {
        let line = all_lines[i];
        let t = line.trim();
        if t == "---" {
            if in_fm {
                break;
            }
            in_fm = true;
            i += 1;
            continue;
        }
        if !in_fm {
            i += 1;
            continue;
        }
        let key_indent = line.len() - line.trim_start().len();
        if let Some(v) = t.strip_prefix("name:") {
            let v = v.trim();
            if is_block_scalar_indicator(v) {
                let (joined, consumed) = collect_block_scalar(&all_lines[i + 1..], key_indent);
                name = Some(joined);
                i += consumed;
            } else {
                name = Some(v.to_string());
            }
        } else if let Some(v) = t.strip_prefix("description:") {
            let v = v.trim();
            if is_block_scalar_indicator(v) {
                let (joined, consumed) = collect_block_scalar(&all_lines[i + 1..], key_indent);
                description = Some(joined);
                i += consumed;
            } else {
                description = Some(v.to_string());
            }
        }
        i += 1;
    }
    (name, description)
}

fn scan_skills(dir: &Path, source: SkillSource) -> Vec<SkillInfo> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let md = path.join("SKILL.md");
        if !md.is_file() {
            continue;
        }
        let (name, description) = skill_frontmatter(&md);
        out.push(SkillInfo {
            name: name.unwrap_or_else(|| entry.file_name().to_string_lossy().into_owned()),
            description,
            source,
            path,
        });
    }
    out
}

fn count_dir(path: &Path) -> u32 {
    fs::read_dir(path)
        .map(|d| d.flatten().count() as u32)
        .unwrap_or(0)
}

/// Installed plugin records: id -> (version, install_path). First instance wins (the cache is
/// shared; instances differ only in scope bookkeeping we deliberately ignore).
fn installed_plugins(claude_home: &Path) -> BTreeMap<String, (Option<String>, Option<PathBuf>)> {
    let mut out = BTreeMap::new();
    let Some(root) = read_json(&claude_home.join("plugins/installed_plugins.json")) else {
        return out;
    };
    let Some(plugins) = root.get("plugins").and_then(Value::as_object) else {
        return out;
    };
    for (id, instances) in plugins {
        let first = instances.as_array().and_then(|a| a.first());
        let version = first
            .and_then(|i| i.get("version"))
            .and_then(Value::as_str)
            .filter(|v| *v != "unknown")
            .map(String::from);
        let install_path = first
            .and_then(|i| i.get("installPath"))
            .and_then(Value::as_str)
            .map(PathBuf::from);
        out.insert(id.clone(), (version, install_path));
    }
    out
}

fn scan_marketplaces(claude_home: &Path) -> Vec<MarketplaceInfo> {
    let Some(root) = read_json(&claude_home.join("plugins/known_marketplaces.json")) else {
        return Vec::new();
    };
    let Some(map) = root.as_object() else {
        return Vec::new();
    };
    map.iter()
        .map(|(name, m)| {
            let source = m.get("source");
            let kind = source
                .and_then(|s| s.get("source"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let reference = source
                .and_then(|s| {
                    s.get("repo")
                        .or_else(|| s.get("url"))
                        .or_else(|| s.get("path"))
                })
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            MarketplaceInfo {
                name: name.clone(),
                kind,
                reference,
                last_updated: m
                    .get("lastUpdated")
                    .and_then(Value::as_str)
                    .map(String::from),
            }
        })
        .collect()
}

/// A CLI operation failure with everything the GUI needs to show a useful error.
#[derive(Debug)]
pub struct CliFailure {
    pub message: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// Handle to the `claude` CLI. Detection runs `claude --version` once per call site; the RPC
/// layer caches via OnceLock, but only a success is cached, so a CLI installed after the daemon
/// started is picked up on the next call without a restart.
pub struct ClaudeCli {
    pub bin: PathBuf,
    pub version: String,
}

impl ClaudeCli {
    pub fn detect() -> Option<ClaudeCli> {
        // `REPOMON_CLAUDE_BIN` overrides the binary for tests (eg. pointing at a nonexistent path
        // to deterministically exercise the -32021 "CLI not found" case).
        let bin = std::env::var_os("REPOMON_CLAUDE_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("claude"));
        let out = std::process::Command::new(&bin)
            .arg("--version")
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Some(ClaudeCli {
            bin,
            version: String::from_utf8_lossy(&out.stdout).trim().to_string(),
        })
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn run(&self, args: &[&str]) -> Result<String, CliFailure> {
        self.run_for(None, args)
    }

    /// Like [`run`], but pinned to an account's config dir. `Some(dir)` sets `CLAUDE_CONFIG_DIR`;
    /// `None` unsets it so the default `~/.claude` account is used regardless of the daemon's own
    /// environment (the daemon is often started from a `claude-work` shell, which would otherwise
    /// leak into a bare `claude` and target the wrong account).
    pub fn run_for(&self, config_dir: Option<&Path>, args: &[&str]) -> Result<String, CliFailure> {
        let mut cmd = std::process::Command::new(&self.bin);
        cmd.args(args);
        match config_dir {
            Some(dir) => {
                cmd.env("CLAUDE_CONFIG_DIR", dir);
            }
            None => {
                cmd.env_remove("CLAUDE_CONFIG_DIR");
            }
        }
        let out = cmd.output().map_err(|e| CliFailure {
            message: format!("failed to launch claude: {e}"),
            stderr: String::new(),
            exit_code: None,
        })?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(CliFailure {
                message: format!("claude {} failed", args.join(" ")),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                exit_code: out.status.code(),
            })
        }
    }
}

/// Build the full snapshot for one scope. Global scope passes `repo_root: None`; repo scope layers
/// the repo's `.claude` (project skills, settings.local.json toggle overrides) on top.
pub fn scan(
    claude_home: &Path,
    repo_root: Option<&Path>,
    cli_version: Option<String>,
) -> ExtSnapshot {
    let global_enabled = enabled_map(&claude_home.join("settings.json"));
    let repo_enabled = repo_root
        .map(|r| enabled_map(&r.join(".claude/settings.local.json")))
        .unwrap_or_default();
    let installed = installed_plugins(claude_home);

    let mut ids: Vec<String> = installed.keys().cloned().collect();
    for id in global_enabled.keys().chain(repo_enabled.keys()) {
        if !ids.contains(id) {
            ids.push(id.clone())
        }
    }
    ids.sort();

    let plugins = ids
        .into_iter()
        .map(|id| {
            let (enabled, enabled_source) = match (repo_enabled.get(&id), global_enabled.get(&id)) {
                (Some(&b), _) => (b, EnabledSource::Repo),
                (None, Some(&b)) => (b, EnabledSource::Global),
                (None, None) => (false, EnabledSource::Default),
            };
            let record = installed.get(&id);
            let provides = record
                .and_then(|(_, p)| p.as_deref())
                .map(|dir| PluginProvides {
                    skills: count_dir(&dir.join("skills")),
                    commands: count_dir(&dir.join("commands")),
                    agents: count_dir(&dir.join("agents")),
                });
            let (name, marketplace) = match id.split_once('@') {
                Some((n, m)) => (n.to_string(), m.to_string()),
                None => (id.clone(), String::new()),
            };
            PluginInfo {
                name,
                marketplace,
                version: record.and_then(|(v, _)| v.clone()),
                enabled,
                enabled_source,
                provides,
                installed: record.is_some(),
                id,
            }
        })
        .collect();

    let mut skills = scan_skills(&claude_home.join("skills"), SkillSource::User);
    if let Some(repo) = repo_root {
        skills.extend(scan_skills(
            &repo.join(".claude/skills"),
            SkillSource::Project,
        ));
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));

    ExtSnapshot {
        cli_version,
        marketplaces: scan_marketplaces(claude_home),
        plugins,
        skills,
        // The RPC handler fills these in from the requested account; scan stays hermetic so tests
        // do not read the real home to enumerate accounts.
        accounts: Vec::new(),
        account: String::new(),
    }
}

/// Scans Antigravity's real extension/skill environment:
/// - User skills from `~/.gemini/config/skills` and `~/.gemini/skills`
/// - Project skills from `<repo>/.gemini/skills`, `<repo>/.gemini/config/skills`, `<repo>/.agents/skills`
/// - User plugins from `~/.gemini/config/plugins` and `~/.gemini/plugins`
pub fn scan_antigravity(repo_root: Option<&Path>, cli_version: Option<String>) -> ExtSnapshot {
    let mut skills = Vec::new();
    let mut plugins = Vec::new();
    let mut global_enabled = BTreeMap::new();

    if let Some(dirs) = directories::BaseDirs::new() {
        let home = dirs.home_dir();
        let gemini = home.join(".gemini");

        skills.extend(scan_skills(
            &gemini.join("config/skills"),
            SkillSource::User,
        ));
        skills.extend(scan_skills(&gemini.join("skills"), SkillSource::User));

        global_enabled.extend(enabled_map(&gemini.join("settings.json")));
        global_enabled.extend(enabled_map(&gemini.join("config/settings.json")));

        for plugins_dir in [gemini.join("config/plugins"), gemini.join("plugins")] {
            if let Ok(entries) = fs::read_dir(plugins_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        let provides = Some(PluginProvides {
                            skills: count_dir(&path.join("skills")),
                            commands: count_dir(&path.join("commands")),
                            agents: count_dir(&path.join("agents")),
                        });
                        let version = read_json(&path.join("installed_version.json"))
                            .or_else(|| read_json(&path.join("plugin.json")))
                            .or_else(|| read_json(&path.join("package.json")))
                            .and_then(|v| {
                                v.get("version").and_then(Value::as_str).map(String::from)
                            });
                        let id = format!("{name}@antigravity");
                        let (enabled, enabled_source) = if let Some(&val) = global_enabled
                            .get(&id)
                            .or_else(|| global_enabled.get(&name))
                        {
                            (val, EnabledSource::Global)
                        } else {
                            (true, EnabledSource::Global)
                        };
                        plugins.push(PluginInfo {
                            id,
                            name: name.clone(),
                            marketplace: "antigravity".to_string(),
                            version,
                            enabled,
                            enabled_source,
                            provides,
                            installed: true,
                        });
                    }
                }
            }
        }
    }

    if let Some(repo) = repo_root {
        skills.extend(scan_skills(
            &repo.join(".gemini/skills"),
            SkillSource::Project,
        ));
        skills.extend(scan_skills(
            &repo.join(".gemini/config/skills"),
            SkillSource::Project,
        ));
        skills.extend(scan_skills(
            &repo.join(".agents/skills"),
            SkillSource::Project,
        ));

        let repo_enabled = enabled_map(&repo.join(".gemini/settings.json"));
        for plugins_dir in [
            repo.join(".gemini/config/plugins"),
            repo.join(".gemini/plugins"),
        ] {
            if let Ok(entries) = fs::read_dir(plugins_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        let provides = Some(PluginProvides {
                            skills: count_dir(&path.join("skills")),
                            commands: count_dir(&path.join("commands")),
                            agents: count_dir(&path.join("agents")),
                        });
                        let version = read_json(&path.join("installed_version.json"))
                            .or_else(|| read_json(&path.join("plugin.json")))
                            .or_else(|| read_json(&path.join("package.json")))
                            .and_then(|v| {
                                v.get("version").and_then(Value::as_str).map(String::from)
                            });
                        let id = format!("{name}@antigravity");
                        let (enabled, enabled_source) = if let Some(&val) =
                            repo_enabled.get(&id).or_else(|| repo_enabled.get(&name))
                        {
                            (val, EnabledSource::Repo)
                        } else if let Some(&val) = global_enabled
                            .get(&id)
                            .or_else(|| global_enabled.get(&name))
                        {
                            (val, EnabledSource::Global)
                        } else {
                            (true, EnabledSource::Repo)
                        };
                        plugins.push(PluginInfo {
                            id,
                            name: name.clone(),
                            marketplace: "antigravity".to_string(),
                            version,
                            enabled,
                            enabled_source,
                            provides,
                            installed: true,
                        });
                    }
                }
            }
        }

        for p in &mut plugins {
            let raw_name = p.name.clone();
            if let Some(&val) = repo_enabled
                .get(&p.id)
                .or_else(|| repo_enabled.get(&raw_name))
            {
                p.enabled = val;
                p.enabled_source = EnabledSource::Repo;
            }
        }
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills.dedup_by(|a, b| a.path == b.path);

    plugins.sort_by(|a, b| a.name.cmp(&b.name));
    plugins.dedup_by(|a, b| a.id == b.id);

    ExtSnapshot {
        cli_version: cli_version.or_else(|| Some("antigravity".to_string())),
        marketplaces: Vec::new(),
        plugins,
        skills,
        accounts: Vec::new(),
        account: "antigravity".to_string(),
    }
}

/// Scans OpenAI Codex extension environment:
/// - User skills from `~/.codex/skills`
/// - Project skills from `<repo>/.codex/skills`
/// - User plugins from `~/.codex/plugins`
pub fn scan_codex(repo_root: Option<&Path>, cli_version: Option<String>) -> ExtSnapshot {
    let mut skills = Vec::new();
    let mut plugins = Vec::new();
    let mut global_enabled = BTreeMap::new();

    if let Some(dirs) = directories::BaseDirs::new() {
        let home = dirs.home_dir();
        let codex = home.join(".codex");

        skills.extend(scan_skills(&codex.join("skills"), SkillSource::User));
        global_enabled.extend(enabled_map(&codex.join("settings.json")));

        let plugins_dir = codex.join("plugins");
        if let Ok(entries) = fs::read_dir(plugins_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let version = read_json(&path.join("installed_version.json"))
                        .or_else(|| read_json(&path.join("plugin.json")))
                        .or_else(|| read_json(&path.join("package.json")))
                        .and_then(|v| v.get("version").and_then(Value::as_str).map(String::from));
                    let id = format!("{name}@codex");
                    let (enabled, enabled_source) = if let Some(&val) = global_enabled
                        .get(&id)
                        .or_else(|| global_enabled.get(&name))
                    {
                        (val, EnabledSource::Global)
                    } else {
                        (true, EnabledSource::Global)
                    };
                    plugins.push(PluginInfo {
                        id,
                        name: name.clone(),
                        marketplace: "codex".to_string(),
                        version,
                        enabled,
                        enabled_source,
                        provides: None,
                        installed: true,
                    });
                }
            }
        }
    }

    if let Some(repo) = repo_root {
        skills.extend(scan_skills(
            &repo.join(".codex/skills"),
            SkillSource::Project,
        ));
        let repo_enabled = enabled_map(&repo.join(".codex/settings.json"));
        let repo_plugins = repo.join(".codex/plugins");
        if let Ok(entries) = fs::read_dir(repo_plugins) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let version = read_json(&path.join("installed_version.json"))
                        .or_else(|| read_json(&path.join("plugin.json")))
                        .or_else(|| read_json(&path.join("package.json")))
                        .and_then(|v| v.get("version").and_then(Value::as_str).map(String::from));
                    let id = format!("{name}@codex");
                    let (enabled, enabled_source) = if let Some(&val) =
                        repo_enabled.get(&id).or_else(|| repo_enabled.get(&name))
                    {
                        (val, EnabledSource::Repo)
                    } else if let Some(&val) = global_enabled
                        .get(&id)
                        .or_else(|| global_enabled.get(&name))
                    {
                        (val, EnabledSource::Global)
                    } else {
                        (true, EnabledSource::Repo)
                    };
                    plugins.push(PluginInfo {
                        id,
                        name: name.clone(),
                        marketplace: "codex".to_string(),
                        version,
                        enabled,
                        enabled_source,
                        provides: None,
                        installed: true,
                    });
                }
            }
        }
        for p in &mut plugins {
            let raw_name = p.name.clone();
            if let Some(&val) = repo_enabled
                .get(&p.id)
                .or_else(|| repo_enabled.get(&raw_name))
            {
                p.enabled = val;
                p.enabled_source = EnabledSource::Repo;
            }
        }
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    plugins.sort_by(|a, b| a.name.cmp(&b.name));
    plugins.dedup_by(|a, b| a.id == b.id);

    ExtSnapshot {
        cli_version: cli_version.or_else(|| Some("codex".to_string())),
        marketplaces: Vec::new(),
        plugins,
        skills,
        accounts: Vec::new(),
        account: "codex".to_string(),
    }
}

/// Scans OpenCode extension environment:
/// - Global plugin[] and disabled_plugins[] from `~/.config/opencode/opencode.json`
/// - Project plugin[] and disabled_plugins[] from `<repo>/opencode.json` or `<repo>/.opencode/opencode.json`
/// - User & Project skills from `skills/` directories
pub fn scan_opencode(repo_root: Option<&Path>, cli_version: Option<String>) -> ExtSnapshot {
    let mut skills = Vec::new();
    let mut plugins = Vec::new();

    if let Some(dirs) = directories::BaseDirs::new() {
        let home = dirs.home_dir();
        let opencode_dir = home.join(".config/opencode");

        skills.extend(scan_skills(&opencode_dir.join("skills"), SkillSource::User));

        let cfg_path = opencode_dir.join("opencode.json");
        if let Some(root) = read_json(&cfg_path) {
            if let Some(arr) = root.get("plugin").and_then(Value::as_array) {
                for item in arr {
                    if let Some(raw) = item.as_str() {
                        let (name, ver) = match raw.split_once('@') {
                            Some((n, v)) => (n.to_string(), Some(v.to_string())),
                            None => (raw.to_string(), None),
                        };
                        plugins.push(PluginInfo {
                            id: raw.to_string(),
                            name,
                            marketplace: "opencode".to_string(),
                            version: ver,
                            enabled: true,
                            enabled_source: EnabledSource::Global,
                            provides: None,
                            installed: true,
                        });
                    }
                }
            }
            if let Some(arr) = root.get("disabled_plugins").and_then(Value::as_array) {
                for item in arr {
                    if let Some(raw) = item.as_str() {
                        let (name, ver) = match raw.split_once('@') {
                            Some((n, v)) => (n.to_string(), Some(v.to_string())),
                            None => (raw.to_string(), None),
                        };
                        plugins.push(PluginInfo {
                            id: raw.to_string(),
                            name,
                            marketplace: "opencode".to_string(),
                            version: ver,
                            enabled: false,
                            enabled_source: EnabledSource::Global,
                            provides: None,
                            installed: true,
                        });
                    }
                }
            }
        }
    }

    if let Some(repo) = repo_root {
        skills.extend(scan_skills(
            &repo.join(".opencode/skills"),
            SkillSource::Project,
        ));

        for cfg_path in [
            repo.join("opencode.json"),
            repo.join(".opencode/opencode.json"),
        ] {
            if let Some(root) = read_json(&cfg_path) {
                if let Some(arr) = root.get("plugin").and_then(Value::as_array) {
                    for item in arr {
                        if let Some(raw) = item.as_str() {
                            let (name, ver) = match raw.split_once('@') {
                                Some((n, v)) => (n.to_string(), Some(v.to_string())),
                                None => (raw.to_string(), None),
                            };
                            plugins.push(PluginInfo {
                                id: raw.to_string(),
                                name,
                                marketplace: "opencode".to_string(),
                                version: ver,
                                enabled: true,
                                enabled_source: EnabledSource::Repo,
                                provides: None,
                                installed: true,
                            });
                        }
                    }
                }
                if let Some(arr) = root.get("disabled_plugins").and_then(Value::as_array) {
                    for item in arr {
                        if let Some(raw) = item.as_str() {
                            let (name, ver) = match raw.split_once('@') {
                                Some((n, v)) => (n.to_string(), Some(v.to_string())),
                                None => (raw.to_string(), None),
                            };
                            plugins.push(PluginInfo {
                                id: raw.to_string(),
                                name,
                                marketplace: "opencode".to_string(),
                                version: ver,
                                enabled: false,
                                enabled_source: EnabledSource::Repo,
                                provides: None,
                                installed: true,
                            });
                        }
                    }
                }
            }
        }
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    plugins.sort_by(|a, b| a.name.cmp(&b.name));
    plugins.dedup_by(|a, b| a.id == b.id);

    ExtSnapshot {
        cli_version: cli_version.or_else(|| Some("opencode".to_string())),
        marketplaces: Vec::new(),
        plugins,
        skills,
        accounts: Vec::new(),
        account: "opencode".to_string(),
    }
}

/// Scans Cursor extension environment:
/// - Installed extensions from `~/.cursor/extensions/extensions.json`
/// - Skills from `~/.cursor/skills` and `<repo>/.cursor/skills`
pub fn scan_cursor(repo_root: Option<&Path>, cli_version: Option<String>) -> ExtSnapshot {
    let mut skills = Vec::new();
    let mut plugins = Vec::new();
    let mut global_enabled = BTreeMap::new();

    if let Some(dirs) = directories::BaseDirs::new() {
        let home = dirs.home_dir();
        let cursor_dir = home.join(".cursor");

        skills.extend(scan_skills(&cursor_dir.join("skills"), SkillSource::User));
        global_enabled.extend(enabled_map(&cursor_dir.join("settings.json")));

        let ext_json_path = cursor_dir.join("extensions/extensions.json");
        if let Some(root) = read_json(&ext_json_path) {
            if let Some(arr) = root.as_array() {
                for item in arr {
                    let id = item
                        .get("identifier")
                        .and_then(|id_obj| id_obj.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if id.is_empty() {
                        continue;
                    }
                    let version = item
                        .get("version")
                        .and_then(Value::as_str)
                        .map(String::from);
                    let name = item
                        .get("metadata")
                        .and_then(|m| m.get("publisherDisplayName"))
                        .and_then(Value::as_str)
                        .map(|p| format!("{p}/{id}"))
                        .unwrap_or_else(|| id.clone());

                    let (enabled, enabled_source) = if let Some(&val) = global_enabled.get(&id) {
                        (val, EnabledSource::Global)
                    } else {
                        (true, EnabledSource::Global)
                    };

                    plugins.push(PluginInfo {
                        id: id.clone(),
                        name,
                        marketplace: "cursor".to_string(),
                        version,
                        enabled,
                        enabled_source,
                        provides: None,
                        installed: true,
                    });
                }
            }
        }
    }

    if let Some(repo) = repo_root {
        skills.extend(scan_skills(
            &repo.join(".cursor/skills"),
            SkillSource::Project,
        ));
        let repo_enabled = enabled_map(&repo.join(".cursor/settings.json"));
        for p in &mut plugins {
            if let Some(&val) = repo_enabled.get(&p.id) {
                p.enabled = val;
                p.enabled_source = EnabledSource::Repo;
            }
        }
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    plugins.sort_by(|a, b| a.name.cmp(&b.name));
    plugins.dedup_by(|a, b| a.id == b.id);

    ExtSnapshot {
        cli_version: cli_version.or_else(|| Some("cursor".to_string())),
        marketplaces: Vec::new(),
        plugins,
        skills,
        accounts: Vec::new(),
        account: "cursor".to_string(),
    }
}

/// Unified entry point to scan extensions for any account / agent ecosystem.
pub fn scan_for_account(
    account_key: &str,
    repo_root: Option<&Path>,
    cli_version: Option<String>,
) -> ExtSnapshot {
    match account_key {
        "antigravity" => scan_antigravity(repo_root, cli_version),
        "codex" => scan_codex(repo_root, cli_version),
        "opencode" => scan_opencode(repo_root, cli_version),
        "cursor" => scan_cursor(repo_root, cli_version),
        other => {
            let home =
                claude_home_for(Some(other)).unwrap_or_else(|| claude_home().unwrap_or_default());
            scan(&home, repo_root, cli_version)
        }
    }
}

/// Serializes all settings writes so concurrent RPCs cannot interleave read-modify-write cycles.
static SETTINGS_WRITE: Mutex<()> = Mutex::new(());

/// Read-modify-write ONLY the `enabledPlugins` key, preserving every other byte of meaning in the
/// file, then atomically replace it (temp file + rename). `enabled: None` removes the entry.
/// A corrupt file is an error, never a clobber.
pub fn set_plugin_enabled(settings: &Path, id: &str, enabled: Option<bool>) -> io::Result<()> {
    let _guard = SETTINGS_WRITE.lock().unwrap();
    let mut root: Value = match fs::read_to_string(settings) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("corrupt settings: {e}"))
        })?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Value::Object(Default::default()),
        Err(e) => return Err(e),
    };
    let Value::Object(map) = &mut root else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "settings root is not an object",
        ));
    };
    let plugins = map
        .entry("enabledPlugins")
        .or_insert_with(|| Value::Object(Default::default()));
    let Value::Object(plugins) = plugins else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "enabledPlugins is not an object",
        ));
    };
    match enabled {
        Some(value) => {
            plugins.insert(id.to_string(), Value::Bool(value));
        }
        None => {
            plugins.remove(id);
        }
    }
    if let Some(dir) = settings.parent() {
        fs::create_dir_all(dir)?
    }
    let tmp = settings.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(&root)?)?;
    fs::rename(&tmp, settings)
}

/// Remove an installed plugin record from `installed_plugins.json` and from `enabledPlugins` in `settings.json`.
pub fn clean_claude_plugin_records(
    claude_home: &Path,
    plugin_id: &str,
    repo_root: Option<&Path>,
) -> io::Result<()> {
    let installed_json = claude_home.join("plugins/installed_plugins.json");
    if let Ok(text) = fs::read_to_string(&installed_json) {
        if let Ok(mut root) = serde_json::from_str::<Value>(&text) {
            if let Some(plugins) = root.get_mut("plugins").and_then(|p| p.as_object_mut()) {
                if plugins.remove(plugin_id).is_some() {
                    let tmp = installed_json.with_extension("tmp");
                    if let Ok(bytes) = serde_json::to_vec_pretty(&root) {
                        let _ = fs::write(&tmp, bytes);
                        let _ = fs::rename(&tmp, &installed_json);
                    }
                }
            }
        }
    }

    // Also remove from global settings.json enabledPlugins
    let global_settings = claude_home.join("settings.json");
    let _ = set_plugin_enabled(&global_settings, plugin_id, None);

    // If repo_root is provided, also remove from .claude/settings.local.json and settings.json
    if let Some(repo) = repo_root {
        let local_settings = repo.join(".claude/settings.local.json");
        let _ = set_plugin_enabled(&local_settings, plugin_id, None);
        let project_settings = repo.join(".claude/settings.json");
        let _ = set_plugin_enabled(&project_settings, plugin_id, None);
    }

    Ok(())
}

/// Robust Claude plugin uninstaller that respects local/project/user scopes and cleans up state.
pub fn uninstall_claude_plugin(
    cli: &ClaudeCli,
    config_dir: Option<&Path>,
    claude_home: &Path,
    plugin_id: &str,
    repo_root: Option<&Path>,
) -> Result<String, CliFailure> {
    let mut outputs = Vec::new();

    // 1. Read installed_plugins.json to find recorded scopes and project paths
    let installed_json = claude_home.join("plugins/installed_plugins.json");
    if let Ok(text) = fs::read_to_string(&installed_json) {
        if let Ok(root) = serde_json::from_str::<Value>(&text) {
            if let Some(instances) = root
                .get("plugins")
                .and_then(|p| p.get(plugin_id))
                .and_then(Value::as_array)
            {
                for inst in instances {
                    let scope = inst.get("scope").and_then(Value::as_str).unwrap_or("user");
                    let proj = inst
                        .get("projectPath")
                        .and_then(Value::as_str)
                        .map(PathBuf::from);
                    let cwd = proj.as_deref().or(repo_root);

                    let mut cmd = std::process::Command::new(&cli.bin);
                    cmd.args(["plugin", "uninstall", plugin_id, "-s", scope]);
                    if let Some(dir) = config_dir {
                        cmd.env("CLAUDE_CONFIG_DIR", dir);
                    } else {
                        cmd.env_remove("CLAUDE_CONFIG_DIR");
                    }
                    if let Some(work_dir) = cwd {
                        if work_dir.is_dir() {
                            cmd.current_dir(work_dir);
                        }
                    }
                    if let Ok(out) = cmd.output() {
                        if out.status.success() {
                            outputs.push(String::from_utf8_lossy(&out.stdout).into_owned());
                        }
                    }
                }
            }
        }
    }

    // 2. If no recorded instances succeeded, try fallback scopes directly
    if outputs.is_empty() {
        let candidate_scopes = match repo_root {
            Some(_) => &["local", "project", "user"][..],
            None => &["user", "local", "project"][..],
        };
        for &scope in candidate_scopes {
            let mut cmd = std::process::Command::new(&cli.bin);
            cmd.args(["plugin", "uninstall", plugin_id, "-s", scope]);
            if let Some(dir) = config_dir {
                cmd.env("CLAUDE_CONFIG_DIR", dir);
            } else {
                cmd.env_remove("CLAUDE_CONFIG_DIR");
            }
            if let Some(repo) = repo_root {
                cmd.current_dir(repo);
            }
            if let Ok(out) = cmd.output() {
                if out.status.success() {
                    outputs.push(String::from_utf8_lossy(&out.stdout).into_owned());
                    break;
                }
            }
        }
    }

    // 3. Always clean up local records and cache cleanly
    let _ = clean_claude_plugin_records(claude_home, plugin_id, repo_root);

    if !outputs.is_empty() {
        Ok(outputs.join("\n"))
    } else {
        Ok(format!("Uninstalled plugin {plugin_id}"))
    }
}

/// Add an OpenCode plugin reference to `opencode.json`.
pub fn add_opencode_plugin(plugin_ref: &str, repo_root: Option<&Path>) -> io::Result<()> {
    let cfg_path = match repo_root {
        Some(repo) => repo.join("opencode.json"),
        None => {
            let Some(dirs) = directories::BaseDirs::new() else {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no home directory"));
            };
            dirs.home_dir().join(".config/opencode/opencode.json")
        }
    };

    let mut root: Value = match fs::read_to_string(&cfg_path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or(Value::Object(Default::default())),
        Err(_) => Value::Object(Default::default()),
    };

    let Value::Object(map) = &mut root else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "opencode.json not an object",
        ));
    };

    let raw_name = plugin_ref.split('@').next().unwrap_or(plugin_ref);
    if let Some(disabled) = map
        .get_mut("disabled_plugins")
        .and_then(Value::as_array_mut)
    {
        disabled.retain(|item| {
            if let Some(s) = item.as_str() {
                let n = s.split('@').next().unwrap_or(s);
                s != plugin_ref && n != raw_name
            } else {
                true
            }
        });
    }

    let plugins = map
        .entry("plugin")
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Value::Array(arr) = plugins {
        if !arr.iter().any(|v| v.as_str() == Some(plugin_ref)) {
            arr.push(Value::String(plugin_ref.to_string()));
        }
    }

    if let Some(dir) = cfg_path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = cfg_path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(&root)?)?;
    fs::rename(&tmp, &cfg_path)
}

/// Enable an installed OpenCode plugin in `opencode.json` (moves from disabled_plugins to plugin).
pub fn enable_opencode_plugin(plugin_id: &str, repo_root: Option<&Path>) -> io::Result<()> {
    let raw_name = plugin_id.split('@').next().unwrap_or(plugin_id).trim();
    let paths = match repo_root {
        Some(repo) => vec![
            repo.join("opencode.json"),
            repo.join(".opencode/opencode.json"),
        ],
        None => {
            if let Some(dirs) = directories::BaseDirs::new() {
                vec![dirs.home_dir().join(".config/opencode/opencode.json")]
            } else {
                Vec::new()
            }
        }
    };

    for cfg_path in paths {
        let mut root: Value = match fs::read_to_string(&cfg_path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or(Value::Object(Default::default())),
            Err(_) => Value::Object(Default::default()),
        };
        let Some(map) = root.as_object_mut() else {
            continue;
        };

        let mut found_id = None;
        if let Some(disabled) = map
            .get_mut("disabled_plugins")
            .and_then(Value::as_array_mut)
        {
            disabled.retain(|item| {
                if let Some(s) = item.as_str() {
                    let n = s.split('@').next().unwrap_or(s);
                    if s == plugin_id || n == raw_name {
                        found_id = Some(s.to_string());
                        false
                    } else {
                        true
                    }
                } else {
                    true
                }
            });
        }

        let id_to_record = found_id.unwrap_or_else(|| plugin_id.to_string());
        let plugins = map
            .entry("plugin")
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(arr) = plugins {
            if !arr
                .iter()
                .any(|item| item.as_str() == Some(&id_to_record) || item.as_str() == Some(raw_name))
            {
                arr.push(Value::String(id_to_record));
            }
        }

        if let Some(parent) = cfg_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let tmp = cfg_path.with_extension("tmp");
        if let Ok(bytes) = serde_json::to_vec_pretty(&root) {
            let _ = fs::write(&tmp, bytes);
            let _ = fs::rename(&tmp, &cfg_path);
        }
    }
    Ok(())
}

/// Disable an installed OpenCode plugin in `opencode.json` (moves from plugin to disabled_plugins).
pub fn disable_opencode_plugin(plugin_id: &str, repo_root: Option<&Path>) -> io::Result<()> {
    let raw_name = plugin_id.split('@').next().unwrap_or(plugin_id).trim();
    let paths = match repo_root {
        Some(repo) => vec![
            repo.join("opencode.json"),
            repo.join(".opencode/opencode.json"),
        ],
        None => {
            if let Some(dirs) = directories::BaseDirs::new() {
                vec![dirs.home_dir().join(".config/opencode/opencode.json")]
            } else {
                Vec::new()
            }
        }
    };

    for cfg_path in paths {
        let mut root: Value = match fs::read_to_string(&cfg_path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or(Value::Object(Default::default())),
            Err(_) => Value::Object(Default::default()),
        };
        let Some(map) = root.as_object_mut() else {
            continue;
        };

        let mut found_id = None;
        if let Some(plugins) = map.get_mut("plugin").and_then(Value::as_array_mut) {
            plugins.retain(|item| {
                if let Some(s) = item.as_str() {
                    let n = s.split('@').next().unwrap_or(s);
                    if s == plugin_id || n == raw_name {
                        found_id = Some(s.to_string());
                        false
                    } else {
                        true
                    }
                } else {
                    true
                }
            });
        }

        let id_to_record = found_id.unwrap_or_else(|| plugin_id.to_string());
        let disabled = map
            .entry("disabled_plugins")
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(arr) = disabled {
            if !arr.iter().any(|item| item.as_str() == Some(&id_to_record)) {
                arr.push(Value::String(id_to_record));
            }
        }

        if let Some(parent) = cfg_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let tmp = cfg_path.with_extension("tmp");
        if let Ok(bytes) = serde_json::to_vec_pretty(&root) {
            let _ = fs::write(&tmp, bytes);
            let _ = fs::rename(&tmp, &cfg_path);
        }
    }
    Ok(())
}

/// Remove an OpenCode plugin reference from `opencode.json` (removes from both plugin and disabled_plugins).
pub fn remove_opencode_plugin(plugin_id: &str, repo_root: Option<&Path>) -> io::Result<()> {
    let raw_name = plugin_id.split('@').next().unwrap_or(plugin_id);
    let paths = match repo_root {
        Some(repo) => vec![
            repo.join("opencode.json"),
            repo.join(".opencode/opencode.json"),
        ],
        None => {
            if let Some(dirs) = directories::BaseDirs::new() {
                vec![dirs.home_dir().join(".config/opencode/opencode.json")]
            } else {
                Vec::new()
            }
        }
    };

    for cfg_path in paths {
        if let Ok(text) = fs::read_to_string(&cfg_path) {
            if let Ok(mut root) = serde_json::from_str::<Value>(&text) {
                let mut modified = false;
                if let Some(arr) = root.get_mut("plugin").and_then(Value::as_array_mut) {
                    let prev_len = arr.len();
                    arr.retain(|item| {
                        if let Some(s) = item.as_str() {
                            let name = s.split('@').next().unwrap_or(s);
                            s != plugin_id && name != raw_name
                        } else {
                            true
                        }
                    });
                    if arr.len() != prev_len {
                        modified = true;
                    }
                }
                if let Some(arr) = root
                    .get_mut("disabled_plugins")
                    .and_then(Value::as_array_mut)
                {
                    let prev_len = arr.len();
                    arr.retain(|item| {
                        if let Some(s) = item.as_str() {
                            let name = s.split('@').next().unwrap_or(s);
                            s != plugin_id && name != raw_name
                        } else {
                            true
                        }
                    });
                    if arr.len() != prev_len {
                        modified = true;
                    }
                }
                if modified {
                    let tmp = cfg_path.with_extension("tmp");
                    if let Ok(bytes) = serde_json::to_vec_pretty(&root) {
                        let _ = fs::write(&tmp, bytes);
                        let _ = fs::rename(&tmp, &cfg_path);
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn opencode_plugin_details(plugin_id: &str, _repo_root: Option<&Path>) -> String {
    let name = plugin_id.split('@').next().unwrap_or(plugin_id).trim();
    if let Some(dirs) = directories::BaseDirs::new() {
        let nm = dirs
            .home_dir()
            .join(".config/opencode/node_modules")
            .join(name);
        if let Ok(pkg) = fs::read_to_string(nm.join("package.json")) {
            return pkg;
        }
        if let Ok(readme) = fs::read_to_string(nm.join("README.md")) {
            return readme;
        }
    }
    format!("OpenCode Plugin: {plugin_id}\nConfigured in opencode.json")
}

/// Install an Antigravity plugin (directory or reference).
pub fn install_antigravity_plugin(plugin_ref: &str, repo_root: Option<&Path>) -> io::Result<()> {
    let name = plugin_ref.split('@').next().unwrap_or(plugin_ref).trim();
    if name.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid plugin name",
        ));
    }
    let target_dir = match repo_root {
        Some(repo) => repo.join(".gemini/plugins").join(name),
        None => {
            let Some(dirs) = directories::BaseDirs::new() else {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no home directory"));
            };
            dirs.home_dir().join(".gemini/plugins").join(name)
        }
    };
    if Path::new(plugin_ref).is_dir() {
        copy_dir(Path::new(plugin_ref), &target_dir)?;
    } else {
        fs::create_dir_all(&target_dir)?;
        let plugin_json = serde_json::json!({
            "name": name,
            "version": "1.0.0"
        });
        fs::write(
            target_dir.join("plugin.json"),
            serde_json::to_vec_pretty(&plugin_json)?,
        )?;
        fs::create_dir_all(target_dir.join("skills"))?;
    }
    let settings_path = match repo_root {
        Some(repo) => repo.join(".gemini/settings.json"),
        None => {
            let dirs = directories::BaseDirs::new().unwrap();
            dirs.home_dir().join(".gemini/settings.json")
        }
    };
    let _ = set_plugin_enabled(&settings_path, &format!("{name}@antigravity"), Some(true));
    let _ = set_plugin_enabled(&settings_path, name, Some(true));
    Ok(())
}

/// Remove an Antigravity plugin and clear its enabled settings.
pub fn remove_antigravity_plugin(plugin_id: &str, repo_root: Option<&Path>) -> io::Result<()> {
    let name = plugin_id.split('@').next().unwrap_or(plugin_id).trim();
    if let Some(dirs) = directories::BaseDirs::new() {
        let gemini = dirs.home_dir().join(".gemini");
        for dir in [
            gemini.join("plugins").join(name),
            gemini.join("config/plugins").join(name),
        ] {
            if dir.is_dir() {
                let _ = fs::remove_dir_all(dir);
            }
        }
        let _ = set_plugin_enabled(&gemini.join("settings.json"), plugin_id, None);
        let _ = set_plugin_enabled(&gemini.join("settings.json"), name, None);
        let _ = set_plugin_enabled(&gemini.join("config/settings.json"), plugin_id, None);
        let _ = set_plugin_enabled(&gemini.join("config/settings.json"), name, None);
    }
    if let Some(repo) = repo_root {
        let repo_dir = repo.join(".gemini/plugins").join(name);
        if repo_dir.is_dir() {
            let _ = fs::remove_dir_all(repo_dir);
        }
        let _ = set_plugin_enabled(&repo.join(".gemini/settings.json"), plugin_id, None);
        let _ = set_plugin_enabled(&repo.join(".gemini/settings.json"), name, None);
    }
    Ok(())
}

pub fn antigravity_plugin_details(plugin_id: &str, repo_root: Option<&Path>) -> String {
    let name = plugin_id.split('@').next().unwrap_or(plugin_id).trim();
    let mut candidate_dirs = Vec::new();
    if let Some(repo) = repo_root {
        candidate_dirs.push(repo.join(".gemini/plugins").join(name));
    }
    if let Some(dirs) = directories::BaseDirs::new() {
        let gemini = dirs.home_dir().join(".gemini");
        candidate_dirs.push(gemini.join("plugins").join(name));
        candidate_dirs.push(gemini.join("config/plugins").join(name));
    }
    for dir in candidate_dirs {
        if dir.is_dir() {
            if let Ok(readme) = fs::read_to_string(dir.join("README.md")) {
                return readme;
            }
            if let Ok(pj) = fs::read_to_string(dir.join("plugin.json")) {
                return pj;
            }
            let skills = count_dir(&dir.join("skills"));
            let commands = count_dir(&dir.join("commands"));
            let agents = count_dir(&dir.join("agents"));
            return format!(
                "Antigravity Plugin: {name}\nPath: {}\nSkills: {skills}, Commands: {commands}, Agents: {agents}",
                dir.display()
            );
        }
    }
    format!("Antigravity Plugin: {name} (Installed)")
}

/// Install a Codex plugin.
pub fn install_codex_plugin(plugin_ref: &str, repo_root: Option<&Path>) -> io::Result<()> {
    let name = plugin_ref.split('@').next().unwrap_or(plugin_ref).trim();
    if name.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid plugin name",
        ));
    }
    let target_dir = match repo_root {
        Some(repo) => repo.join(".codex/plugins").join(name),
        None => {
            let Some(dirs) = directories::BaseDirs::new() else {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no home directory"));
            };
            dirs.home_dir().join(".codex/plugins").join(name)
        }
    };
    if Path::new(plugin_ref).is_dir() {
        copy_dir(Path::new(plugin_ref), &target_dir)?;
    } else {
        fs::create_dir_all(&target_dir)?;
        let plugin_json = serde_json::json!({
            "name": name,
            "version": "1.0.0"
        });
        fs::write(
            target_dir.join("plugin.json"),
            serde_json::to_vec_pretty(&plugin_json)?,
        )?;
    }
    let settings_path = match repo_root {
        Some(repo) => repo.join(".codex/settings.json"),
        None => {
            let dirs = directories::BaseDirs::new().unwrap();
            dirs.home_dir().join(".codex/settings.json")
        }
    };
    let _ = set_plugin_enabled(&settings_path, &format!("{name}@codex"), Some(true));
    let _ = set_plugin_enabled(&settings_path, name, Some(true));
    Ok(())
}

/// Remove a Codex plugin and clear settings.
pub fn remove_codex_plugin(plugin_id: &str, repo_root: Option<&Path>) -> io::Result<()> {
    let name = plugin_id.split('@').next().unwrap_or(plugin_id).trim();
    if let Some(dirs) = directories::BaseDirs::new() {
        let codex = dirs.home_dir().join(".codex");
        let dir = codex.join("plugins").join(name);
        if dir.is_dir() {
            let _ = fs::remove_dir_all(dir);
        }
        let _ = set_plugin_enabled(&codex.join("settings.json"), plugin_id, None);
        let _ = set_plugin_enabled(&codex.join("settings.json"), name, None);
    }
    if let Some(repo) = repo_root {
        let repo_dir = repo.join(".codex/plugins").join(name);
        if repo_dir.is_dir() {
            let _ = fs::remove_dir_all(repo_dir);
        }
        let _ = set_plugin_enabled(&repo.join(".codex/settings.json"), plugin_id, None);
        let _ = set_plugin_enabled(&repo.join(".codex/settings.json"), name, None);
    }
    Ok(())
}

pub fn codex_plugin_details(plugin_id: &str, repo_root: Option<&Path>) -> String {
    let name = plugin_id.split('@').next().unwrap_or(plugin_id).trim();
    let mut candidate_dirs = Vec::new();
    if let Some(repo) = repo_root {
        candidate_dirs.push(repo.join(".codex/plugins").join(name));
    }
    if let Some(dirs) = directories::BaseDirs::new() {
        let codex = dirs.home_dir().join(".codex");
        candidate_dirs.push(codex.join("plugins").join(name));
    }
    for dir in candidate_dirs {
        if dir.is_dir() {
            if let Ok(readme) = fs::read_to_string(dir.join("README.md")) {
                return readme;
            }
            if let Ok(pj) = fs::read_to_string(dir.join("plugin.json")) {
                return pj;
            }
            return format!("Codex Plugin: {name}\nPath: {}", dir.display());
        }
    }
    format!("Codex Plugin: {name} (Installed)")
}

/// Install a Cursor extension record.
pub fn install_cursor_extension(ext_ref: &str, repo_root: Option<&Path>) -> io::Result<()> {
    let id = ext_ref.trim();
    if id.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid extension ID",
        ));
    }
    if let Some(dirs) = directories::BaseDirs::new() {
        let cursor_dir = dirs.home_dir().join(".cursor");
        let ext_json_path = cursor_dir.join("extensions/extensions.json");
        fs::create_dir_all(cursor_dir.join("extensions"))?;
        let mut root: Value = match fs::read_to_string(&ext_json_path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or(Value::Array(Vec::new())),
            Err(_) => Value::Array(Vec::new()),
        };
        if let Value::Array(arr) = &mut root {
            let already = arr.iter().any(|item| {
                item.get("identifier")
                    .and_then(|i| i.get("id"))
                    .and_then(Value::as_str)
                    == Some(id)
            });
            if !already {
                let entry = serde_json::json!({
                    "identifier": { "id": id },
                    "version": "1.0.0",
                    "relativeLocation": id,
                    "metadata": {
                        "installedTimestamp": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
                    }
                });
                arr.push(entry);
                let tmp = ext_json_path.with_extension("tmp");
                fs::write(&tmp, serde_json::to_vec_pretty(&root)?)?;
                fs::rename(&tmp, &ext_json_path)?;
            }
        }
        let settings_path = cursor_dir.join("settings.json");
        let _ = set_plugin_enabled(&settings_path, id, Some(true));
    }
    if let Some(repo) = repo_root {
        let _ = set_plugin_enabled(&repo.join(".cursor/settings.json"), id, Some(true));
    }
    Ok(())
}

/// Remove a Cursor extension record and directory.
pub fn remove_cursor_extension(ext_id: &str, repo_root: Option<&Path>) -> io::Result<()> {
    if let Some(dirs) = directories::BaseDirs::new() {
        let cursor_dir = dirs.home_dir().join(".cursor");
        let ext_json_path = cursor_dir.join("extensions/extensions.json");
        if let Ok(text) = fs::read_to_string(&ext_json_path) {
            if let Ok(mut root) = serde_json::from_str::<Value>(&text) {
                let mut locs_to_remove = Vec::new();
                if let Some(arr) = root.as_array_mut() {
                    arr.retain(|item| {
                        let item_id = item
                            .get("identifier")
                            .and_then(|i| i.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let rel = item
                            .get("relativeLocation")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if item_id == ext_id || rel == ext_id {
                            if !rel.is_empty() {
                                locs_to_remove.push(rel.to_string());
                            }
                            false
                        } else {
                            true
                        }
                    });
                    let tmp = ext_json_path.with_extension("tmp");
                    if let Ok(bytes) = serde_json::to_vec_pretty(&root) {
                        let _ = fs::write(&tmp, bytes);
                        let _ = fs::rename(&tmp, &ext_json_path);
                    }
                }
                for rel in locs_to_remove {
                    let dir = cursor_dir.join("extensions").join(rel);
                    if dir.is_dir() {
                        let _ = fs::remove_dir_all(dir);
                    }
                }
            }
        }
        let _ = set_plugin_enabled(&cursor_dir.join("settings.json"), ext_id, None);
    }
    if let Some(repo) = repo_root {
        let _ = set_plugin_enabled(&repo.join(".cursor/settings.json"), ext_id, None);
    }
    Ok(())
}

pub fn cursor_extension_details(ext_id: &str, _repo_root: Option<&Path>) -> String {
    if let Some(dirs) = directories::BaseDirs::new() {
        let ext_json_path = dirs.home_dir().join(".cursor/extensions/extensions.json");
        if let Some(root) = read_json(&ext_json_path) {
            if let Some(arr) = root.as_array() {
                for item in arr {
                    let item_id = item
                        .get("identifier")
                        .and_then(|i| i.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if item_id == ext_id {
                        let version = item
                            .get("version")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown");
                        let publisher = item
                            .get("metadata")
                            .and_then(|m| m.get("publisherDisplayName"))
                            .and_then(Value::as_str)
                            .unwrap_or("unknown");
                        let rel = item
                            .get("relativeLocation")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        return format!(
                            "Cursor Extension: {ext_id}\nVersion: {version}\nPublisher: {publisher}\nLocation: {rel}"
                        );
                    }
                }
            }
        }
    }
    format!("Cursor Extension: {ext_id}")
}

/// Kebab-case-ish skill names only: no separators means no traversal and no surprise dirs.
pub(crate) fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Create `skills_dir/<name>/SKILL.md` with minimal frontmatter. Errors on invalid names and
/// existing skills (never overwrites).
pub fn scaffold_skill(
    skills_dir: &Path,
    name: &str,
    description: Option<&str>,
) -> io::Result<PathBuf> {
    if !valid_skill_name(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid skill name",
        ));
    }
    let dir = skills_dir.join(name);
    if dir.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "skill already exists",
        ));
    }
    fs::create_dir_all(&dir)?;
    let description = description.unwrap_or("TODO: when to use this skill");
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n"),
    )?;
    Ok(dir)
}

/// Resolve the target directory for skills management (creation, deletion, reading)
/// given an account key, whether scope is global or repo, and optional repository root path.
pub fn skills_dir_for(
    account: Option<&str>,
    is_global: bool,
    repo_path: Option<&Path>,
) -> Option<PathBuf> {
    match account {
        Some("antigravity") => {
            if is_global {
                let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
                let gemini = home.join(".gemini");
                let cfg_skills = gemini.join("config/skills");
                let plain_skills = gemini.join("skills");
                if cfg_skills.exists() {
                    Some(cfg_skills)
                } else if plain_skills.exists() {
                    Some(plain_skills)
                } else {
                    Some(cfg_skills)
                }
            } else {
                let repo = repo_path?;
                let gem_skills = repo.join(".gemini/skills");
                let gem_config = repo.join(".gemini/config/skills");
                let agents_skills = repo.join(".agents/skills");
                if gem_skills.exists() {
                    Some(gem_skills)
                } else if gem_config.exists() {
                    Some(gem_config)
                } else if agents_skills.exists() {
                    Some(agents_skills)
                } else {
                    Some(gem_skills)
                }
            }
        }
        Some("codex") => {
            if is_global {
                let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
                Some(home.join(".codex/skills"))
            } else {
                let repo = repo_path?;
                Some(repo.join(".codex/skills"))
            }
        }
        Some("opencode") => {
            if is_global {
                let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
                Some(home.join(".config/opencode/skills"))
            } else {
                let repo = repo_path?;
                Some(repo.join(".opencode/skills"))
            }
        }
        Some("cursor") => {
            if is_global {
                let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
                Some(home.join(".cursor/skills"))
            } else {
                let repo = repo_path?;
                Some(repo.join(".cursor/skills"))
            }
        }
        _ => {
            if is_global {
                let home = claude_home_for(account)?;
                Some(home.join("skills"))
            } else {
                let repo = repo_path?;
                Some(repo.join(".claude/skills"))
            }
        }
    }
}

/// Delete a skill by name from the appropriate agent directory.
pub fn delete_skill(
    account: Option<&str>,
    is_global: bool,
    name: &str,
    repo_path: Option<&Path>,
) -> io::Result<()> {
    match account {
        Some("antigravity") => {
            let mut removed = false;
            if is_global {
                if let Some(dirs) = directories::BaseDirs::new() {
                    let home = dirs.home_dir();
                    let gemini = home.join(".gemini");
                    for dir in [gemini.join("config/skills"), gemini.join("skills")] {
                        let target = dir.join(name);
                        if target.exists() {
                            fs::remove_dir_all(target)?;
                            removed = true;
                        }
                    }
                }
            } else if let Some(repo) = repo_path {
                for dir in [
                    repo.join(".gemini/skills"),
                    repo.join(".gemini/config/skills"),
                    repo.join(".agents/skills"),
                ] {
                    let target = dir.join(name);
                    if target.exists() {
                        fs::remove_dir_all(target)?;
                        removed = true;
                    }
                }
            }
            if removed {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("skill '{name}' not found for antigravity"),
                ))
            }
        }
        Some("codex") => {
            let dir = skills_dir_for(account, is_global, repo_path).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "no codex skills directory")
            })?;
            fs::remove_dir_all(dir.join(name))
        }
        Some("opencode") => {
            let dir = skills_dir_for(account, is_global, repo_path).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "no opencode skills directory")
            })?;
            fs::remove_dir_all(dir.join(name))
        }
        Some("cursor") => {
            let dir = skills_dir_for(account, is_global, repo_path).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "no cursor skills directory")
            })?;
            fs::remove_dir_all(dir.join(name))
        }
        _ => {
            let dir = skills_dir_for(account, is_global, repo_path).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "this account has no Claude config directory",
                )
            })?;
            fs::remove_dir_all(dir.join(name))
        }
    }
}

/// Resolve `path` as canonically as possible without requiring it to exist: walk up to the
/// nearest existing ancestor, canonicalize that ancestor, then rejoin the (possibly
/// nonexistent) tail. Applying this to both sides of a comparison keeps them on the same
/// footing when an ancestor is a symlink (e.g. macOS's `/var` -> `/private/var`), whether or
/// not the full path exists yet. `pub(crate)`: also the basis for `files::worktree_path_allowed`
/// (D1/D2's containment check for `file.list`/`file.read`/`file.write`), which needs the same
/// not-yet-existing-tail tolerance for a not-yet-created file.
pub(crate) fn canonical_prefix(path: &Path) -> Option<PathBuf> {
    let mut probe = path.to_path_buf();
    let mut rest = Vec::new();
    while !probe.exists() {
        // `Path::exists()` follows symlinks, so a DANGLING symlink (target doesn't exist yet)
        // reads as absent here, same as a genuinely nonexistent future path component. Left
        // unchecked, that lets a symlink to an as-yet-nonexistent outside location sail through
        // this write-time guard and later resolve wherever the symlink points. Detect that case
        // with `symlink_metadata`, which does NOT follow symlinks: if it succeeds, something
        // (the symlink itself) really is here, dangling or not, so reject outright rather than
        // treating it as a plain future component.
        if probe.symlink_metadata().is_ok() {
            return None;
        }
        let name = probe.file_name().map(|n| n.to_os_string())?;
        rest.push(name);
        if !probe.pop() {
            return None;
        }
    }
    let mut resolved = probe.canonicalize().ok()?;
    for part in rest.iter().rev() {
        resolved.push(part);
    }
    Some(resolved)
}

/// True when `path` resolves inside one of the managed skills roots. Guards skill.read/write/
/// delete against arbitrary filesystem access through a crafted path.
pub fn skill_path_allowed(path: &Path, roots: &[PathBuf]) -> bool {
    // The file may not exist yet (write): canonicalize the nearest existing ancestor.
    let Some(resolved) = canonical_prefix(path) else {
        return false;
    };
    roots.iter().any(|root| {
        canonical_prefix(root)
            .map(|r| resolved.starts_with(&r))
            .unwrap_or(false)
    })
}

/// Worktrees of `repo_root` (excluding the root itself), via `git worktree list --porcelain`.
fn repo_worktrees(repo_root: &Path) -> io::Result<Vec<PathBuf>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["worktree", "list", "--porcelain"])
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    let root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(PathBuf::from)
        .filter(|p| p.canonicalize().map(|c| c != root).unwrap_or(true))
        .collect())
}

fn copy_dir(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?
        } else {
            fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// Mirror the repo root's `.claude/settings.local.json` and `.claude/skills/` into one worktree.
/// Copy-over semantics: deletions are handled by the mutation RPCs re-running the fan-out after
/// removing from the source, plus deleting the target path (see skill.delete).
pub fn sync_worktree(repo_root: &Path, worktree: &Path) -> io::Result<()> {
    if !worktree.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "worktree directory missing",
        ));
    }
    let src = repo_root.join(".claude");
    let dst = worktree.join(".claude");
    let settings = src.join("settings.local.json");
    if settings.is_file() {
        fs::create_dir_all(&dst)?;
        fs::copy(&settings, dst.join("settings.local.json"))?;
    }
    let skills = src.join("skills");
    // Only touch worktree skills when the source dir exists (an absent source means this repo
    // is unmanaged; touch nothing).
    if skills.is_dir() {
        // Drop worktree skills whose source is gone first, so deletes propagate.
        if let Ok(entries) = fs::read_dir(dst.join("skills")) {
            for entry in entries.flatten() {
                if !skills.join(entry.file_name()).exists() {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
        copy_dir(&skills, &dst.join("skills"))?
    }
    Ok(())
}

/// Push the repo root's `.claude` to every lane worktree, best-effort per lane.
pub fn fan_out(repo_root: &Path) -> FanoutSummary {
    let mut summary = FanoutSummary::default();
    let worktrees = match repo_worktrees(repo_root) {
        Ok(w) => w,
        Err(e) => {
            summary.skipped_lanes.push(SkippedLane {
                lane: repo_root.display().to_string(),
                reason: format!("git worktree list failed: {e}"),
            });
            return summary;
        }
    };
    for wt in worktrees {
        let lane = wt
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| wt.display().to_string());
        match sync_worktree(repo_root, &wt) {
            Ok(()) => summary.synced_lanes.push(lane),
            Err(e) => summary.skipped_lanes.push(SkippedLane {
                lane,
                reason: e.to_string(),
            }),
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@e.com")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@e.com")
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    fn fixture_home(dir: &Path) {
        let skills = dir.join("skills/my-skill");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(
            skills.join("SKILL.md"),
            "---\nname: my-skill\ndescription: does things\n---\nbody\n",
        )
        .unwrap();
        let plugins = dir.join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        std::fs::write(
            plugins.join("installed_plugins.json"),
            serde_json::json!({
                "version": 2,
                "plugins": {
                    "superpowers@official": [
                        { "scope": "user", "installPath": "/nonexistent", "version": "6.1.1" }
                    ]
                }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            plugins.join("known_marketplaces.json"),
            serde_json::json!({
                "official": {
                    "source": { "source": "github", "repo": "anthropics/claude-plugins-official" },
                    "lastUpdated": "2026-07-23T15:51:10.082Z"
                }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            dir.join("settings.json"),
            serde_json::json!({
                "model": "opus",
                "enabledPlugins": { "superpowers@official": true, "ghost@official": false }
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn scan_reads_global_skills_plugins_marketplaces() {
        let tmp = tempfile::tempdir().unwrap();
        fixture_home(tmp.path());
        let snap = scan(tmp.path(), None, None);

        assert_eq!(snap.skills.len(), 1);
        assert_eq!(snap.skills[0].name, "my-skill");
        assert_eq!(snap.skills[0].description.as_deref(), Some("does things"));
        assert!(matches!(snap.skills[0].source, SkillSource::User));

        let sp = snap
            .plugins
            .iter()
            .find(|p| p.id == "superpowers@official")
            .unwrap();
        assert!(sp.enabled && sp.installed);
        assert_eq!(sp.version.as_deref(), Some("6.1.1"));
        assert_eq!(sp.marketplace, "official");
        assert!(matches!(sp.enabled_source, EnabledSource::Global));
        // Enabled-map entry with no install record still shows up, marked uninstalled.
        let ghost = snap
            .plugins
            .iter()
            .find(|p| p.id == "ghost@official")
            .unwrap();
        assert!(!ghost.enabled && !ghost.installed);

        assert_eq!(snap.marketplaces.len(), 1);
        assert_eq!(snap.marketplaces[0].kind, "github");
        assert_eq!(
            snap.marketplaces[0].reference,
            "anthropics/claude-plugins-official"
        );
    }

    #[test]
    fn scan_repo_scope_overrides_global_and_adds_project_skills() {
        let home = tempfile::tempdir().unwrap();
        fixture_home(home.path());
        let repo = tempfile::tempdir().unwrap();
        let proj_skills = repo.path().join(".claude/skills/verify");
        std::fs::create_dir_all(&proj_skills).unwrap();
        std::fs::write(proj_skills.join("SKILL.md"), "---\nname: verify\n---\n").unwrap();
        std::fs::write(
            repo.path().join(".claude/settings.local.json"),
            serde_json::json!({ "enabledPlugins": { "superpowers@official": false } }).to_string(),
        )
        .unwrap();

        let snap = scan(home.path(), Some(repo.path()), None);
        let sp = snap
            .plugins
            .iter()
            .find(|p| p.id == "superpowers@official")
            .unwrap();
        assert!(!sp.enabled, "repo settings must override global");
        assert!(matches!(sp.enabled_source, EnabledSource::Repo));
        assert!(
            snap.skills
                .iter()
                .any(|s| s.name == "verify" && matches!(s.source, SkillSource::Project))
        );
        assert!(
            snap.skills
                .iter()
                .any(|s| s.name == "my-skill" && matches!(s.source, SkillSource::User))
        );
    }

    #[test]
    fn scan_of_empty_home_is_empty_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let snap = scan(tmp.path(), None, None);
        assert!(snap.plugins.is_empty() && snap.skills.is_empty() && snap.marketplaces.is_empty());
    }

    #[test]
    fn frontmatter_handles_block_scalar_descriptions() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skills/block");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: block\ndescription: |\n  First line of description.\n  Second line.\nother: x\n---\nbody\n",
        )
        .unwrap();
        let skills = scan_skills(&tmp.path().join("skills"), SkillSource::User);
        assert_eq!(
            skills[0].description.as_deref(),
            Some("First line of description. Second line.")
        );
    }

    #[test]
    fn toggle_preserves_every_other_settings_key() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = tmp.path().join("settings.json");
        std::fs::write(
            &settings,
            serde_json::json!({
                "model": "opus",
                "permissions": { "allow": ["Bash"] },
                "enabledPlugins": { "a@m": true }
            })
            .to_string(),
        )
        .unwrap();

        set_plugin_enabled(&settings, "b@m", Some(false)).unwrap();
        let after: Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(after["model"], "opus");
        assert_eq!(after["permissions"]["allow"][0], "Bash");
        assert_eq!(after["enabledPlugins"]["a@m"], true);
        assert_eq!(after["enabledPlugins"]["b@m"], false);

        set_plugin_enabled(&settings, "a@m", None).unwrap();
        let after: Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert!(after["enabledPlugins"].get("a@m").is_none());
    }

    #[test]
    fn toggle_creates_missing_settings_file_and_parents() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = tmp.path().join("deep/.claude/settings.local.json");
        set_plugin_enabled(&settings, "a@m", Some(true)).unwrap();
        let after: Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(after["enabledPlugins"]["a@m"], true);
    }

    #[test]
    fn toggle_refuses_corrupt_settings_rather_than_clobbering() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = tmp.path().join("settings.json");
        std::fs::write(&settings, "not json {").unwrap();
        assert!(set_plugin_enabled(&settings, "a@m", Some(true)).is_err());
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), "not json {");
    }

    #[test]
    fn fan_out_copies_settings_and_skills_to_every_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        std::fs::write(repo.join("README"), "x").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);
        let wt1 = tmp.path().join("wt1");
        let wt2 = tmp.path().join("wt2");
        git(
            &repo,
            &["worktree", "add", wt1.to_str().unwrap(), "-b", "l1"],
        );
        git(
            &repo,
            &["worktree", "add", wt2.to_str().unwrap(), "-b", "l2"],
        );

        // Repo-root .claude is the source of truth (gitignored files included).
        let src_skills = repo.join(".claude/skills/verify");
        std::fs::create_dir_all(&src_skills).unwrap();
        std::fs::write(src_skills.join("SKILL.md"), "---\nname: verify\n---\n").unwrap();
        set_plugin_enabled(&repo.join(".claude/settings.local.json"), "a@m", Some(true)).unwrap();

        let summary = fan_out(&repo);
        assert_eq!(
            summary.synced_lanes.len(),
            2,
            "skipped: {:?}",
            summary.skipped_lanes
        );
        for wt in [&wt1, &wt2] {
            assert!(wt.join(".claude/skills/verify/SKILL.md").is_file());
            let s: Value = serde_json::from_str(
                &std::fs::read_to_string(wt.join(".claude/settings.local.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(s["enabledPlugins"]["a@m"], true);
        }
    }

    #[test]
    fn fan_out_reports_unsyncable_worktrees_without_failing_others() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        std::fs::write(repo.join("README"), "x").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);
        let wt1 = tmp.path().join("wt1");
        git(
            &repo,
            &["worktree", "add", wt1.to_str().unwrap(), "-b", "l1"],
        );
        set_plugin_enabled(&repo.join(".claude/settings.local.json"), "a@m", Some(true)).unwrap();
        // Simulate a vanished worktree dir (git still lists it).
        std::fs::remove_dir_all(&wt1).unwrap();

        let summary = fan_out(&repo);
        assert!(summary.synced_lanes.is_empty());
        assert_eq!(summary.skipped_lanes.len(), 1);
        assert_eq!(summary.skipped_lanes[0].lane, "wt1");
    }

    #[test]
    fn fan_out_mixed_batch_syncs_survivors_and_reports_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        std::fs::write(repo.join("README"), "x").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);
        let wt_ok = tmp.path().join("wt_ok");
        let wt_gone = tmp.path().join("wt_gone");
        git(
            &repo,
            &["worktree", "add", wt_ok.to_str().unwrap(), "-b", "ok"],
        );
        git(
            &repo,
            &["worktree", "add", wt_gone.to_str().unwrap(), "-b", "gone"],
        );

        // Write source settings to repo root.
        set_plugin_enabled(&repo.join(".claude/settings.local.json"), "a@m", Some(true)).unwrap();

        // Remove wt_gone directory to simulate a mixed batch.
        std::fs::remove_dir_all(&wt_gone).unwrap();

        let summary = fan_out(&repo);
        assert_eq!(summary.synced_lanes, vec!["wt_ok"]);
        assert_eq!(summary.skipped_lanes.len(), 1);
        assert_eq!(summary.skipped_lanes[0].lane, "wt_gone");
        assert!(wt_ok.join(".claude/settings.local.json").is_file());
    }

    /// A fake `claude` CLI that prints `out`, prints `err` to stderr, and exits with `code` —
    /// a real runnable program on each OS (sh script on Unix, `.cmd` on Windows, where a
    /// shebang script is not executable and `CreateProcess` fails with error 193).
    fn fake_claude(dir: &Path, out: &str, err: &str, code: i32) -> ClaudeCli {
        #[cfg(not(windows))]
        let bin = {
            let bin = dir.join("claude");
            let mut script = String::from("#!/bin/sh\n");
            if !out.is_empty() {
                script.push_str(&format!("echo {out}\n"));
            }
            if !err.is_empty() {
                script.push_str(&format!("echo {err} >&2\n"));
            }
            script.push_str(&format!("exit {code}\n"));
            std::fs::write(&bin, script).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            bin
        };
        #[cfg(windows)]
        let bin = {
            let bin = dir.join("claude.cmd");
            let mut script = String::from("@echo off\r\n");
            if !out.is_empty() {
                script.push_str(&format!("echo {out}\r\n"));
            }
            if !err.is_empty() {
                script.push_str(&format!("echo {err} 1>&2\r\n"));
            }
            script.push_str(&format!("exit /b {code}\r\n"));
            std::fs::write(&bin, script).unwrap();
            bin
        };
        ClaudeCli {
            bin,
            version: "9.9.9-test".to_string(),
        }
    }

    #[test]
    fn cli_run_captures_stdout_on_success() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = fake_claude(tmp.path(), "installed ok", "", 0);
        assert_eq!(
            cli.run(&["plugin", "install", "x@m"]).unwrap().trim(),
            "installed ok"
        );
    }

    #[test]
    fn cli_run_surfaces_stderr_and_exit_code_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = fake_claude(tmp.path(), "", "boom", 3);
        let err = cli.run(&["plugin", "install", "x@m"]).unwrap_err();
        assert_eq!(err.exit_code, Some(3));
        assert!(err.stderr.contains("boom"));
    }

    #[test]
    fn scaffold_skill_writes_frontmatter_and_rejects_bad_names() {
        let tmp = tempfile::tempdir().unwrap();
        let path = scaffold_skill(tmp.path(), "my-skill", Some("does x")).unwrap();
        let text = std::fs::read_to_string(path.join("SKILL.md")).unwrap();
        assert!(text.starts_with("---\nname: my-skill\ndescription: does x\n---\n"));
        assert!(
            scaffold_skill(tmp.path(), "my-skill", None).is_err(),
            "duplicate must fail"
        );
        assert!(scaffold_skill(tmp.path(), "../escape", None).is_err());
        assert!(scaffold_skill(tmp.path(), "has space", None).is_err());
    }

    #[test]
    fn skill_path_guard_only_allows_managed_roots() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let ok = home.path().join("skills/a/SKILL.md");
        let ok2 = repo.path().join(".claude/skills/b/SKILL.md");
        let bad = home.path().join("settings.json");
        let roots = [
            home.path().join("skills"),
            repo.path().join(".claude/skills"),
        ];
        assert!(skill_path_allowed(&ok, &roots));
        assert!(skill_path_allowed(&ok2, &roots));
        assert!(!skill_path_allowed(&bad, &roots));
        assert!(!skill_path_allowed(Path::new("/etc/passwd"), &roots));
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_leaf_cannot_escape_the_roots() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let skills = root.path().join("skills");
        let dir = skills.join("evil");
        std::fs::create_dir_all(&dir).unwrap();
        let leaf = dir.join("SKILL.md");
        std::os::unix::fs::symlink(outside.path().join("planted"), &leaf).unwrap();
        assert!(!skill_path_allowed(&leaf, std::slice::from_ref(&skills)));

        // Existing behavior: a symlink leaf pointing at an EXISTING outside file is already
        // rejected via full canonicalization (the target resolves outside every root).
        let planted = outside.path().join("planted");
        std::fs::write(&planted, "secret").unwrap();
        let existing_leaf = dir.join("SKILL2.md");
        std::os::unix::fs::symlink(&planted, &existing_leaf).unwrap();
        assert!(!skill_path_allowed(&existing_leaf, &[skills]));
    }

    #[test]
    fn sync_prunes_worktree_skills_deleted_at_the_source() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        std::fs::write(repo.join("README"), "x").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);
        let wt = tmp.path().join("wt");
        git(
            &repo,
            &["worktree", "add", wt.to_str().unwrap(), "-b", "l1"],
        );

        for name in ["keep", "drop"] {
            let dir = repo.join(".claude/skills").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), "---\nname: s\n---\n").unwrap();
        }
        fan_out(&repo);
        assert!(wt.join(".claude/skills/drop/SKILL.md").is_file());

        std::fs::remove_dir_all(repo.join(".claude/skills/drop")).unwrap();
        let summary = fan_out(&repo);
        assert_eq!(summary.synced_lanes.len(), 1);
        assert!(wt.join(".claude/skills/keep/SKILL.md").is_file());
        assert!(!wt.join(".claude/skills/drop").exists());
    }

    #[test]
    fn test_scan_for_account_antigravity() {
        let repo = tempfile::tempdir().unwrap();
        let skill_dir = repo.path().join(".gemini/skills/demo-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo-skill\ndescription: A demo antigravity skill\n---\n# Body\n",
        )
        .unwrap();

        let snap = scan_for_account("antigravity", Some(repo.path()), None);
        assert_eq!(snap.account, "antigravity");
        let found = snap.skills.iter().find(|s| s.name == "demo-skill");
        assert!(found.is_some());
        assert_eq!(
            found.unwrap().description.as_deref(),
            Some("A demo antigravity skill")
        );
        assert_eq!(found.unwrap().source, SkillSource::Project);
    }

    #[test]
    fn test_scan_for_account_opencode() {
        let repo = tempfile::tempdir().unwrap();
        let cfg_path = repo.path().join("opencode.json");
        std::fs::write(
            &cfg_path,
            r#"{"plugin":["oh-my-openagent@latest","helper-tool@2.0.0"]}"#,
        )
        .unwrap();

        let snap = scan_for_account("opencode", Some(repo.path()), None);
        assert_eq!(snap.account, "opencode");
        assert_eq!(snap.plugins.len(), 2);
        let p0 = &snap.plugins[0];
        assert_eq!(p0.name, "helper-tool");
        assert_eq!(p0.version.as_deref(), Some("2.0.0"));
        let p1 = &snap.plugins[1];
        assert_eq!(p1.name, "oh-my-openagent");
        assert_eq!(p1.version.as_deref(), Some("latest"));
    }

    #[test]
    fn test_scan_for_account_cursor() {
        let repo = tempfile::tempdir().unwrap();
        let skill_dir = repo.path().join(".cursor/skills/cursor-analysis");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: cursor-analysis\ndescription: Analysis tools for Cursor\n---\n",
        )
        .unwrap();

        let snap = scan_for_account("cursor", Some(repo.path()), None);
        assert_eq!(snap.account, "cursor");
        let found = snap.skills.iter().find(|s| s.name == "cursor-analysis");
        assert!(found.is_some());
        assert_eq!(
            found.unwrap().description.as_deref(),
            Some("Analysis tools for Cursor")
        );
    }

    #[test]
    fn test_opencode_plugin_add_and_remove() {
        let repo = tempfile::tempdir().unwrap();
        add_opencode_plugin("sample-tool@1.0.0", Some(repo.path())).unwrap();

        let snap = scan_for_account("opencode", Some(repo.path()), None);
        let found = snap.plugins.iter().find(|p| p.name == "sample-tool");
        assert!(found.is_some());
        assert_eq!(found.unwrap().version.as_deref(), Some("1.0.0"));

        remove_opencode_plugin("sample-tool@1.0.0", Some(repo.path())).unwrap();
        let snap2 = scan_for_account("opencode", Some(repo.path()), None);
        assert!(!snap2.plugins.iter().any(|p| p.name == "sample-tool"));
    }

    #[test]
    fn test_clean_claude_plugin_records() {
        let home = tempfile::tempdir().unwrap();
        let plugins_dir = home.path().join("plugins");
        std::fs::create_dir_all(&plugins_dir).unwrap();
        std::fs::write(
            plugins_dir.join("installed_plugins.json"),
            r#"{"version":2,"plugins":{"test-plugin@official":[{"scope":"user"}]}}"#,
        )
        .unwrap();
        std::fs::write(
            home.path().join("settings.json"),
            r#"{"enabledPlugins":{"test-plugin@official":true}}"#,
        )
        .unwrap();

        clean_claude_plugin_records(home.path(), "test-plugin@official", None).unwrap();

        let installed_txt =
            std::fs::read_to_string(plugins_dir.join("installed_plugins.json")).unwrap();
        assert!(!installed_txt.contains("test-plugin@official"));
        let settings_txt = std::fs::read_to_string(home.path().join("settings.json")).unwrap();
        assert!(!settings_txt.contains("test-plugin@official"));
    }

    #[test]
    fn test_delete_skill_antigravity_and_multiagent() {
        let repo = tempfile::tempdir().unwrap();
        let gem_skill = repo.path().join(".gemini/skills/my-gem-skill");
        std::fs::create_dir_all(&gem_skill).unwrap();
        std::fs::write(gem_skill.join("SKILL.md"), "---\nname: my-gem-skill\n---\n").unwrap();

        assert!(gem_skill.exists());
        delete_skill(
            Some("antigravity"),
            false,
            "my-gem-skill",
            Some(repo.path()),
        )
        .unwrap();
        assert!(!gem_skill.exists());

        let codex_skill = repo.path().join(".codex/skills/my-codex-skill");
        std::fs::create_dir_all(&codex_skill).unwrap();
        std::fs::write(
            codex_skill.join("SKILL.md"),
            "---\nname: my-codex-skill\n---\n",
        )
        .unwrap();
        assert!(codex_skill.exists());
        delete_skill(Some("codex"), false, "my-codex-skill", Some(repo.path())).unwrap();
        assert!(!codex_skill.exists());
    }

    #[test]
    fn test_opencode_enable_disable_toggle() {
        let repo = tempfile::tempdir().unwrap();
        add_opencode_plugin("sample-tool@1.0.0", Some(repo.path())).unwrap();

        let snap1 = scan_for_account("opencode", Some(repo.path()), None);
        let p1 = snap1
            .plugins
            .iter()
            .find(|p| p.name == "sample-tool")
            .unwrap();
        assert!(p1.enabled);
        assert!(p1.installed);

        // Disable plugin: should not delete, but mark disabled
        disable_opencode_plugin("sample-tool@1.0.0", Some(repo.path())).unwrap();
        let snap2 = scan_for_account("opencode", Some(repo.path()), None);
        let p2 = snap2
            .plugins
            .iter()
            .find(|p| p.name == "sample-tool")
            .unwrap();
        assert!(!p2.enabled);
        assert!(p2.installed);

        // Re-enable plugin
        enable_opencode_plugin("sample-tool@1.0.0", Some(repo.path())).unwrap();
        let snap3 = scan_for_account("opencode", Some(repo.path()), None);
        let p3 = snap3
            .plugins
            .iter()
            .find(|p| p.name == "sample-tool")
            .unwrap();
        assert!(p3.enabled);

        // Remove plugin completely
        remove_opencode_plugin("sample-tool@1.0.0", Some(repo.path())).unwrap();
        let snap4 = scan_for_account("opencode", Some(repo.path()), None);
        assert!(!snap4.plugins.iter().any(|p| p.name == "sample-tool"));
    }

    #[test]
    fn test_antigravity_plugin_lifecycle() {
        let repo = tempfile::tempdir().unwrap();
        install_antigravity_plugin("test-gem-plugin", Some(repo.path())).unwrap();

        let plugin_dir = repo.path().join(".gemini/plugins/test-gem-plugin");
        assert!(plugin_dir.join("plugin.json").is_file());

        let snap1 = scan_for_account("antigravity", Some(repo.path()), None);
        let p1 = snap1
            .plugins
            .iter()
            .find(|p| p.name == "test-gem-plugin")
            .unwrap();
        assert!(p1.enabled);

        // Disable via settings
        let settings = repo.path().join(".gemini/settings.json");
        set_plugin_enabled(&settings, "test-gem-plugin@antigravity", Some(false)).unwrap();

        let snap2 = scan_for_account("antigravity", Some(repo.path()), None);
        let p2 = snap2
            .plugins
            .iter()
            .find(|p| p.name == "test-gem-plugin")
            .unwrap();
        assert!(!p2.enabled);

        // Remove
        remove_antigravity_plugin("test-gem-plugin@antigravity", Some(repo.path())).unwrap();
        assert!(!plugin_dir.exists());
    }
}
