# Extensions Manager Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** GUI management of Claude Code skills and plugins in Repomon Desktop, global and per-repo, backed by new daemon RPCs.

**Architecture:** The daemon owns all operations: it scans Claude Code config files for a live view, edits `enabledPlugins` and skill directories for toggles/authoring, and shells to the `claude` CLI for install/update/marketplace ops. Repo scope treats the repo-root `.claude` as source of truth and fans changes out to every lane's worktree. The GUI adds a dedicated Extensions view (unified searchable inventory + detail drawer) plus right-click quick toggles on fleet-sidebar repo rows.

**Tech Stack:** Rust (repomon-daemon, repomon-core, ts-rs 12 bindings), SolidJS + Tailwind, vitest, tokio.

**Spec:** `docs/superpowers/specs/2026-07-24-extensions-manager-design.md`

## Global Constraints

- Never use em-dashes in code, comments, UI copy, or commit messages.
- Commit as "Ali Hamza Azam" (already the git user); no Co-Authored-By trailer needed in this plan's commits.
- `cargo fmt --check` is CI-enforced: run `cargo fmt` before every Rust commit.
- Frontend package manager is `bun`; run frontend commands from `apps/desktop/`.
- Rust commands run from the repo root.
- The daemon is the API authority: the GUI never touches config files directly.
- Verified CLI facts (do not re-derive): `claude plugin install|enable|disable|details|new` and `claude plugin marketplace add|remove|update` exist; `install -s user|project|local` selects scope; `details <name>` prints inventory + token cost.
- Design decision baked in: plugin installs ALWAYS use `-s user` (global cache + registration), and per-repo activation is controlled solely via `enabledPlugins` in the repo's `.claude/settings.local.json`. This avoids `installed_plugins.json` `projectPath` records that would not match worktree paths.
- v1 deviation from spec, agreed: no `skill.reveal` RPC; the drawer shows the skill path with a copy button instead (avoids adding the Tauri opener plugin).

## File Structure

- `crates/repomon-core/src/model.rs`: new model types (ts-rs exported).
- `crates/repomon-daemon/src/ext.rs` (new): scanner, settings writer, fan-out, CLI runner.
- `crates/repomon-daemon/src/lib.rs`: `pub mod ext;`.
- `crates/repomon-daemon/src/rpc.rs`: new dispatch arms.
- `crates/repomon-daemon/tests/integration.rs`: end-to-end RPC tests.
- `apps/desktop/src/bindings/`: regenerated ts-rs types.
- `apps/desktop/src/ipc/rpc.ts`: RpcMap entries.
- `apps/desktop/src/stores/extensions.ts` (new) + `extensions.test.ts`: view state.
- `apps/desktop/src/components/ExtensionsView.tsx` (new): scope tabs, search, chips, unified list.
- `apps/desktop/src/components/ExtensionDrawer.tsx` (new): detail drawer.
- `apps/desktop/src/components/SkillEditorModal.tsx` (new): markdown editor.
- `apps/desktop/src/components/RepoExtMenu.tsx` (new): right-click quick toggles.
- `apps/desktop/src/App.tsx`, `apps/desktop/src/components/FleetSidebar.tsx`: wiring.

---

### Task 1: Model types and TS bindings

**Files:**
- Modify: `crates/repomon-core/src/model.rs` (append at end)
- Modify: `apps/desktop/src/bindings/index.ts`

**Interfaces:**
- Produces: `ExtSnapshot { cli_version, marketplaces, plugins, skills }`, `PluginInfo`, `PluginProvides`, `SkillInfo`, `MarketplaceInfo`, `EnabledSource`, `SkillSource`, `SkippedLane`, `FanoutSummary`. Later Rust tasks import these from `repomon_core::model`; the frontend imports the generated types from `../bindings`.

- [ ] **Step 1: Append the types to `model.rs`**

Follow the file's existing pattern exactly (see `Repo` at the top of the file for reference). Append:

```rust
/// Where a plugin's enabled/disabled value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "snake_case")]
pub enum EnabledSource {
    Global,
    Repo,
    Default,
}

/// Component counts inside an installed plugin's cache directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct PluginProvides {
    pub skills: u32,
    pub commands: u32,
    pub agents: u32,
}

/// One Claude Code plugin as seen by the extensions manager.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct PluginInfo {
    /// Full id, e.g. "superpowers@claude-plugins-official".
    pub id: String,
    pub name: String,
    pub marketplace: String,
    pub version: Option<String>,
    pub enabled: bool,
    pub enabled_source: EnabledSource,
    pub provides: Option<PluginProvides>,
    /// False when the plugin appears in enabledPlugins but has no install record.
    pub installed: bool,
}

/// Where a skill lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    User,
    Project,
}

/// One standalone skill (SKILL.md directory) as seen by the extensions manager.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct SkillInfo {
    pub name: String,
    pub description: Option<String>,
    pub source: SkillSource,
    pub path: PathBuf,
}

/// One configured plugin marketplace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct MarketplaceInfo {
    pub name: String,
    /// "github", "url", or "local".
    pub kind: String,
    /// Repo slug, URL, or path.
    pub reference: String,
    pub last_updated: Option<String>,
}

/// The full extensions snapshot for one scope, returned by `ext.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct ExtSnapshot {
    /// None when the `claude` CLI is not on PATH (install/update UI disables).
    pub cli_version: Option<String>,
    pub marketplaces: Vec<MarketplaceInfo>,
    pub plugins: Vec<PluginInfo>,
    pub skills: Vec<SkillInfo>,
}

/// A lane worktree the repo-scope fan-out could not sync.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct SkippedLane {
    pub lane: String,
    pub reason: String,
}

/// Result of fanning a repo-scope change out to lane worktrees.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct FanoutSummary {
    pub synced_lanes: Vec<String>,
    pub skipped_lanes: Vec<SkippedLane>,
}
```

- [ ] **Step 2: Regenerate bindings and verify**

```bash
cd apps/desktop/src-tauri && TS_RS_EXPORT_DIR=../../apps/desktop/src/bindings cargo test -p repomon-core --features ts export_bindings --locked
```

Run from wherever CI runs it (check `.github/workflows/ci.yml:33` for the exact working directory; mirror it). Expected: new files `ExtSnapshot.ts`, `PluginInfo.ts`, `PluginProvides.ts`, `SkillInfo.ts`, `MarketplaceInfo.ts`, `EnabledSource.ts`, `SkillSource.ts`, `SkippedLane.ts`, `FanoutSummary.ts` in `apps/desktop/src/bindings/`.

- [ ] **Step 3: Export from `bindings/index.ts`**

Add (alphabetical position, matching existing style):

```ts
export type { EnabledSource } from "./EnabledSource";
export type { ExtSnapshot } from "./ExtSnapshot";
export type { FanoutSummary } from "./FanoutSummary";
export type { MarketplaceInfo } from "./MarketplaceInfo";
export type { PluginInfo } from "./PluginInfo";
export type { PluginProvides } from "./PluginProvides";
export type { SkillInfo } from "./SkillInfo";
export type { SkillSource } from "./SkillSource";
export type { SkippedLane } from "./SkippedLane";
```

- [ ] **Step 4: Verify everything compiles**

```bash
cargo check -p repomon-core --features ts && cd apps/desktop && bun run check
```

Expected: both pass.

- [ ] **Step 5: Commit**

```bash
git add crates/repomon-core/src/model.rs apps/desktop/src/bindings
git commit -m "feat(core): extension manager model types with ts bindings"
```

---

### Task 2: Config scanner (`ext::scan`)

**Files:**
- Create: `crates/repomon-daemon/src/ext.rs`
- Modify: `crates/repomon-daemon/src/lib.rs` (add `pub mod ext;` beside the other mods at lines 7-17)
- Modify: `crates/repomon-daemon/Cargo.toml` (add `directories = { workspace = true }` under `[dependencies]`)

**Interfaces:**
- Produces: `ext::claude_home() -> Option<PathBuf>`; `ext::scan(claude_home: &Path, repo_root: Option<&Path>) -> ExtSnapshot`. Test code and RPC arms call these.
- Consumes: Task 1 types.

- [ ] **Step 1: Write the failing tests** (in `ext.rs` `#[cfg(test)] mod tests`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

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
        let snap = scan(tmp.path(), None);

        assert_eq!(snap.skills.len(), 1);
        assert_eq!(snap.skills[0].name, "my-skill");
        assert_eq!(snap.skills[0].description.as_deref(), Some("does things"));
        assert!(matches!(snap.skills[0].source, SkillSource::User));

        let sp = snap.plugins.iter().find(|p| p.id == "superpowers@official").unwrap();
        assert!(sp.enabled && sp.installed);
        assert_eq!(sp.version.as_deref(), Some("6.1.1"));
        assert_eq!(sp.marketplace, "official");
        assert!(matches!(sp.enabled_source, EnabledSource::Global));
        // Enabled-map entry with no install record still shows up, marked uninstalled.
        let ghost = snap.plugins.iter().find(|p| p.id == "ghost@official").unwrap();
        assert!(!ghost.enabled && !ghost.installed);

        assert_eq!(snap.marketplaces.len(), 1);
        assert_eq!(snap.marketplaces[0].kind, "github");
        assert_eq!(snap.marketplaces[0].reference, "anthropics/claude-plugins-official");
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

        let snap = scan(home.path(), Some(repo.path()));
        let sp = snap.plugins.iter().find(|p| p.id == "superpowers@official").unwrap();
        assert!(!sp.enabled, "repo settings must override global");
        assert!(matches!(sp.enabled_source, EnabledSource::Repo));
        assert!(snap.skills.iter().any(|s| s.name == "verify" && matches!(s.source, SkillSource::Project)));
        assert!(snap.skills.iter().any(|s| s.name == "my-skill" && matches!(s.source, SkillSource::User)));
    }

    #[test]
    fn scan_of_empty_home_is_empty_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let snap = scan(tmp.path(), None);
        assert!(snap.plugins.is_empty() && snap.skills.is_empty() && snap.marketplaces.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p repomon-daemon ext:: 2>&1 | tail -5
```

Expected: compile error (`scan` not defined).

- [ ] **Step 3: Implement the scanner**

```rust
//! Claude Code extension management: config scanning, enabledPlugins toggles, and repo-to-worktree
//! fan-out. The daemon is the single authority; the GUI and TUI only speak the ext RPCs.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use repomon_core::model::{
    EnabledSource, ExtSnapshot, MarketplaceInfo, PluginInfo, PluginProvides, SkillInfo, SkillSource,
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

/// Parse the `name:`/`description:` frontmatter lines from a SKILL.md.
fn skill_frontmatter(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(text) = fs::read_to_string(path) else { return (None, None) };
    let (mut name, mut description, mut in_fm) = (None, None, false);
    for line in text.lines() {
        let t = line.trim();
        if t == "---" {
            if in_fm { break }
            in_fm = true;
            continue;
        }
        if !in_fm { continue }
        if let Some(v) = t.strip_prefix("name:") { name = Some(v.trim().to_string()) }
        else if let Some(v) = t.strip_prefix("description:") { description = Some(v.trim().to_string()) }
    }
    (name, description)
}

fn scan_skills(dir: &Path, source: SkillSource) -> Vec<SkillInfo> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let md = path.join("SKILL.md");
        if !md.is_file() { continue }
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
    fs::read_dir(path).map(|d| d.flatten().count() as u32).unwrap_or(0)
}

