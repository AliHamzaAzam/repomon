//! Claude Code extension management: config scanning, enabledPlugins toggles, and repo-to-worktree
//! fan-out. The daemon is the single authority; the GUI and TUI only speak the ext RPCs.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use repomon_core::model::{
    EnabledSource, ExtSnapshot, FanoutSummary, MarketplaceInfo, PluginInfo, PluginProvides,
    SkillInfo, SkillSource, SkippedLane,
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
        let out = std::process::Command::new(&self.bin)
            .args(args)
            .output()
            .map_err(|e| CliFailure {
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

/// Kebab-case-ish skill names only: no separators means no traversal and no surprise dirs.
fn valid_skill_name(name: &str) -> bool {
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

/// Resolve `path` as canonically as possible without requiring it to exist: walk up to the
/// nearest existing ancestor, canonicalize that ancestor, then rejoin the (possibly
/// nonexistent) tail. Applying this to both sides of a comparison keeps them on the same
/// footing when an ancestor is a symlink (e.g. macOS's `/var` -> `/private/var`), whether or
/// not the full path exists yet.
fn canonical_prefix(path: &Path) -> Option<PathBuf> {
    let mut probe = path.to_path_buf();
    let mut rest = Vec::new();
    while !probe.exists() {
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

    fn fake_claude(dir: &Path, script: &str) -> ClaudeCli {
        let bin = dir.join("claude");
        std::fs::write(&bin, format!("#!/bin/sh\n{script}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        ClaudeCli {
            bin,
            version: "9.9.9-test".to_string(),
        }
    }

    #[test]
    fn cli_run_captures_stdout_on_success() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = fake_claude(tmp.path(), "echo installed ok");
        assert_eq!(
            cli.run(&["plugin", "install", "x@m"]).unwrap().trim(),
            "installed ok"
        );
    }

    #[test]
    fn cli_run_surfaces_stderr_and_exit_code_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = fake_claude(tmp.path(), "echo boom >&2; exit 3");
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
}