/// Installed plugin records: id -> (version, install_path). First instance wins (the cache is
/// shared; instances differ only in scope bookkeeping we deliberately ignore).
fn installed_plugins(claude_home: &Path) -> BTreeMap<String, (Option<String>, Option<PathBuf>)> {
    let mut out = BTreeMap::new();
    let Some(root) = read_json(&claude_home.join("plugins/installed_plugins.json")) else {
        return out;
    };
    let Some(plugins) = root.get("plugins").and_then(Value::as_object) else { return out };
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
    let Some(map) = root.as_object() else { return Vec::new() };
    map.iter()
        .map(|(name, m)| {
            let source = m.get("source");
            let kind = source
                .and_then(|s| s.get("source"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let reference = source
                .and_then(|s| s.get("repo").or_else(|| s.get("url")).or_else(|| s.get("path")))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            MarketplaceInfo {
                name: name.clone(),
                kind,
                reference,
                last_updated: m.get("lastUpdated").and_then(Value::as_str).map(String::from),
            }
        })
        .collect()
}

/// Build the full snapshot for one scope. Global scope passes `repo_root: None`; repo scope layers
/// the repo's `.claude` (project skills, settings.local.json toggle overrides) on top.
pub fn scan(claude_home: &Path, repo_root: Option<&Path>) -> ExtSnapshot {
    let global_enabled = enabled_map(&claude_home.join("settings.json"));
    let repo_enabled = repo_root
        .map(|r| enabled_map(&r.join(".claude/settings.local.json")))
        .unwrap_or_default();
    let installed = installed_plugins(claude_home);

    let mut ids: Vec<String> = installed.keys().cloned().collect();
    for id in global_enabled.keys().chain(repo_enabled.keys()) {
        if !ids.contains(id) { ids.push(id.clone()) }
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
            let provides = record.and_then(|(_, p)| p.as_deref()).map(|dir| PluginProvides {
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
        skills.extend(scan_skills(&repo.join(".claude/skills"), SkillSource::Project));
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));

    ExtSnapshot {
        cli_version: None, // filled by the RPC layer once the CLI runner exists (Task 9)
        marketplaces: scan_marketplaces(claude_home),
        plugins,
        skills,
    }
}
```

Add `pub mod ext;` to `crates/repomon-daemon/src/lib.rs` and `directories = { workspace = true }` to the daemon's `[dependencies]`.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p repomon-daemon ext:: 2>&1 | tail -3
```

Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt && git add -A crates/repomon-daemon && git commit -m "feat(daemon): extension config scanner"
```

---

### Task 3: Surgical settings writer

**Files:**
- Modify: `crates/repomon-daemon/src/ext.rs`

**Interfaces:**
- Produces: `ext::set_plugin_enabled(settings: &Path, id: &str, enabled: Option<bool>) -> std::io::Result<()>` (None removes the key). RPC arms and fan-out tests call this.

- [ ] **Step 1: Write the failing tests** (append to `ext.rs` tests)

```rust
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
    let after: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(after["model"], "opus");
    assert_eq!(after["permissions"]["allow"][0], "Bash");
    assert_eq!(after["enabledPlugins"]["a@m"], true);
    assert_eq!(after["enabledPlugins"]["b@m"], false);

    set_plugin_enabled(&settings, "a@m", None).unwrap();
    let after: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert!(after["enabledPlugins"].get("a@m").is_none());
}

#[test]
fn toggle_creates_missing_settings_file_and_parents() {
    let tmp = tempfile::tempdir().unwrap();
    let settings = tmp.path().join("deep/.claude/settings.local.json");
    set_plugin_enabled(&settings, "a@m", Some(true)).unwrap();
    let after: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
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
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p repomon-daemon ext:: 2>&1 | tail -3
```

Expected: compile error (`set_plugin_enabled` not defined).

- [ ] **Step 3: Implement**

```rust
use std::io;
use std::sync::Mutex;

/// Serializes all settings writes so concurrent RPCs cannot interleave read-modify-write cycles.
static SETTINGS_WRITE: Mutex<()> = Mutex::new(());

/// Read-modify-write ONLY the `enabledPlugins` key, preserving every other byte of meaning in the
/// file, then atomically replace it (temp file + rename). `enabled: None` removes the entry.
/// A corrupt file is an error, never a clobber.
pub fn set_plugin_enabled(settings: &Path, id: &str, enabled: Option<bool>) -> io::Result<()> {
    let _guard = SETTINGS_WRITE.lock().unwrap();
    let mut root: Value = match fs::read_to_string(settings) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("corrupt settings: {e}")))?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Value::Object(Default::default()),
        Err(e) => return Err(e),
    };
    let Value::Object(map) = &mut root else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "settings root is not an object"));
    };
    let plugins = map
        .entry("enabledPlugins")
        .or_insert_with(|| Value::Object(Default::default()));
    let Value::Object(plugins) = plugins else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "enabledPlugins is not an object"));
    };
    match enabled {
        Some(value) => { plugins.insert(id.to_string(), Value::Bool(value)); }
        None => { plugins.remove(id); }
    }
    if let Some(dir) = settings.parent() { fs::create_dir_all(dir)? }
    let tmp = settings.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(&root)?)?;
    fs::rename(&tmp, settings)
}
```

- [ ] **Step 4: Run tests, expect 6 passed, then commit**

```bash
cargo test -p repomon-daemon ext:: 2>&1 | tail -3
cargo fmt && git add crates/repomon-daemon/src/ext.rs && git commit -m "feat(daemon): surgical enabledPlugins writer"
```

---

### Task 4: Worktree fan-out

**Files:**
- Modify: `crates/repomon-daemon/src/ext.rs`

**Interfaces:**
- Produces: `ext::sync_worktree(repo_root: &Path, worktree: &Path) -> std::io::Result<()>`; `ext::fan_out(repo_root: &Path) -> FanoutSummary`. RPC arms call `fan_out` after repo-scope mutations; the lane.create arm calls `sync_worktree` for seeding.

- [ ] **Step 1: Write the failing tests** (append; note the `git` helper)

```rust
fn git(dir: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .arg("-C").arg(dir).args(args)
        .env("GIT_AUTHOR_NAME", "T").env("GIT_AUTHOR_EMAIL", "t@e.com")
        .env("GIT_COMMITTER_NAME", "T").env("GIT_COMMITTER_EMAIL", "t@e.com")
        .output().unwrap().status.success();
    assert!(ok, "git {args:?}");
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
    git(&repo, &["worktree", "add", wt1.to_str().unwrap(), "-b", "l1"]);
    git(&repo, &["worktree", "add", wt2.to_str().unwrap(), "-b", "l2"]);

    // Repo-root .claude is the source of truth (gitignored files included).
    let src_skills = repo.join(".claude/skills/verify");
    std::fs::create_dir_all(&src_skills).unwrap();
    std::fs::write(src_skills.join("SKILL.md"), "---\nname: verify\n---\n").unwrap();
    set_plugin_enabled(&repo.join(".claude/settings.local.json"), "a@m", Some(true)).unwrap();

    let summary = fan_out(&repo);
    assert_eq!(summary.synced_lanes.len(), 2, "skipped: {:?}", summary.skipped_lanes);
    for wt in [&wt1, &wt2] {
        assert!(wt.join(".claude/skills/verify/SKILL.md").is_file());
        let s: Value = serde_json::from_str(
            &std::fs::read_to_string(wt.join(".claude/settings.local.json")).unwrap(),
        ).unwrap();
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
    git(&repo, &["worktree", "add", wt1.to_str().unwrap(), "-b", "l1"]);
    set_plugin_enabled(&repo.join(".claude/settings.local.json"), "a@m", Some(true)).unwrap();
    // Simulate a vanished worktree dir (git still lists it).
    std::fs::remove_dir_all(&wt1).unwrap();

    let summary = fan_out(&repo);
    assert!(summary.synced_lanes.is_empty());
    assert_eq!(summary.skipped_lanes.len(), 1);
    assert_eq!(summary.skipped_lanes[0].lane, "wt1");
}
```

- [ ] **Step 2: Run to verify failure** (compile error: `fan_out` not defined)

- [ ] **Step 3: Implement**

```rust
use repomon_core::model::{FanoutSummary, SkippedLane};

/// Worktrees of `repo_root` (excluding the root itself), via `git worktree list --porcelain`.
fn repo_worktrees(repo_root: &Path) -> io::Result<Vec<PathBuf>> {
    let out = std::process::Command::new("git")
        .arg("-C").arg(repo_root)
        .args(["worktree", "list", "--porcelain"])
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other(String::from_utf8_lossy(&out.stderr).into_owned()));
    }
    let root = repo_root.canonicalize().unwrap_or_else(|_| repo_root.to_path_buf());
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
        if entry.file_type()?.is_dir() { copy_dir(&entry.path(), &to)? }
        else { fs::copy(entry.path(), &to)?; }
    }
    Ok(())
}

/// Mirror the repo root's `.claude/settings.local.json` and `.claude/skills/` into one worktree.
/// Copy-over semantics: deletions are handled by the mutation RPCs re-running the fan-out after
/// removing from the source, plus deleting the target path (see skill.delete).
pub fn sync_worktree(repo_root: &Path, worktree: &Path) -> io::Result<()> {
    if !worktree.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "worktree directory missing"));
    }
    let src = repo_root.join(".claude");
    let dst = worktree.join(".claude");
    let settings = src.join("settings.local.json");
    if settings.is_file() {
        fs::create_dir_all(&dst)?;
        fs::copy(&settings, dst.join("settings.local.json"))?;
    }
    let skills = src.join("skills");
    if skills.is_dir() { copy_dir(&skills, &dst.join("skills"))? }
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
            Err(e) => summary.skipped_lanes.push(SkippedLane { lane, reason: e.to_string() }),
        }
    }
    summary
}
```

- [ ] **Step 4: Run tests, expect 8 passed, then commit**

```bash
cargo test -p repomon-daemon ext:: 2>&1 | tail -3
cargo fmt && git add crates/repomon-daemon/src/ext.rs && git commit -m "feat(daemon): repo-to-worktree extension fan-out"
```

---

### Task 5: D1 RPC arms + lane.create seeding + integration test

**Files:**
- Modify: `crates/repomon-daemon/src/rpc.rs`
- Modify: `crates/repomon-daemon/tests/integration.rs`

**Interfaces:**
- Produces RPCs: `ext.list {scope, repo_id?} -> ExtSnapshot`; `plugin.enable` / `plugin.disable {id, scope, repo_id?} -> {ok, fanout: FanoutSummary|null}`; broadcast `event.ext.changed {scope, repo_id?}`.
- Consumes: `ext::{claude_home, scan, set_plugin_enabled, fan_out, sync_worktree}`, `ctx.store.get_repo(repo_id)`, `ctx.broadcast(method, params)`.

- [ ] **Step 1: Write the failing integration test** (follow the existing pattern in `integration.rs`: `Ctx::new` + `serve` + `connect_retry` + `call`)

```rust
#[tokio::test]
async fn extension_rpcs_list_toggle_and_fan_out() {
    // Point the daemon at an isolated Claude home (process-global env: keep this the only test
    // that sets it).
    let claude_home = tempfile::tempdir().unwrap();
    std::env::set_var("REPOMON_CLAUDE_HOME", claude_home.path());
    std::fs::create_dir_all(claude_home.path().join("skills/global-skill")).unwrap();
    std::fs::write(
        claude_home.path().join("skills/global-skill/SKILL.md"),
        "---\nname: global-skill\ndescription: g\n---\n",
    )
    .unwrap();

    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-ext-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    let mut stream = connect_retry(&sock).await;

    // Global list sees the skill.
    let r = call(&mut stream, 1, "ext.list", Some(json!({ "scope": "global" }))).await;
    let snap = r.result.unwrap();
    assert_eq!(snap["skills"][0]["name"], "global-skill");

    // Register a repo with one worktree, then toggle a plugin at repo scope.
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    std::fs::write(repo.join("README"), "x").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "init"]);
    let wt = tmp.path().join("wt");
    git(&repo, &["worktree", "add", wt.to_str().unwrap(), "-b", "l1"]);
    let r = call(&mut stream, 2, "repo.add", Some(json!({ "path": repo.to_str().unwrap() }))).await;
    let repo_id = r.result.unwrap()["id"].as_i64().unwrap();

    let r = call(
        &mut stream,
        3,
        "plugin.enable",
        Some(json!({ "id": "superpowers@official", "scope": "repo", "repo_id": repo_id })),
    )
    .await;
    let result = r.result.unwrap();
    assert_eq!(result["ok"], true);
    assert_eq!(result["fanout"]["synced_lanes"].as_array().unwrap().len(), 1);

    // The toggle landed in the repo root AND the worktree.
    for base in [&repo, &wt] {
        let s: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(base.join(".claude/settings.local.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(s["enabledPlugins"]["superpowers@official"], true);
    }

    // Repo-scope list reflects it with enabled_source repo.
    let r = call(&mut stream, 4, "ext.list", Some(json!({ "scope": "repo", "repo_id": repo_id }))).await;
    let snap = r.result.unwrap();
    let plugin = snap["plugins"]
        .as_array().unwrap().iter()
        .find(|p| p["id"] == "superpowers@official")
        .unwrap();
    assert_eq!(plugin["enabled"], true);
    assert_eq!(plugin["enabled_source"], "repo");

    server.abort();
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p repomon-daemon --test integration extension_rpcs 2>&1 | tail -5
```

Expected: FAIL with method not found: ext.list.

- [ ] **Step 3: Implement the arms in `rpc.rs`**

Add param structs beside the existing ones (around `AgentCapture` etc.):

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
enum ExtScope {
    Global,
    Repo { repo_id: RepoId },
}

#[derive(Deserialize)]
struct ExtList {
    #[serde(flatten)]
    scope: ExtScope,
}

#[derive(Deserialize)]
struct PluginToggle {
    id: String,
    #[serde(flatten)]
    scope: ExtScope,
}

fn ext_scope_json(scope: &ExtScope) -> Value {
    match scope {
        ExtScope::Global => json!({ "scope": "global" }),
        ExtScope::Repo { repo_id } => json!({ "scope": "repo", "repo_id": repo_id }),
    }
}
```

Add dispatch arms (near the other lane/repo arms):

```rust
"ext.list" => {
    let p: ExtList = parse(params)?;
    let home = crate::ext::claude_home().ok_or_else(|| internal("cannot resolve home directory"))?;
    let repo_root = match p.scope {
        ExtScope::Global => None,
        ExtScope::Repo { repo_id } => Some(ctx.store.get_repo(repo_id).await.map_err(internal)?.path),
    };
    let snap = tokio::task::spawn_blocking(move || crate::ext::scan(&home, repo_root.as_deref()))
        .await
        .map_err(internal)?;
    to_value(snap)
}
"plugin.enable" | "plugin.disable" => {
    let enabled = method == "plugin.enable";
    let p: PluginToggle = parse(params)?;
    let (settings, fanout_root) = match &p.scope {
        ExtScope::Global => {
            let home = crate::ext::claude_home().ok_or_else(|| internal("cannot resolve home directory"))?;
            (home.join("settings.json"), None)
        }
        ExtScope::Repo { repo_id } => {
            let repo = ctx.store.get_repo(*repo_id).await.map_err(internal)?;
            (repo.path.join(".claude/settings.local.json"), Some(repo.path))
        }
    };
    let id = p.id.clone();
    let fanout = tokio::task::spawn_blocking(move || {
        crate::ext::set_plugin_enabled(&settings, &id, Some(enabled))?;
        Ok::<_, std::io::Error>(fanout_root.map(|root| crate::ext::fan_out(&root)))
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;
    ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
    Ok(json!({ "ok": true, "fanout": fanout }))
}
```

- [ ] **Step 4: Seed new worktrees in the `lane.create` arm** (rpc.rs:618)

After the arm's successful create (it produces the created lane; find the `Ok(...)` path) add, using the lane's repo path and worktree path fields (`lane.repo.path`, `lane.worktree.path`; check the `Worktree` model struct for the exact path field name while editing):

```rust
// Seed the new worktree with the repo's extension config (best-effort; a failure only means
// the lane starts with whatever git checked out).
let repo_root = lane.repo.path.clone();
let wt_path = lane.worktree.path.clone();
tokio::task::spawn_blocking(move || {
    if let Err(e) = crate::ext::sync_worktree(&repo_root, &wt_path) {
        tracing::debug!("lane.create ext seed skipped: {e}");
    }
});
```

- [ ] **Step 5: Run the integration test, expect PASS, run the full daemon suite, commit**

```bash
cargo test -p repomon-daemon 2>&1 | grep "test result"
cargo fmt && git add crates/repomon-daemon && git commit -m "feat(daemon): ext.list and plugin toggle RPCs with worktree fan-out"
```

---

### Task 6: Frontend RpcMap + extensions store

**Files:**
- Modify: `apps/desktop/src/ipc/rpc.ts` (RpcMap interface + imports)
- Create: `apps/desktop/src/stores/extensions.ts`
- Create: `apps/desktop/src/stores/extensions.test.ts`

**Interfaces:**
- Produces: `createExtensionsStore(source?)` returning `{ scope, setScope, query, setQuery, filter, setFilter, snapshot, rows, busy, error, refresh, setEnabled }`. `ExtRow = { kind: "plugin"; plugin: PluginInfo } | { kind: "skill"; skill: SkillInfo }`. Components consume this.
- Consumes: Task 1 bindings, Task 5 RPCs.

- [ ] **Step 1: Add RpcMap entries** (inside `interface RpcMap` in `rpc.ts`; import `ExtSnapshot`, `FanoutSummary` from `../bindings`)

```ts
export type ExtScopeParams = { scope: "global" } | { scope: "repo"; repo_id: number };

  "ext.list": { params: ExtScopeParams; result: ExtSnapshot };
  "plugin.enable": { params: { id: string } & ExtScopeParams; result: { ok: boolean; fanout: FanoutSummary | null } };
  "plugin.disable": { params: { id: string } & ExtScopeParams; result: { ok: boolean; fanout: FanoutSummary | null } };
```

(`export type ExtScopeParams` goes at module level near `ConfigView`, not inside RpcMap.)

- [ ] **Step 2: Write the failing store test**

```ts
import { createRoot } from "solid-js";
import { describe, expect, it, vi } from "vitest";

import type { ExtSnapshot } from "../bindings";
import { createExtensionsStore, type ExtSource } from "./extensions";

const snapshot: ExtSnapshot = {
  cli_version: null,
  marketplaces: [{ name: "official", kind: "github", reference: "a/b", last_updated: null }],
  plugins: [
    { id: "superpowers@official", name: "superpowers", marketplace: "official", version: "6.1.1", enabled: true, enabled_source: "global", provides: null, installed: true },
    { id: "github@official", name: "github", marketplace: "official", version: null, enabled: false, enabled_source: "default", provides: null, installed: true },
  ],
  skills: [{ name: "verify", description: "checks things", source: "project", path: "/r/.claude/skills/verify" }],
};

function source(overrides: Partial<ExtSource> = {}): ExtSource {
  return {
    list: vi.fn().mockResolvedValue(snapshot),
    setEnabled: vi.fn().mockResolvedValue({ ok: true, fanout: null }),
    ...overrides,
  };
}

async function flush() {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe("extensions store", () => {
  it("loads a snapshot and exposes unified filtered rows", async () => {
    await createRoot(async (dispose) => {
      const store = createExtensionsStore(source());
      await flush();
      expect(store.rows().length).toBe(3); // 2 plugins + 1 skill, marketplaces excluded from rows
      store.setQuery("verify");
      expect(store.rows().length).toBe(1);
      expect(store.rows()[0].kind).toBe("skill");
      store.setQuery("");
      store.setFilter("plugins");
      expect(store.rows().every((r) => r.kind === "plugin")).toBe(true);
      dispose();
    });
  });

  it("toggling calls the daemon with the active scope and refreshes", async () => {
    await createRoot(async (dispose) => {
      const src = source();
      const store = createExtensionsStore(src);
      store.setScope({ scope: "repo", repo_id: 7 });
      await flush();
      await store.setEnabled("github@official", true);
      expect(src.setEnabled).toHaveBeenCalledWith("github@official", true, { scope: "repo", repo_id: 7 });
      expect(src.list).toHaveBeenCalledTimes(3); // initial + scope change + post-toggle refresh
      dispose();
    });
  });

  it("surfaces toggle failures without wedging busy", async () => {
    await createRoot(async (dispose) => {
      const src = source({ setEnabled: vi.fn().mockRejectedValue(new Error("nope")) });
      const store = createExtensionsStore(src);
      await flush();
      await store.setEnabled("github@official", true);
      expect(store.error()).toContain("nope");
      expect(store.busy()).toBe(false);
      dispose();
    });
  });
});
```

- [ ] **Step 3: Run to verify failure**

```bash
cd apps/desktop && bun run test extensions 2>&1 | tail -5
```

Expected: FAIL, cannot resolve `./extensions`.

- [ ] **Step 4: Implement the store** (mirror `fleet.ts`'s injectable-source pattern)

```ts
import { createMemo, createSignal } from "solid-js";

import type { ExtSnapshot, PluginInfo, SkillInfo } from "../bindings";
import { daemonCall, type ExtScopeParams } from "../ipc/rpc";

export type ExtFilter = "all" | "plugins" | "skills" | "marketplaces";

export type ExtRow =
  | { kind: "plugin"; plugin: PluginInfo }
  | { kind: "skill"; skill: SkillInfo };

export interface ExtSource {
  list(scope: ExtScopeParams): Promise<ExtSnapshot>;
  setEnabled(id: string, enabled: boolean, scope: ExtScopeParams): Promise<unknown>;
}

export const daemonExtSource: ExtSource = {
  list: (scope) => daemonCall("ext.list", scope),
  setEnabled: (id, enabled, scope) =>
    daemonCall(enabled ? "plugin.enable" : "plugin.disable", { id, ...scope }),
};

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function createExtensionsStore(source: ExtSource = daemonExtSource) {
  const [scope, setScopeSignal] = createSignal<ExtScopeParams>({ scope: "global" });
  const [query, setQuery] = createSignal("");
  const [filter, setFilter] = createSignal<ExtFilter>("all");
  const [snapshot, setSnapshot] = createSignal<ExtSnapshot | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  async function refresh() {
    setBusy(true);
    try {
      setSnapshot(await source.list(scope()));
      setError(null);
    } catch (cause) {
      setError(message(cause));
    } finally {
      setBusy(false);
    }
  }

  function setScope(next: ExtScopeParams) {
    setScopeSignal(next);
    void refresh();
  }

  async function setEnabled(id: string, enabled: boolean) {
    setBusy(true);
    try {
      await source.setEnabled(id, enabled, scope());
      setError(null);
      await refresh();
    } catch (cause) {
      setError(message(cause));
    } finally {
      setBusy(false);
    }
  }

  const rows = createMemo<ExtRow[]>(() => {
    const snap = snapshot();
    if (!snap) return [];
    const q = query().trim().toLowerCase();
    const active = filter();
    const rows: ExtRow[] = [];
    if (active === "all" || active === "plugins") {
      for (const plugin of snap.plugins) rows.push({ kind: "plugin", plugin });
    }
    if (active === "all" || active === "skills") {
      for (const skill of snap.skills) rows.push({ kind: "skill", skill });
    }
    if (!q) return rows;
    return rows.filter((row) => {
      const text = row.kind === "plugin"
        ? `${row.plugin.id} ${row.plugin.name}`
        : `${row.skill.name} ${row.skill.description ?? ""}`;
      return text.toLowerCase().includes(q);
    });
  });

  void refresh();

  return { scope, setScope, query, setQuery, filter, setFilter, snapshot, rows, busy, error, refresh, setEnabled };
}

export type ExtensionsStore = ReturnType<typeof createExtensionsStore>;
```

- [ ] **Step 5: Run tests + typecheck, expect PASS, commit**

```bash
cd apps/desktop && bun run test extensions && bun run check
git add apps/desktop/src/ipc/rpc.ts apps/desktop/src/stores/extensions.ts apps/desktop/src/stores/extensions.test.ts
git commit -m "feat(desktop): extensions store and RPC map entries"
```

---

### Task 7: Extensions view + App wiring

**Files:**
- Create: `apps/desktop/src/components/ExtensionsView.tsx`
- Create: `apps/desktop/src/components/ExtensionDrawer.tsx`
- Modify: `apps/desktop/src/App.tsx`

**Interfaces:**
- Produces: `<ExtensionsView store fleet />` rendered in the terminal bay; `<ExtensionDrawer row store onClose />`. App owns one `ExtensionsStore` instance (`ext`) and an `extensionsOpen` signal; both are consumed by Task 8.
- Consumes: Task 6 store, `fleet.repos()` for scope tabs.

- [ ] **Step 1: Implement `ExtensionDrawer.tsx`** (v1: toggles + facts; details/edit buttons arrive in Tasks 10/12)

```tsx
import { Show } from "solid-js";

import type { ExtensionsStore, ExtRow } from "../stores/extensions";

interface ExtensionDrawerProps {
  row: ExtRow;
  store: ExtensionsStore;
  onClose: () => void;
}

export default function ExtensionDrawer(props: ExtensionDrawerProps) {
  return (
    <aside class="flex w-72 shrink-0 flex-col gap-3 border-l border-line bg-surface p-4 text-sm">
      <div class="flex items-center justify-between">
        <span class="section-label">{props.row.kind === "plugin" ? "Plugin" : "Skill"}</span>
        <button type="button" class="focus-ring rounded px-1 font-mono text-muted hover:text-foreground" onClick={() => props.onClose()} aria-label="Close details">×</button>
      </div>
      <Show when={props.row.kind === "plugin" ? props.row : null}>
        {(row) => {
          const plugin = () => row().plugin;
          return (
            <>
              <h3 class="font-mono text-[0.8rem] text-foreground">{plugin().name}</h3>
              <p class="font-mono text-[0.64rem] text-muted">
                {plugin().marketplace}{plugin().version ? ` · v${plugin().version}` : ""}
                {plugin().installed ? "" : " · not installed"}
              </p>
              <Show when={plugin().provides}>
                {(provides) => (
                  <p class="font-mono text-[0.64rem] text-muted">
                    {provides().skills} skills · {provides().commands} commands · {provides().agents} agents
                  </p>
                )}
              </Show>
              <label class="flex items-center gap-2 font-mono text-[0.7rem]">
                <input
                  type="checkbox"
                  checked={plugin().enabled}
                  disabled={props.store.busy()}
                  onChange={(event) => void props.store.setEnabled(plugin().id, event.currentTarget.checked)}
                />
                Enabled in this scope
              </label>
              <p class="text-[0.62rem] text-muted">Changes apply to new agent sessions.</p>
            </>
          );
        }}
      </Show>
      <Show when={props.row.kind === "skill" ? props.row : null}>
        {(row) => {
          const skill = () => row().skill;
          return (
            <>
              <h3 class="font-mono text-[0.8rem] text-foreground">{skill().name}</h3>
              <p class="text-[0.68rem] text-muted">{skill().description ?? "No description"}</p>
              <p class="font-mono text-[0.6rem] text-muted">{skill().source}</p>
              <button
                type="button"
                class="focus-ring self-start rounded-md border border-line bg-raised px-2 py-1 font-mono text-[0.6rem] text-muted hover:text-foreground"
                onClick={() => void navigator.clipboard.writeText(String(skill().path))}
              >Copy path</button>
              <p class="text-[0.62rem] text-muted">Changes apply to new agent sessions.</p>
            </>
          );
        }}
      </Show>
    </aside>
  );
}
```

- [ ] **Step 2: Implement `ExtensionsView.tsx`**

```tsx
import { For, Show, createSignal } from "solid-js";

import type { FleetStore } from "../stores/fleet";
import type { ExtensionsStore, ExtFilter, ExtRow } from "../stores/extensions";
import ExtensionDrawer from "./ExtensionDrawer";

interface ExtensionsViewProps {
  store: ExtensionsStore;
  fleet: FleetStore;
}

const filters: ExtFilter[] = ["all", "plugins", "skills", "marketplaces"];

function rowKey(row: ExtRow): string {
  return row.kind === "plugin" ? `p:${row.plugin.id}` : `s:${row.skill.path}`;
}

export default function ExtensionsView(props: ExtensionsViewProps) {
  const [selectedKey, setSelectedKey] = createSignal<string | null>(null);
  const selected = () => props.store.rows().find((row) => rowKey(row) === selectedKey()) ?? null;
  const scopeIsRepo = (repoId: number) => {
    const scope = props.store.scope();
    return scope.scope === "repo" && scope.repo_id === repoId;
  };

  return (
    <div class="flex h-full min-h-0">
      <div class="flex min-w-0 flex-1 flex-col gap-3 p-4">
        <div class="flex flex-wrap items-center gap-2">
          <button
            type="button"
            class={`focus-ring rounded-md border px-2.5 py-1 font-mono text-[0.62rem] uppercase tracking-[0.1em] ${props.store.scope().scope === "global" ? "border-signal/40 bg-signal/10 text-signal" : "border-line bg-raised text-muted"}`}
            onClick={() => props.store.setScope({ scope: "global" })}
          >Global</button>
          <For each={props.fleet.repos()}>
            {(repo) => (
              <button
                type="button"
                class={`focus-ring rounded-md border px-2.5 py-1 font-mono text-[0.62rem] ${scopeIsRepo(repo.id) ? "border-signal/40 bg-signal/10 text-signal" : "border-line bg-raised text-muted"}`}
                onClick={() => props.store.setScope({ scope: "repo", repo_id: repo.id })}
              >{repo.name}</button>
            )}
          </For>
        </div>
        <div class="flex items-center gap-2">
          <input
            class="focus-ring min-w-0 flex-1 rounded-md border border-line bg-raised px-2.5 py-1.5 font-mono text-[0.7rem]"
            placeholder="Search extensions"
            value={props.store.query()}
            onInput={(event) => props.store.setQuery(event.currentTarget.value)}
          />
          <For each={filters}>
            {(filter) => (
              <button
                type="button"
                class={`focus-ring rounded-full border px-2.5 py-1 font-mono text-[0.58rem] uppercase ${props.store.filter() === filter ? "border-signal/40 bg-signal/10 text-signal" : "border-line bg-raised text-muted"}`}
                onClick={() => props.store.setFilter(filter)}
              >{filter}</button>
            )}
          </For>
        </div>
        <Show when={props.store.error()}>
          {(error) => <p class="rounded-md border border-fault/40 bg-fault/10 px-3 py-2 font-mono text-[0.66rem] text-fault">{error()}</p>}
        </Show>
        <Show
          when={props.store.filter() !== "marketplaces"}
          fallback={
            <ul class="min-h-0 flex-1 space-y-1 overflow-y-auto">
              <For each={props.store.snapshot()?.marketplaces ?? []}>
                {(marketplace) => (
                  <li class="flex items-center justify-between rounded-md border border-line bg-raised px-3 py-2 font-mono text-[0.7rem]">
                    <span>{marketplace.name}</span>
                    <span class="text-muted">{marketplace.kind} · {marketplace.reference}</span>
                  </li>
                )}
              </For>
            </ul>
          }
        >
          <ul class="min-h-0 flex-1 space-y-1 overflow-y-auto" aria-label="Extensions">
            <For each={props.store.rows()}>
              {(row) => (
                <li>
                  <button
                    type="button"
                    class={`focus-ring flex w-full items-center justify-between gap-2 rounded-md border px-3 py-2 text-left font-mono text-[0.72rem] ${selectedKey() === rowKey(row) ? "border-signal/40 bg-signal/10" : "border-line bg-raised hover:border-signal/30"}`}
                    onClick={() => setSelectedKey(rowKey(row))}
                  >
                    <span class="flex min-w-0 items-center gap-2 truncate">
                      <span class="truncate">{row.kind === "plugin" ? row.plugin.name : row.skill.name}</span>
                      <span class="rounded-full border border-line px-1.5 text-[0.55rem] uppercase text-muted">{row.kind}</span>
                      <span class="text-[0.58rem] text-muted">
                        {row.kind === "plugin" ? row.plugin.marketplace : row.skill.source}
                      </span>
                    </span>
                    <Show when={row.kind === "plugin" ? row : null}>
                      {(pluginRow) => (
                        <span class={`text-[0.6rem] ${pluginRow().plugin.enabled ? "text-signal" : "text-muted"}`}>
                          {pluginRow().plugin.enabled ? "on" : "off"}
                        </span>
                      )}
                    </Show>
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </div>
      <Show when={selected()}>
        {(row) => <ExtensionDrawer row={row()} store={props.store} onClose={() => setSelectedKey(null)} />}
      </Show>
    </div>
  );
}
```

Check `stores/fleet.ts` for the exported store type name; if it is not `FleetStore`, use the actual exported type (e.g. `ReturnType<typeof createFleetStore>`).

- [ ] **Step 3: Wire into `App.tsx`**

Add imports and state:

```tsx
import ExtensionsView from "./components/ExtensionsView";
import { createExtensionsStore } from "./stores/extensions";

const [extensionsOpen, setExtensionsOpen] = createSignal(false);
const ext = createExtensionsStore();
```

Header: add an Extensions button styled exactly like the existing Repomind toggle button (App.tsx around line 197), placed before it:

```tsx
<button
  type="button"
  class={`focus-ring rounded-md border px-2.5 py-1.5 font-mono text-[0.58rem] uppercase tracking-[0.1em] ${extensionsOpen() ? "border-signal/40 bg-signal/10 text-signal" : "border-line bg-raised text-muted"}`}
  onClick={() => setExtensionsOpen(!extensionsOpen())}
  aria-pressed={extensionsOpen()}
  title="Extensions (6)"
>Extensions</button>
```

Terminal bay swap (the `<main>` block):

```tsx
<main aria-label="Terminal bay" class="terminal-bay relative min-h-0 overflow-hidden bg-background">
  <Show when={extensionsOpen()} fallback={<TerminalWorkspace fleet={fleet} actions={actions} />}>
    <ExtensionsView store={ext} fleet={fleet} />
  </Show>
</main>
```

Keyboard shortcut, next to the existing `onSettingsShortcut` listener registration (App.tsx around line 93):

```tsx
const onExtensionsShortcut = (event: KeyboardEvent) => {
  if (event.key !== "6" || event.metaKey || event.ctrlKey || event.altKey) return;
  const target = event.target as HTMLElement | null;
  if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable)) return;
  setExtensionsOpen((open) => !open);
};
window.addEventListener("keydown", onExtensionsShortcut);
```

Add `window.removeEventListener("keydown", onExtensionsShortcut)` in the same cleanup place `onSettingsShortcut` is removed.

Event-driven refresh (spec: every client refreshes on `event.ext.changed`): App already pumps daemon events somewhere (find the existing `subscribeDaemon` usage; `stores/fleet.ts` wires it via its `subscribe` source). Add alongside the other subscriptions in App's onMount:

```tsx
import { subscribeDaemon } from "./ipc/rpc";

const unsubscribeExt = await subscribeDaemon((event) => {
  if (event.method === "event.ext.changed") void ext.refresh();
});
onCleanup(() => void unsubscribeExt());
```

If App does not subscribe directly (fleet does it internally), mirror fleet's pattern instead: give the extensions store an optional `subscribe` field on `ExtSource` defaulting to `subscribeDaemon`, call it inside `createExtensionsStore`, and refresh on `event.ext.changed`. Either placement is fine; pick the one matching the existing code.

- [ ] **Step 4: Verify**

```bash
cd apps/desktop && bun run check && bun run test 2>&1 | grep -E "Test Files|Tests "
```

Expected: typecheck passes, all tests pass. Then run `bun run tauri dev` briefly, toggle Extensions with the header button and the 6 key, switch scopes, click a row: drawer opens.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/components/ExtensionsView.tsx apps/desktop/src/components/ExtensionDrawer.tsx apps/desktop/src/App.tsx
git commit -m "feat(desktop): extensions view with scope tabs and detail drawer"
```

---

### Task 8: Repo-row quick toggles

**Files:**
- Create: `apps/desktop/src/components/RepoExtMenu.tsx`
- Modify: `apps/desktop/src/components/FleetSidebar.tsx`
- Modify: `apps/desktop/src/App.tsx`

**Interfaces:**
- Produces: `<RepoExtMenu repoId x y onOpenExtensions onClose />`; FleetSidebar prop `onOpenExtensions?: (repoId: number) => void`.
- Consumes: `daemonCall("ext.list"/"plugin.enable"/"plugin.disable")` directly (context menu is scoped and ephemeral; it does not share the view's store).

- [ ] **Step 1: Implement `RepoExtMenu.tsx`**

```tsx
import { For, Show, createResource } from "solid-js";

import type { PluginInfo } from "../bindings";
import { daemonCall } from "../ipc/rpc";

interface RepoExtMenuProps {
  repoId: number;
  x: number;
  y: number;
  onOpenExtensions: () => void;
  onClose: () => void;
}

export default function RepoExtMenu(props: RepoExtMenuProps) {
  const [snapshot, { refetch }] = createResource(() =>
    daemonCall("ext.list", { scope: "repo", repo_id: props.repoId }),
  );

  async function toggle(plugin: PluginInfo) {
    await daemonCall(plugin.enabled ? "plugin.disable" : "plugin.enable", {
      id: plugin.id,
      scope: "repo",
      repo_id: props.repoId,
    }).catch(() => undefined);
    void refetch();
  }

  return (
    <>
      <div class="fixed inset-0 z-40" onClick={() => props.onClose()} />
      <div
        class="fixed z-50 w-56 rounded-md border border-line bg-surface p-1 shadow-lg"
        style={{ left: `${props.x}px`, top: `${props.y}px` }}
        role="menu"
      >
        <button
          type="button"
          class="focus-ring block w-full rounded px-2 py-1.5 text-left font-mono text-[0.66rem] text-foreground hover:bg-raised"
          onClick={() => { props.onOpenExtensions(); props.onClose(); }}
          role="menuitem"
        >Extensions…</button>
        <div class="my-1 border-t border-line" />
        <Show when={snapshot()} fallback={<p class="px-2 py-1 font-mono text-[0.6rem] text-muted">Loading…</p>}>
          {(snap) => (
            <For each={snap().plugins.filter((plugin) => plugin.installed)}>
              {(plugin) => (
                <button
                  type="button"
                  class="focus-ring flex w-full items-center justify-between rounded px-2 py-1 text-left font-mono text-[0.62rem] text-muted hover:bg-raised hover:text-foreground"
                  onClick={() => void toggle(plugin)}
                  role="menuitemcheckbox"
                  aria-checked={plugin.enabled}
                >
                  <span class="truncate">{plugin.name}</span>
                  <span class={plugin.enabled ? "text-signal" : "text-muted"}>{plugin.enabled ? "on" : "off"}</span>
                </button>
              )}
            </For>
          )}
        </Show>
      </div>
    </>
  );
}
```

- [ ] **Step 2: Wire into `FleetSidebar.tsx`**

Add to the props interface: `onOpenExtensions?: (repoId: number) => void;`. Add menu state inside the component:

```tsx
const [extMenu, setExtMenu] = createSignal<{ repoId: number; x: number; y: number } | null>(null);
```

On the repo header row div (the `flex items-center justify-between px-2 py-1.5` div inside the repo `<section>`), add:

```tsx
onContextMenu={(event) => {
  event.preventDefault();
  setExtMenu({ repoId: repo.id, x: event.clientX, y: event.clientY });
}}
```

Render at the component root (after the scrollable list div):

```tsx
<Show when={extMenu()}>
  {(menu) => (
    <RepoExtMenu
      repoId={menu().repoId}
      x={menu().x}
      y={menu().y}
      onOpenExtensions={() => props.onOpenExtensions?.(menu().repoId)}
      onClose={() => setExtMenu(null)}
    />
  )}
</Show>
```

Import `RepoExtMenu` and add `createSignal`/`Show` to the solid-js import if missing.

- [ ] **Step 3: Pass the callback from `App.tsx`**

```tsx
<FleetSidebar
  fleet={fleet}
  actions={actions}
  searchRef={(element) => { searchInput = element; }}
  onOpenExtensions={(repoId) => {
    ext.setScope({ scope: "repo", repo_id: repoId });
    setExtensionsOpen(true);
  }}
/>
```

- [ ] **Step 4: Verify** (`bun run check && bun run test`, then in `tauri dev`: right-click a repo row, toggle a plugin, click "Extensions…" and confirm the view opens on that repo's scope)

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/components/RepoExtMenu.tsx apps/desktop/src/components/FleetSidebar.tsx apps/desktop/src/App.tsx
git commit -m "feat(desktop): repo-row extension quick toggles"
```

---

### Task 9: CLI runner + D3 RPCs (install/update/marketplaces/details)

**Files:**
- Modify: `crates/repomon-daemon/src/ext.rs`
- Modify: `crates/repomon-daemon/src/rpc.rs`
- Modify: `crates/repomon-daemon/tests/integration.rs`

**Interfaces:**
- Produces: `ext::ClaudeCli { pub fn detect() -> Option<ClaudeCli>; pub fn version(&self) -> &str; pub fn run(&self, args: &[&str]) -> Result<String, CliFailure> }`; `pub struct CliFailure { pub message: String, pub stderr: String, pub exit_code: Option<i32> }`. RPCs: `plugin.install {ref, scope, repo_id?}`, `plugin.remove {id, scope, repo_id?}`, `plugin.update {id?}`, `marketplace.add {source}`, `marketplace.remove {name}`, `marketplace.refresh {name?}`, `plugin.details {id}` -> `{text}`. `ext.list` now fills `cli_version`. Error code for CLI failures: `-32020` with `data: {stderr, exit_code}`.
- Consumes: Tasks 2-5.

- [ ] **Step 1: Write the failing unit test for the runner** (append to `ext.rs` tests; a PATH shim fakes `claude`)

```rust
fn fake_claude(dir: &Path, script: &str) -> ClaudeCli {
    let bin = dir.join("claude");
    std::fs::write(&bin, format!("#!/bin/sh\n{script}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    ClaudeCli { bin, version: "9.9.9-test".to_string() }
}

#[test]
fn cli_run_captures_stdout_on_success() {
    let tmp = tempfile::tempdir().unwrap();
    let cli = fake_claude(tmp.path(), "echo installed ok");
    assert_eq!(cli.run(&["plugin", "install", "x@m"]).unwrap().trim(), "installed ok");
}

#[test]
fn cli_run_surfaces_stderr_and_exit_code_on_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let cli = fake_claude(tmp.path(), "echo boom >&2; exit 3");
    let err = cli.run(&["plugin", "install", "x@m"]).unwrap_err();
    assert_eq!(err.exit_code, Some(3));
    assert!(err.stderr.contains("boom"));
}
```

- [ ] **Step 2: Run to verify failure** (compile error: `ClaudeCli` not defined)

- [ ] **Step 3: Implement the runner**

```rust
/// A CLI operation failure with everything the GUI needs to show a useful error.
#[derive(Debug)]
pub struct CliFailure {
    pub message: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// Handle to the `claude` CLI. Detection runs `claude --version` once per call site; the RPC
/// layer caches via OnceLock so a missing CLI is cheap to re-report.
pub struct ClaudeCli {
    pub bin: PathBuf,
    pub version: String,
}

impl ClaudeCli {
    pub fn detect() -> Option<ClaudeCli> {
        let out = std::process::Command::new("claude").arg("--version").output().ok()?;
        if !out.status.success() { return None }
        Some(ClaudeCli {
            bin: PathBuf::from("claude"),
            version: String::from_utf8_lossy(&out.stdout).trim().to_string(),
        })
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
```

Also fill `cli_version` in `scan` by adding a parameter: change the signature to `pub fn scan(claude_home: &Path, repo_root: Option<&Path>, cli_version: Option<String>) -> ExtSnapshot` and set `cli_version` in the returned struct. Update the existing tests to pass `None`.

- [ ] **Step 4: Add the RPC arms**

In `rpc.rs`, cache detection and add a helper:

```rust
static CLAUDE_CLI: std::sync::OnceLock<Option<std::sync::Arc<crate::ext::ClaudeCli>>> =
    std::sync::OnceLock::new();

fn claude_cli() -> Result<std::sync::Arc<crate::ext::ClaudeCli>, RpcError> {
    CLAUDE_CLI
        .get_or_init(|| crate::ext::ClaudeCli::detect().map(std::sync::Arc::new))
        .clone()
        .ok_or_else(|| RpcError::new(-32021, "claude CLI not found on PATH"))
}

fn cli_error(failure: crate::ext::CliFailure) -> RpcError {
    RpcError {
        code: -32020,
        message: failure.message,
        data: Some(json!({ "stderr": failure.stderr, "exit_code": failure.exit_code })),
    }
}

/// Run a CLI op off the async runtime, emit event.ext.changed, and return {ok, stdout}.
async fn run_cli_op(
    ctx: &Ctx,
    args: Vec<String>,
    changed_scope: Value,
) -> Result<Value, RpcError> {
    let cli = claude_cli()?;
    let stdout = tokio::task::spawn_blocking(move || {
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        cli.run(&arg_refs)
    })
    .await
    .map_err(internal)?
    .map_err(cli_error)?;
    ctx.broadcast("event.ext.changed", changed_scope);
    Ok(json!({ "ok": true, "stdout": stdout }))
}
```

Param structs and arms:

```rust
#[derive(Deserialize)]
struct PluginInstall {
    r#ref: String,
    #[serde(flatten)]
    scope: ExtScope,
}
#[derive(Deserialize)]
struct NameOnly { name: String }
#[derive(Deserialize)]
struct OptionalName { #[serde(default)] name: Option<String> }
#[derive(Deserialize)]
struct IdOnly { id: String }
#[derive(Deserialize)]
struct OptionalId { #[serde(default)] id: Option<String> }
```

```rust
"plugin.install" => {
    // Always -s user: the cache and install record stay global; per-repo activation is purely
    // an enabledPlugins toggle (worktree projectPath records would never match otherwise).
    let p: PluginInstall = parse(params)?;
    let result = run_cli_op(
        ctx,
        vec!["plugin".into(), "install".into(), p.r#ref.clone(), "-s".into(), "user".into()],
        ext_scope_json(&p.scope),
    )
    .await?;
    // Repo-scope install also enables it there so "install to this repo" does what it says.
    if let ExtScope::Repo { repo_id } = p.scope {
        let repo = ctx.store.get_repo(repo_id).await.map_err(internal)?;
        let settings = repo.path.join(".claude/settings.local.json");
        let id = p.r#ref.clone();
        let root = repo.path.clone();
        tokio::task::spawn_blocking(move || {
            crate::ext::set_plugin_enabled(&settings, &id, Some(true))?;
            crate::ext::fan_out(&root);
            Ok::<_, std::io::Error>(())
        })
        .await
        .map_err(internal)?
        .map_err(internal)?;
    }
    Ok(result)
}
"plugin.remove" => {
    let p: PluginToggle = parse(params)?;
    run_cli_op(
        ctx,
        vec!["plugin".into(), "uninstall".into(), p.id.clone()],
        ext_scope_json(&p.scope),
    )
    .await
}
"plugin.update" => {
    let p: OptionalId = parse(params)?;
    let mut args = vec!["plugin".into(), "update".into()];
    if let Some(id) = p.id { args.push(id) }
    run_cli_op(ctx, args, json!({ "scope": "global" })).await
}
"plugin.details" => {
    let p: IdOnly = parse(params)?;
    let cli = claude_cli()?;
    let text = tokio::task::spawn_blocking(move || cli.run(&["plugin", "details", &p.id]))
        .await
        .map_err(internal)?
        .map_err(cli_error)?;
    Ok(json!({ "text": text }))
}
"marketplace.add" => {
    let p: SourceOnly = parse(params)?;
    run_cli_op(
        ctx,
        vec!["plugin".into(), "marketplace".into(), "add".into(), p.source],
        json!({ "scope": "global" }),
    )
    .await
}
"marketplace.remove" => {
    let p: NameOnly = parse(params)?;
    run_cli_op(
        ctx,
        vec!["plugin".into(), "marketplace".into(), "remove".into(), p.name],
        json!({ "scope": "global" }),
    )
    .await
}
"marketplace.refresh" => {
    let p: OptionalName = parse(params)?;
    let mut args = vec!["plugin".into(), "marketplace".into(), "update".into()];
    if let Some(name) = p.name { args.push(name) }
    run_cli_op(ctx, args, json!({ "scope": "global" })).await
}
```

Add `#[derive(Deserialize)] struct SourceOnly { source: String }`. Before implementing `plugin.remove`, run `claude plugin --help` and confirm the uninstall subcommand's exact name (`uninstall` vs `remove`); use what the CLI prints. Update the `ext.list` arm to pass the cached CLI version: `crate::ext::scan(&home, repo_root.as_deref(), CLAUDE_CLI.get().and_then(|c| c.as_ref().map(|c| c.version.clone())))`, after calling `claude_cli().ok()` once to populate the cache (ignore the error: a missing CLI is fine for listing).

- [ ] **Step 5: Extend the integration test** (append to `extension_rpcs_list_toggle_and_fan_out` or a new test; a PATH-shim `claude` comes first on PATH)

```rust
#[tokio::test]
async fn plugin_details_returns_cli_text_or_structured_error() {
    // Missing CLI must produce -32021, not a crash. (PATH manipulation is process-global; if the
    // real claude is installed this asserts the success path instead.)
    let store = Store::open_in_memory().unwrap();
    let ctx = Ctx::new(store, Config::default(), None);
    let sock = std::env::temp_dir().join(format!("repomon-ext2-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let server = {
        let ctx = ctx.clone();
        let sock = sock.clone();
        tokio::spawn(async move { serve(ctx, &sock).await })
    };
    let mut stream = connect_retry(&sock).await;
    let r = call(&mut stream, 1, "plugin.details", Some(json!({ "id": "nonexistent-plugin-xyz@nowhere" }))).await;
    match (r.result, r.error) {
        (Some(v), None) => assert!(v["text"].is_string()),
        (None, Some(e)) => assert!(e.code == -32020 || e.code == -32021, "unexpected {e:?}"),
        other => panic!("unexpected response {other:?}"),
    }
    server.abort();
}
```

- [ ] **Step 6: Run all daemon tests, expect PASS, commit**

```bash
cargo test -p repomon-daemon 2>&1 | grep "test result"
cargo fmt && git add crates/repomon-daemon && git commit -m "feat(daemon): plugin install/update and marketplace RPCs via claude CLI"
```

---

### Task 10: D3 frontend (install, marketplaces, details, degradation)

**Files:**
- Modify: `apps/desktop/src/ipc/rpc.ts`
- Modify: `apps/desktop/src/stores/extensions.ts` + `extensions.test.ts`
- Modify: `apps/desktop/src/components/ExtensionsView.tsx`, `ExtensionDrawer.tsx`

**Interfaces:**
- Produces: store methods `install(ref)`, `remove(id)`, `update(id?)`, `details(id) -> Promise<string>`, `marketplaceAdd(source)`, `marketplaceRemove(name)`, `marketplaceRefresh(name?)`; `cliAvailable()` accessor.
- Consumes: Task 9 RPCs.

- [ ] **Step 1: RpcMap entries**

```ts
  "plugin.install": { params: { ref: string } & ExtScopeParams; result: { ok: boolean; stdout: string } };
  "plugin.remove": { params: { id: string } & ExtScopeParams; result: { ok: boolean; stdout: string } };
  "plugin.update": { params: { id?: string }; result: { ok: boolean; stdout: string } };
  "plugin.details": { params: { id: string }; result: { text: string } };
  "marketplace.add": { params: { source: string }; result: { ok: boolean; stdout: string } };
  "marketplace.remove": { params: { name: string }; result: { ok: boolean; stdout: string } };
  "marketplace.refresh": { params: { name?: string }; result: { ok: boolean; stdout: string } };
```

- [ ] **Step 2: Extend `ExtSource` + store** (same wrap-in-busy/error pattern as `setEnabled`; every mutator refreshes on success). Add to `ExtSource`:

```ts
  install(ref: string, scope: ExtScopeParams): Promise<unknown>;
  remove(id: string, scope: ExtScopeParams): Promise<unknown>;
  update(id: string | undefined): Promise<unknown>;
  details(id: string): Promise<string>;
  marketplaceAdd(source: string): Promise<unknown>;
  marketplaceRemove(name: string): Promise<unknown>;
  marketplaceRefresh(name: string | undefined): Promise<unknown>;
```

`daemonExtSource` implementations map one-to-one (`details` returns `(await daemonCall("plugin.details", { id })).text`). In the store add a generic private helper:

```ts
  async function mutate(op: () => Promise<unknown>) {
    setBusy(true);
    try {
      await op();
      setError(null);
      await refresh();
    } catch (cause) {
      setError(message(cause));
    } finally {
      setBusy(false);
    }
  }
```

Rewrite `setEnabled` through it and expose `install(ref)`, `remove(id)`, `update(id?)`, `marketplaceAdd(source)`, `marketplaceRemove(name)`, `marketplaceRefresh(name?)` as `mutate(...)` wrappers plus `details(id)` (no refresh; returns the text) and `cliAvailable = () => snapshot()?.cli_version != null`.

- [ ] **Step 3: Extend the store test** (add to the source factory mock resolutions and one test)

```ts
  it("install goes through the active scope and cli availability gates on cli_version", async () => {
    await createRoot(async (dispose) => {
      const src = source();
      const store = createExtensionsStore(src);
      await flush();
      expect(store.cliAvailable()).toBe(false); // fixture cli_version: null
      await store.install("x@official");
      expect(src.install).toHaveBeenCalledWith("x@official", { scope: "global" });
      dispose();
    });
  });
```

- [ ] **Step 4: UI additions**

- ExtensionsView header row (beside the filter chips): an `+ Install` button opening a small inline form (signal-controlled) with one input (`plugin@marketplace`) submitting `void props.store.install(ref)`; disabled with `title="Requires the claude CLI"` when `!props.store.cliAvailable()`.
- Marketplaces list items gain `Refresh` and `Remove` buttons calling the store, plus an `+ Add marketplace` input row (same disabled gating).
- ExtensionDrawer plugin section gains: `Details` button that calls `props.store.details(plugin().id)` into a local signal rendered in a `<pre class="max-h-48 overflow-auto whitespace-pre-wrap font-mono text-[0.58rem]">`, `Update` and `Remove` buttons (both `disabled={props.store.busy() || !props.store.cliAvailable()}`).
- Busy state: reuse the existing error banner slot; add a subtle `opacity-60 pointer-events-none` on the list container when `props.store.busy()`.

- [ ] **Step 5: Verify + commit**

```bash
cd apps/desktop && bun run check && bun run test 2>&1 | grep -E "Test Files|Tests "
git add apps/desktop/src && git commit -m "feat(desktop): plugin install, marketplaces, and details in extensions view"
```

---

### Task 11: D4 skill RPCs (create/read/write/delete)

**Files:**
- Modify: `crates/repomon-daemon/src/ext.rs`
- Modify: `crates/repomon-daemon/src/rpc.rs`
- Modify: `crates/repomon-daemon/tests/integration.rs`

**Interfaces:**
- Produces RPCs: `skill.create {scope, repo_id?, name, description?} -> {path}`; `skill.read {path} -> {content}`; `skill.write {path, content} -> {ok}`; `skill.delete {scope, repo_id?, name} -> {ok, fanout}`. Path safety rule: read/write/delete only accept paths inside `claude_home()/skills` or a registered repo's `.claude/skills`.
- Consumes: Tasks 2-5.

- [ ] **Step 1: Write failing unit tests for the helpers** (append to `ext.rs` tests)

```rust
#[test]
fn scaffold_skill_writes_frontmatter_and_rejects_bad_names() {
    let tmp = tempfile::tempdir().unwrap();
    let path = scaffold_skill(tmp.path(), "my-skill", Some("does x")).unwrap();
    let text = std::fs::read_to_string(path.join("SKILL.md")).unwrap();
    assert!(text.starts_with("---\nname: my-skill\ndescription: does x\n---\n"));
    assert!(scaffold_skill(tmp.path(), "my-skill", None).is_err(), "duplicate must fail");
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
    let roots = [home.path().join("skills"), repo.path().join(".claude/skills")];
    assert!(skill_path_allowed(&ok, &roots));
    assert!(skill_path_allowed(&ok2, &roots));
    assert!(!skill_path_allowed(&bad, &roots));
    assert!(!skill_path_allowed(Path::new("/etc/passwd"), &roots));
}
```

- [ ] **Step 2: Run to verify failure** (compile error)

- [ ] **Step 3: Implement the helpers**

```rust
/// Kebab-case-ish skill names only: no separators means no traversal and no surprise dirs.
fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Create `skills_dir/<name>/SKILL.md` with minimal frontmatter. Errors on invalid names and
/// existing skills (never overwrites).
pub fn scaffold_skill(skills_dir: &Path, name: &str, description: Option<&str>) -> io::Result<PathBuf> {
    if !valid_skill_name(name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid skill name"));
    }
    let dir = skills_dir.join(name);
    if dir.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "skill already exists"));
    }
    fs::create_dir_all(&dir)?;
    let description = description.unwrap_or("TODO: when to use this skill");
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n"),
    )?;
    Ok(dir)
}

/// True when `path` resolves inside one of the managed skills roots. Guards skill.read/write/
/// delete against arbitrary filesystem access through a crafted path.
pub fn skill_path_allowed(path: &Path, roots: &[PathBuf]) -> bool {
    // The file may not exist yet (write): canonicalize the nearest existing ancestor.
    let mut probe = path.to_path_buf();
    let mut rest = Vec::new();
    while !probe.exists() {
        let Some(name) = probe.file_name().map(|n| n.to_os_string()) else { return false };
        rest.push(name);
        if !probe.pop() { return false }
    }
    let Ok(canon) = probe.canonicalize() else { return false };
    let mut resolved = canon;
    for part in rest.iter().rev() {
        resolved.push(part);
    }
    roots.iter().any(|root| {
        let root = root.canonicalize().unwrap_or_else(|_| root.clone());
        resolved.starts_with(&root)
    })
}
```

- [ ] **Step 4: Add the RPC arms**

```rust
#[derive(Deserialize)]
struct SkillCreate {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(flatten)]
    scope: ExtScope,
}
#[derive(Deserialize)]
struct SkillPath { path: PathBuf }
#[derive(Deserialize)]
struct SkillWrite { path: PathBuf, content: String }
#[derive(Deserialize)]
struct SkillDelete {
    name: String,
    #[serde(flatten)]
    scope: ExtScope,
}
```

A shared helper in rpc.rs to collect the allowed roots:

```rust
async fn skill_roots(ctx: &Ctx) -> Result<Vec<PathBuf>, RpcError> {
    let mut roots = Vec::new();
    if let Some(home) = crate::ext::claude_home() {
        roots.push(home.join("skills"));
    }
    for repo in ctx.store.repo_list().await.map_err(internal)? {
        roots.push(repo.path.join(".claude/skills"));
    }
    Ok(roots)
}
```

(Confirm the store's list method name by looking at how the `repo.list` arm fetches repos; use that method.)

```rust
"skill.create" => {
    let p: SkillCreate = parse(params)?;
    let (skills_dir, fanout_root) = match &p.scope {
        ExtScope::Global => {
            let home = crate::ext::claude_home().ok_or_else(|| internal("cannot resolve home directory"))?;
            (home.join("skills"), None)
        }
        ExtScope::Repo { repo_id } => {
            let repo = ctx.store.get_repo(*repo_id).await.map_err(internal)?;
            (repo.path.join(".claude/skills"), Some(repo.path))
        }
    };
    let (name, description) = (p.name.clone(), p.description.clone());
    let path = tokio::task::spawn_blocking(move || {
        let path = crate::ext::scaffold_skill(&skills_dir, &name, description.as_deref())?;
        if let Some(root) = fanout_root { crate::ext::fan_out(&root); }
        Ok::<_, std::io::Error>(path)
    })
    .await
    .map_err(internal)?
    .map_err(|e| RpcError::invalid_params(e.to_string()))?;
    ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
    Ok(json!({ "path": path }))
}
"skill.read" => {
    let p: SkillPath = parse(params)?;
    let roots = skill_roots(ctx).await?;
    if !crate::ext::skill_path_allowed(&p.path, &roots) {
        return Err(RpcError::invalid_params("path is outside managed skill directories"));
    }
    let md = if p.path.ends_with("SKILL.md") { p.path.clone() } else { p.path.join("SKILL.md") };
    let content = tokio::fs::read_to_string(&md).await.map_err(internal)?;
    Ok(json!({ "content": content }))
}
"skill.write" => {
    let p: SkillWrite = parse(params)?;
    let roots = skill_roots(ctx).await?;
    if !crate::ext::skill_path_allowed(&p.path, &roots) {
        return Err(RpcError::invalid_params("path is outside managed skill directories"));
    }
    let md = if p.path.ends_with("SKILL.md") { p.path.clone() } else { p.path.join("SKILL.md") };
    tokio::fs::write(&md, p.content).await.map_err(internal)?;
    ctx.broadcast("event.ext.changed", json!({ "scope": "global" }));
    Ok(json!({ "ok": true }))
}
"skill.delete" => {
    let p: SkillDelete = parse(params)?;
    if !p.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(RpcError::invalid_params("invalid skill name"));
    }
    let (skills_dir, fanout_root) = match &p.scope {
        ExtScope::Global => {
            let home = crate::ext::claude_home().ok_or_else(|| internal("cannot resolve home directory"))?;
            (home.join("skills"), None)
        }
        ExtScope::Repo { repo_id } => {
            let repo = ctx.store.get_repo(*repo_id).await.map_err(internal)?;
            (repo.path.join(".claude/skills"), Some(repo.path))
        }
    };
    let name = p.name.clone();
    let fanout = tokio::task::spawn_blocking(move || {
        std::fs::remove_dir_all(skills_dir.join(&name))?;
        // sync_worktree prunes skills whose source dir is gone (see this task's ext.rs change),
        // so one fan-out both deletes the skill everywhere and re-syncs the survivors.
        Ok::<_, std::io::Error>(fanout_root.map(|root| crate::ext::fan_out(&root)))
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;
    ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
    Ok(json!({ "ok": true, "fanout": fanout }))
}
```

This arm depends on an `ext.rs` change in this same task: copy-over sync must also delete stale worktree skill dirs, or deletes never propagate. In `sync_worktree`, immediately before the `copy_dir` call for skills, add:

```rust
// Drop worktree skills whose source is gone, so deletes propagate. Only prune when the
// source skills dir exists (an absent source means this repo is unmanaged; touch nothing).
if skills.is_dir() {
    if let Ok(entries) = fs::read_dir(dst.join("skills")) {
        for entry in entries.flatten() {
            if !skills.join(entry.file_name()).exists() {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
}
```

And add this unit test to `ext.rs` (reuses the `git` helper from Task 4):

```rust
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
    git(&repo, &["worktree", "add", wt.to_str().unwrap(), "-b", "l1"]);

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
```

- [ ] **Step 5: Integration coverage** (append to the existing ext integration test: create a repo-scope skill, assert `{path}` exists and the worktree received it, `skill.write` then `skill.read` round-trips, `skill.read` of `/etc/passwd` errors, `skill.delete` removes it from repo and worktree)

```rust
    let r = call(&mut stream, 5, "skill.create",
        Some(json!({ "name": "e2e-skill", "description": "d", "scope": "repo", "repo_id": repo_id }))).await;
    let skill_path = r.result.unwrap()["path"].as_str().unwrap().to_string();
    assert!(Path::new(&skill_path).join("SKILL.md").is_file());
    assert!(wt.join(".claude/skills/e2e-skill/SKILL.md").is_file());

    let r = call(&mut stream, 6, "skill.write",
        Some(json!({ "path": skill_path, "content": "---\nname: e2e-skill\n---\nedited" }))).await;
    assert_eq!(r.result.unwrap()["ok"], true);
    let r = call(&mut stream, 7, "skill.read", Some(json!({ "path": skill_path }))).await;
    assert!(r.result.unwrap()["content"].as_str().unwrap().contains("edited"));

    let r = call(&mut stream, 8, "skill.read", Some(json!({ "path": "/etc/passwd" }))).await;
    assert!(r.error.is_some());

    let r = call(&mut stream, 9, "skill.delete",
        Some(json!({ "name": "e2e-skill", "scope": "repo", "repo_id": repo_id }))).await;
    assert_eq!(r.result.unwrap()["ok"], true);
    assert!(!wt.join(".claude/skills/e2e-skill").exists());
```

- [ ] **Step 6: Run all daemon tests, expect PASS, commit**

```bash
cargo test -p repomon-daemon 2>&1 | grep "test result"
cargo fmt && git add crates/repomon-daemon && git commit -m "feat(daemon): skill authoring RPCs with path guards"
```

---

### Task 12: D4 frontend (skill editor + new-skill flow)

**Files:**
- Create: `apps/desktop/src/components/SkillEditorModal.tsx`
- Modify: `apps/desktop/src/ipc/rpc.ts`, `apps/desktop/src/stores/extensions.ts`, `ExtensionsView.tsx`, `ExtensionDrawer.tsx`

**Interfaces:**
- Consumes: Task 11 RPCs; existing `Modal.tsx` (default export, see `RenameModal.tsx` for usage pattern).
- Produces: `<SkillEditorModal path onClose />`; store methods `createSkill(name, description?)`, `deleteSkill(name)`.

- [ ] **Step 1: RpcMap entries**

```ts
  "skill.create": { params: { name: string; description?: string } & ExtScopeParams; result: { path: string } };
  "skill.read": { params: { path: string }; result: { content: string } };
  "skill.write": { params: { path: string; content: string }; result: { ok: boolean } };
  "skill.delete": { params: { name: string } & ExtScopeParams; result: { ok: boolean; fanout: FanoutSummary | null } };
```

- [ ] **Step 2: Store additions** (through the Task 10 `mutate` helper; extend `ExtSource` with `createSkill`, `deleteSkill` and the vitest source factory with resolved mocks)

```ts
  createSkill: (name: string, description?: string) => mutate(() => source.createSkill(name, description, scope())),
  deleteSkill: (name: string) => mutate(() => source.deleteSkill(name, scope())),
```

- [ ] **Step 3: Implement `SkillEditorModal.tsx`** (look at `RenameModal.tsx` first and mirror its Modal usage, button styles, and submit handling)

```tsx
import { Show, createResource, createSignal } from "solid-js";

import { daemonCall } from "../ipc/rpc";
import Modal from "./Modal";

interface SkillEditorModalProps {
  path: string;
  onClose: () => void;
}

export default function SkillEditorModal(props: SkillEditorModalProps) {
  const [content] = createResource(() => daemonCall("skill.read", { path: props.path }));
  const [draft, setDraft] = createSignal<string | null>(null);
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const text = () => draft() ?? content()?.content ?? "";

  async function save() {
    setSaving(true);
    try {
      await daemonCall("skill.write", { path: props.path, content: text() });
      setError(null);
      props.onClose();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal title="Edit skill" onClose={props.onClose}>
      <div class="flex flex-col gap-3">
        <textarea
          class="focus-ring h-72 w-full resize-y rounded-md border border-line bg-raised p-3 font-mono text-[0.7rem] leading-relaxed"
          value={text()}
          onInput={(event) => setDraft(event.currentTarget.value)}
          spellcheck={false}
        />
        <Show when={error()}>
          {(message) => <p class="font-mono text-[0.64rem] text-fault">{message()}</p>}
        </Show>
        <div class="flex items-center justify-between">
          <p class="text-[0.62rem] text-muted">Saved changes apply to new agent sessions.</p>
          <button
            type="button"
            class="focus-ring rounded-md border border-signal/40 bg-signal/10 px-3 py-1.5 font-mono text-[0.64rem] text-signal disabled:opacity-50"
            disabled={saving() || content.loading}
            onClick={() => void save()}
          >Save</button>
        </div>
      </div>
    </Modal>
  );
}
```

Adapt the `Modal` props to the actual `ModalProps` interface in `Modal.tsx` (check whether it takes `title`/`onClose` or children-only; mirror `RenameModal.tsx`).

- [ ] **Step 4: Wire the flows**

- ExtensionDrawer skill section: `Edit` button setting an `editorPath` signal owned by ExtensionsView (pass a `onEdit(path: string)` prop down); `Delete` button calling `props.store.deleteSkill(skill().name)` behind a `window.confirm`-free two-click confirm (first click arms the button, second confirms; matches ConfirmDialog patterns if one exists: check `ConfirmDialog.tsx` and prefer it).
- ExtensionsView: `+ New skill` button beside `+ Install`, opening an inline form (name + description inputs) that calls `void props.store.createSkill(name, description)`; render `<Show when={editorPath()}><SkillEditorModal path={editorPath()!} onClose={() => setEditorPath(null)} /></Show>`.

- [ ] **Step 5: Verify + commit**

```bash
cd apps/desktop && bun run check && bun run test 2>&1 | grep -E "Test Files|Tests "
git add apps/desktop/src && git commit -m "feat(desktop): skill authoring with in-app editor"
```

---

### Task 13: Full verification and wrap-up

**Files:** none new.

- [ ] **Step 1: Full Rust suite + lints**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets 2>&1 | grep -c "^error" ; cargo test --workspace 2>&1 | grep "test result" | tail -8
```

Expected: fmt clean, 0 clippy errors, all suites pass.

- [ ] **Step 2: Bindings staleness gate** (regenerate and confirm no diff, same as CI)

```bash
TS_RS_EXPORT_DIR=../../apps/desktop/src/bindings cargo test -p repomon-core --features ts export_bindings --locked && git diff --exit-code apps/desktop/src/bindings
```

- [ ] **Step 3: Frontend suite**

```bash
cd apps/desktop && bun run check && bun run test 2>&1 | grep -E "Test Files|Tests "
```

- [ ] **Step 4: Live smoke** (requires the user's real daemon: rebuild + reinstall + restart, per the project rule that a rebuilt daemon must be reinstalled to `~/.cargo/bin` and restarted or the user tests stale code)

```bash
cargo install --path crates/repomon-daemon && repomon daemon restart
cd apps/desktop && bun run tauri dev
```

Manual pass: Extensions button + key 6; global list shows real skills/plugins; toggle a plugin globally and per-repo; right-click repo row quick toggle; create + edit + delete a repo skill and confirm it appears in a lane worktree; install flow (or its disabled state with hint when the CLI is missing).

- [ ] **Step 5: Update memory/docs and hand off** (note in the PR/commit series that `event.ext.changed` exists for the TUI to adopt later)

---

## Self-review notes (already applied)

- Spec coverage: D1 = Tasks 2-5, D2 = Tasks 6-8, D3 = Tasks 9-10, D4 = Tasks 11-12; cross-cutting rules land in Tasks 3 (surgical writes), 5 (events), 4/11 (fan-out + pruning); degradation in Tasks 9-10; apply-semantics hints in Tasks 7 and 12.
- Known deviations from spec, both agreed during planning: no `skill.reveal` (copy-path instead); `plugin.details` returns raw CLI text rather than parsed fields (the drawer shows it verbatim).
- `enabled_source: "repo"` serialization relies on the `EnabledSource` serde rename to snake_case; the integration test asserts the wire form.
- Env-var use (`REPOMON_CLAUDE_HOME`) is process-global: keep it to the single integration test noted there.
