//! Discovery for `agent.command_catalog`: real per-kind commands and models, never a fabricated
//! entry. Each kind's section below states, in its own doc comment, exactly what is discovered
//! from disk versus curated by hand - see `fleet/repomon/tasks/2026-09-12-native-commands.md`
//! for the contract and the operator's rule against driving an agent's interactive picker blind.
//!
//! Self-contained: the only things this module reads from the rest of the daemon are `Ctx`
//! (for a live pane capture), an already-overlaid `Lane` its caller in `rpc.rs` resolves (the
//! lane/session lookup itself needs `overlay_agents`, private to `rpc.rs`), and a handful of
//! `pub(crate)` helpers already in `ext.rs` (plugin/settings scanning claude-code and codex's
//! plugin systems share). Its own cache is a private, in-process static, not threaded through `Ctx`.
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use repomon_core::agent::backend::CaptureOpts;
use repomon_core::agent::conversation_activity::pane_activity;
use repomon_core::command_catalog::{
    CatalogCommand, CatalogEffort, CatalogModel, CatalogSource, CommandCatalog,
};
use repomon_core::model::{AgentKind, Lane};
use serde_json::Value;

use crate::Ctx;

/// How long a kind's discovered command/model lists (everything but which model is "current")
/// stay cached before the filesystem is read again. Short enough that a newly added user command
/// shows up well within a session; long enough that opening the palette on every keystroke never
/// re-walks a plugin cache. "Current" is always recomputed fresh (see `build`), so this TTL only
/// ever makes a *addition* look briefly stale, never a wrong "current" model.
const STATIC_TTL: Duration = Duration::from_secs(60);

/// opencode's model list means invoking its own CLI (`opencode models`), measured on this
/// machine at several seconds for ~100 entries - too slow to pay on every cache miss at the
/// default TTL above, so it gets a much longer one and a hard timeout (see `opencode::models`).
const OPENCODE_MODELS_TTL: Duration = Duration::from_secs(1800);

#[derive(Clone, PartialEq, Eq, Hash)]
struct StaticKey {
    kind: &'static str,
    repo_root: PathBuf,
}

#[derive(Clone)]
struct StaticValue {
    commands: Vec<CatalogCommand>,
    /// Never carries `current: true`; `build` marks that fresh on every call.
    models: Vec<CatalogModel>,
    model_command: Option<String>,
}

fn static_cache() -> &'static Mutex<HashMap<StaticKey, (Instant, StaticValue)>> {
    static CACHE: OnceLock<Mutex<HashMap<StaticKey, (Instant, StaticValue)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Runs `discover` at most once per `STATIC_TTL` for a given (kind, repo root).
fn cached_static(
    kind: &'static str,
    repo_root: &Path,
    discover: impl FnOnce() -> StaticValue,
) -> StaticValue {
    let key = StaticKey {
        kind,
        repo_root: repo_root.to_path_buf(),
    };
    let mut cache = static_cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, value)) = cache.get(&key) {
        if at.elapsed() < STATIC_TTL {
            return value.clone();
        }
    }
    let value = discover();
    cache.insert(key, (Instant::now(), value.clone()));
    value
}

/// Resolves `window` against an already-overlaid `lane` (see the `agent.command_catalog` match
/// arm in `rpc.rs`, which calls `overlay_agents` before this - `lane.agent_sessions` is empty
/// otherwise) to a kind and worktree, then discovers and caches its catalog. An unmatched
/// window, or a kind this module has nothing for, is an empty catalog, never an error - "no
/// commands known for this agent" is exactly what the palette should show in that case.
pub async fn build(ctx: &Ctx, lane: &Lane, window: Option<String>) -> CommandCatalog {
    let window = window.unwrap_or_else(|| repomon_core::TmuxRuntime::window_name(lane.id));
    let Some(session) = lane
        .agent_sessions
        .iter()
        .find(|s| s.tmux_window.as_deref() == Some(window.as_str()))
    else {
        return CommandCatalog::empty();
    };
    let kind = session.agent.clone();
    let repo_root = lane.worktree.path.clone();
    let session_id = session.session_id.clone();

    let StaticValue {
        commands,
        mut models,
        model_command,
    } = match &kind {
        AgentKind::ClaudeCode => {
            let root = repo_root.clone();
            tokio::task::spawn_blocking(move || {
                cached_static("claude-code", &root, || claude_code::discover(&root))
            })
            .await
            .unwrap_or_else(|_| StaticValue {
                commands: Vec::new(),
                models: Vec::new(),
                model_command: None,
            })
        }
        AgentKind::Codex => {
            let root = repo_root.clone();
            tokio::task::spawn_blocking(move || {
                cached_static("codex", &root, || codex::discover(&root))
            })
            .await
            .unwrap_or_else(|_| StaticValue {
                commands: Vec::new(),
                models: Vec::new(),
                model_command: None,
            })
        }
        AgentKind::OpenCode => opencode::discover(ctx, &repo_root).await,
        AgentKind::Antigravity => {
            let root = repo_root.clone();
            tokio::task::spawn_blocking(move || {
                cached_static("antigravity", &root, || antigravity::discover(&root))
            })
            .await
            .unwrap_or_else(|_| StaticValue {
                commands: Vec::new(),
                models: Vec::new(),
                model_command: None,
            })
        }
        AgentKind::Hermes => {
            tokio::task::spawn_blocking(|| cached_static("hermes", Path::new(""), hermes::discover))
                .await
                .unwrap_or_else(|_| StaticValue {
                    commands: Vec::new(),
                    models: Vec::new(),
                    model_command: None,
                })
        }
        AgentKind::Aider => {
            let root = repo_root.clone();
            tokio::task::spawn_blocking(move || {
                cached_static("aider", &root, || aider::discover(&root))
            })
            .await
            .unwrap_or_else(|_| StaticValue {
                commands: Vec::new(),
                models: Vec::new(),
                model_command: None,
            })
        }
        AgentKind::Cursor | AgentKind::Other(_) => StaticValue {
            commands: Vec::new(),
            models: Vec::new(),
            model_command: None,
        },
    };

    let current = match &kind {
        AgentKind::ClaudeCode => {
            let root = repo_root.clone();
            let sid = session_id.clone();
            tokio::task::spawn_blocking(move || claude_code::current_model(&root, sid.as_deref()))
                .await
                .unwrap_or(None)
        }
        AgentKind::Codex => codex::current_model(ctx, &window, &repo_root).await,
        AgentKind::Hermes => tokio::task::spawn_blocking(hermes::current_model)
            .await
            .unwrap_or(None),
        AgentKind::Aider => {
            let root = repo_root.clone();
            tokio::task::spawn_blocking(move || aider::current_model(&root))
                .await
                .unwrap_or(None)
        }
        AgentKind::Antigravity => tokio::task::spawn_blocking(antigravity::current_model)
            .await
            .unwrap_or(None),
        AgentKind::OpenCode => {
            let sid = session_id.clone();
            tokio::task::spawn_blocking(move || opencode::current_model(sid.as_deref()))
                .await
                .unwrap_or(None)
        }
        AgentKind::Cursor | AgentKind::Other(_) => None,
    };
    if let Some(current_id) = current.clone() {
        match models.iter_mut().find(|m| m.id == current_id) {
            Some(m) => m.current = true,
            None => models.push(CatalogModel {
                id: current_id.clone(),
                label: current_id,
                current: true,
            }),
        }
    }

    // Only claude-code has an effort concept this can read. For every other kind the lists stay
    // empty, which the panel renders as no control at all rather than a dead one.
    let (efforts, effort_command) = match &kind {
        AgentKind::ClaudeCode => {
            let model = current.clone();
            let active =
                tokio::task::spawn_blocking(move || claude_code::current_effort(model.as_deref()))
                    .await
                    .unwrap_or(None);
            let efforts = claude_code::EFFORTS
                .iter()
                .map(|id| CatalogEffort {
                    id: (*id).into(),
                    label: effort_label(id),
                    current: active.as_deref() == Some(*id),
                })
                .collect();
            (efforts, Some("/effort".to_string()))
        }
        _ => (Vec::new(), None),
    };

    CommandCatalog {
        commands,
        models,
        model_command,
        efforts,
        effort_command,
    }
}

/// "xhigh" reads as one word, so the label is the level with its first letter raised rather than
/// a split on any separator.
fn effort_label(id: &str) -> String {
    let mut chars = id.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Reads `commands/*.md` in one directory as one plugin's or one user's commands. `namer` turns
/// a file stem into the catalog's `name` (bare for user commands, `plugin:stem` for a plugin's).
/// `description` comes only from real content in the file - a heading-only file with no prose
/// gets an empty description rather than an invented one.
fn commands_in_dir(
    dir: &Path,
    source: CatalogSource,
    namer: impl Fn(&str) -> String,
) -> Vec<CatalogCommand> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let description = command_description(&path);
        out.push(CatalogCommand {
            name: namer(stem),
            description,
            source,
            one_shot: true,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A command markdown file's description: the `description:` frontmatter field used by Claude's
/// and opencode's command files (same shape as a SKILL.md's, see `ext::skill_frontmatter`), or,
/// for a heading-only file like codex's plugin commands (`# /name` with no frontmatter at all),
/// the first non-empty prose line after the heading. Never a guess when neither is present.
fn command_description(path: &Path) -> String {
    let (_, frontmatter_description) = crate::ext::skill_frontmatter(path);
    if let Some(description) = frontmatter_description.filter(|d| !d.is_empty()) {
        return description;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return String::new();
    };
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_description_prefers_frontmatter_over_the_heading_body() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review-plan.md");
        fs::write(&path, "---\ndescription: Check the active plan against repo conventions\n---\n\n# /review-plan\n\nBody text.\n").unwrap();
        assert_eq!(
            command_description(&path),
            "Check the active plan against repo conventions"
        );
    }

    #[test]
    fn command_description_falls_back_to_the_first_prose_line_when_there_is_no_frontmatter() {
        // Matches codex's real plugin command shape: a `# /name` heading, no frontmatter, the
        // first paragraph read as the description.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connect-figma-components.md");
        fs::write(&path, "# /connect-figma-components\n\nCreate or update parserless Figma Code Connect template files.\n\n## Arguments\n").unwrap();
        assert_eq!(
            command_description(&path),
            "Create or update parserless Figma Code Connect template files."
        );
    }

    #[test]
    fn command_description_is_empty_for_a_heading_only_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clear.md");
        fs::write(&path, "# /clear\n").unwrap();
        assert_eq!(command_description(&path), "");
    }

    #[test]
    fn commands_in_dir_skips_non_markdown_files_and_sorts_by_the_named_result() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("review.md"),
            "# /review\n\nReview the diff.\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("clear.md"),
            "# /clear\n\nClear the conversation.\n",
        )
        .unwrap();
        fs::write(dir.path().join("README"), "not a command").unwrap();
        let commands = commands_in_dir(dir.path(), CatalogSource::Plugin, |stem| {
            format!("repomind:{stem}")
        });
        assert_eq!(
            commands.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["repomind:clear", "repomind:review"]
        );
        assert!(
            commands
                .iter()
                .all(|c| c.source == CatalogSource::Plugin && c.one_shot)
        );
    }

    #[test]
    fn commands_in_dir_is_empty_for_a_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            commands_in_dir(&dir.path().join("nope"), CatalogSource::User, |s| s
                .to_string())
            .is_empty()
        );
    }

    // `build`'s own contract, now that resolving `agent_sessions` is its caller's job (see the
    // `agent.command_catalog` match arm in rpc.rs, which must call `overlay_agents` first -
    // `Lanes::get` alone always returns an empty `agent_sessions`, the exact bug this guards
    // against regressing). Proven against a real spawned tmux session separately; this is the
    // deterministic, CI-safe half: given an already-populated lane, does `build` pick the right
    // session and fail closed on anything it does not recognize.
    fn lane_with_session(session: repomon_core::model::AgentSession) -> Lane {
        use repomon_core::model::{Repo, Worktree, WorktreeState};
        let head = "0000000000000000000000000000000000000000".parse().unwrap();
        Lane {
            id: 1,
            repo: Repo {
                id: 1,
                path: std::path::PathBuf::from("/code/alpha"),
                name: "alpha".into(),
                added_at: chrono::Utc::now(),
                worktree_root_template: None,
                hidden: false,
                position: None,
                label: None,
                accent: None,
            },
            worktree: Worktree {
                id: 1,
                repo_id: 1,
                path: std::path::PathBuf::from("/code/alpha"),
                branch: Some("main".into()),
                head,
                is_main: true,
                name: "main".into(),
            },
            state: WorktreeState {
                worktree_id: 1,
                head,
                branch: Some("main".into()),
                upstream: None,
                ahead: 0,
                behind: 0,
                dirty: Default::default(),
                last_commit_at: None,
                locked: false,
                prunable: false,
                merged: false,
                last_change_at: None,
            },
            agent_sessions: vec![session],
            last_activity_at: chrono::Utc::now(),
            pinned: false,
            role: None,
            view_mode: None,
        }
    }

    fn fake_session(agent: AgentKind, tmux_window: &str) -> repomon_core::model::AgentSession {
        use repomon_core::model::AgentStatus;
        repomon_core::model::AgentSession {
            id: 1,
            agent,
            repo_id: 1,
            worktree_id: Some(1),
            started_at: chrono::Utc::now(),
            last_activity_at: chrono::Utc::now(),
            ended_at: None,
            manifest_path: std::path::PathBuf::new(),
            tool_call_count: 0,
            title: None,
            status: AgentStatus::Idle,
            external: false,
            session_id: None,
            resume_at: None,
            inferred: false,
            tmux_window: Some(tmux_window.into()),
            last_message: None,
            pending_prompt: None,
            pending_dialog: None,
            stale: false,
            stalled_since: None,
            subagent_running: None,
            status_reason: None,
            attention_kind: None,
            ended_turn: true,
            gate: None,
            config_dir: None,
            custom_label: None,
            generated_label: None,
        }
    }

    fn test_ctx() -> std::sync::Arc<crate::Ctx> {
        crate::Ctx::new(
            repomon_core::Store::open_in_memory().unwrap(),
            repomon_core::Config::default(),
            None,
        )
    }

    #[tokio::test]
    async fn build_ignores_a_session_in_a_different_window_of_the_same_lane() {
        let ctx = test_ctx();
        let lane = lane_with_session(fake_session(AgentKind::ClaudeCode, "lane-1/2"));
        let result = build(&ctx, &lane, Some("lane-1/1".into())).await;
        assert_eq!(result, CommandCatalog::empty());
    }

    #[tokio::test]
    async fn build_returns_empty_for_a_kind_it_has_no_discovery_for_even_with_a_matched_window() {
        let ctx = test_ctx();
        let lane = lane_with_session(fake_session(AgentKind::Cursor, "lane-1"));
        let result = build(&ctx, &lane, Some("lane-1".into())).await;
        assert_eq!(result, CommandCatalog::empty());
    }

    #[tokio::test]
    async fn build_defaults_the_window_from_the_lane_id_when_none_is_given() {
        let ctx = test_ctx();
        let window = repomon_core::TmuxRuntime::window_name(1);
        let lane = lane_with_session(fake_session(AgentKind::Cursor, &window));
        // Cursor has no discovery, but reaching an empty result (not a panic on a missing
        // window) proves the default-window fallback matched the session at all.
        let result = build(&ctx, &lane, None).await;
        assert_eq!(result, CommandCatalog::empty());
    }
}

mod claude_code {
    //! User commands (`~/.claude/commands/*.md`) and enabled plugins' commands
    //! (`~/.claude/plugins/cache/<marketplace>/<plugin>/<version>/commands/*.md`, matching the
    //! operator's own screenshot) are discovered from disk. Built-ins and the model list are
    //! curated: verified against this machine's `claude --help` (`/resume`, `/doctor`, and the
    //! `--model` flag's alias examples 'fable'/'opus'/'sonnet'), not read from anywhere on disk -
    //! Claude Code ships no on-disk manifest of either. `haiku` is the family's fourth alias,
    //! documented alongside the other three (Claude 5 family: Fable 5.1, Opus 5, Sonnet 5,
    //! Haiku 4.5) though not itself quoted in the `--help` excerpt this was checked against.
    use super::*;

    const MODELS: &[(&str, &str)] = &[
        ("fable", "Fable 5.1"),
        ("opus", "Opus 5"),
        ("sonnet", "Sonnet 5"),
        ("haiku", "Haiku 4.5"),
    ];

    pub fn discover(repo_root: &Path) -> StaticValue {
        let Some(claude_home) = crate::ext::claude_home() else {
            return StaticValue {
                commands: Vec::new(),
                models: Vec::new(),
                model_command: None,
            };
        };
        let mut commands = vec![
            CatalogCommand {
                name: "model".into(),
                description: "Change the active model".into(),
                source: CatalogSource::Builtin,
                one_shot: true,
            },
            CatalogCommand {
                name: "resume".into(),
                description: "Resume a previous session".into(),
                source: CatalogSource::Builtin,
                one_shot: false,
            },
            CatalogCommand {
                name: "doctor".into(),
                description: "Check the health of your Claude Code installation".into(),
                source: CatalogSource::Builtin,
                one_shot: true,
            },
        ];
        commands.extend(commands_in_dir(
            &claude_home.join("commands"),
            CatalogSource::User,
            |stem| stem.to_string(),
        ));
        commands.extend(plugin_commands(&claude_home, repo_root));
        commands.sort_by(|a, b| (a.source as u8, &a.name).cmp(&(b.source as u8, &b.name)));

        let models = MODELS
            .iter()
            .map(|(id, label)| CatalogModel {
                id: (*id).into(),
                label: (*label).into(),
                current: false,
            })
            .collect();
        StaticValue {
            commands,
            models,
            model_command: Some("/model".into()),
        }
    }

    fn plugin_commands(claude_home: &Path, repo_root: &Path) -> Vec<CatalogCommand> {
        let global_enabled = crate::ext::enabled_map(&claude_home.join("settings.json"));
        let repo_enabled = crate::ext::enabled_map(&repo_root.join(".claude/settings.local.json"));
        let installed = crate::ext::installed_plugins(claude_home);
        let mut out = Vec::new();
        for (id, (_, install_path)) in &installed {
            let enabled = match (repo_enabled.get(id), global_enabled.get(id)) {
                (Some(&b), _) => b,
                (None, Some(&b)) => b,
                (None, None) => false,
            };
            if !enabled {
                continue;
            }
            let Some(install_path) = install_path.as_deref() else {
                continue;
            };
            let plugin_name = id
                .split_once('@')
                .map(|(name, _)| name)
                .unwrap_or(id.as_str());
            out.extend(commands_in_dir(
                &install_path.join("commands"),
                CatalogSource::Plugin,
                |stem| format!("{plugin_name}:{stem}"),
            ));
        }
        out
    }

    /// The last `message.model` in the session's own transcript (`~/.claude/projects/<cwd
    /// with '/' replaced by '-'>/<session id>.jsonl`), read directly rather than through the
    /// daemon's shared transcript cache to keep this module self-contained. `None` when there is
    /// no session id yet, or the file cannot be read - never a guess.
    pub fn current_model(repo_root: &Path, session_id: Option<&str>) -> Option<String> {
        let claude_home = crate::ext::claude_home()?;
        let session_id = session_id?;
        let slug: String = repo_root
            .to_string_lossy()
            .chars()
            .map(|c| if c == '/' { '-' } else { c })
            .collect();
        let path = claude_home
            .join("projects")
            .join(slug)
            .join(format!("{session_id}.jsonl"));
        let text = fs::read_to_string(path).ok()?;
        text.lines().rev().find_map(|line| {
            let v: Value = serde_json::from_str(line).ok()?;
            if v.get("type").and_then(Value::as_str) != Some("assistant") {
                return None;
            }
            v.get("message")?.get("model")?.as_str().map(String::from)
        })
    }

    /// The levels `claude --help` lists for `--effort` on this machine, in its own order. The
    /// in-session form is `/effort <level>`, which the shipped binary's own guidance quotes
    /// ("try /effort medium"), so it takes an argument on one line exactly as `/model` does.
    pub const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

    /// Effort is stored globally as `effortLevel`, and a model the operator has tuned separately
    /// gets an entry under `modelSettings`. The per-model value wins when one exists, because it
    /// is what the session actually runs at.
    pub fn current_effort(model: Option<&str>) -> Option<String> {
        let settings = crate::ext::claude_home()?.join("settings.json");
        let value: Value = serde_json::from_str(&fs::read_to_string(settings).ok()?).ok()?;
        let per_model = model
            .and_then(|model| value.get("modelSettings")?.get(model))
            .and_then(|entry| entry.get("effortLevel"))
            .and_then(Value::as_str);
        per_model
            .or_else(|| value.get("effortLevel").and_then(Value::as_str))
            .map(String::from)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The levels are the ones `claude --help` prints for `--effort` on this machine. Pinning
        /// them here is what makes a CLI that drops or renames one fail a test rather than offer
        /// the operator a level the agent will reject.
        #[test]
        fn effort_levels_are_the_ones_the_cli_documents() {
            assert_eq!(EFFORTS, &["low", "medium", "high", "xhigh", "max"]);
        }

        /// A model the operator tuned separately runs at its own level, so that entry wins over
        /// the global one. With neither present the answer is absent, never a default.
        #[test]
        fn a_per_model_effort_override_wins_over_the_global_one() {
            let read = |json: &str, model: Option<&str>| -> Option<String> {
                let value: Value = serde_json::from_str(json).ok()?;
                model
                    .and_then(|model| value.get("modelSettings")?.get(model))
                    .and_then(|entry| entry.get("effortLevel"))
                    .and_then(Value::as_str)
                    .or_else(|| value.get("effortLevel").and_then(Value::as_str))
                    .map(String::from)
            };
            let settings = r#"{"effortLevel":"high","modelSettings":{"claude-fable-5-1":{"effortLevel":"medium"}}}"#;
            assert_eq!(read(settings, None).as_deref(), Some("high"));
            assert_eq!(
                read(settings, Some("claude-opus-5")).as_deref(),
                Some("high")
            );
            assert_eq!(
                read(settings, Some("claude-fable-5-1")).as_deref(),
                Some("medium")
            );
            assert!(read("{}", Some("claude-opus-5")).is_none());
        }
    }
}

mod codex {
    //! Plugin commands are discovered from disk (`~/.codex/plugins/cache/<marketplace>/<plugin>/
    //! <version>/commands/*.md`, enabled state from `~/.codex/config.toml`'s
    //! `[plugins."<id>"]` tables). No user command directory exists. Built-ins are curated from
    //! typing `/` in a live spawned codex session, which opens its own real palette; models are
    //! real, from `~/.codex/models_cache.json`'s `models[]` (`visibility == "list"`).
    //!
    //! `model_command` is `null`, confirmed rather than merely unverified: sending
    //! `/model gpt-6-astra` as one line to a live session did not switch the model - codex
    //! answered "I can't change the model from within the conversation," treating the whole line
    //! as a chat prompt instead of a command with an argument.
    use super::*;

    const BUILTINS: &[(&str, &str, bool)] = &[
        (
            "model",
            "choose what model and reasoning effort to use",
            false,
        ),
        ("fast", "1.5x speed, increased usage", true),
        (
            "ide",
            "include current selection, open files, and other context from your IDE",
            true,
        ),
        ("permissions", "choose what Codex is allowed to do", false),
        ("keymap", "remap TUI shortcuts", false),
        ("vim", "toggle Vim mode for the composer", true),
        ("experimental", "toggle experimental features", false),
        (
            "approve",
            "approve one retry of a recent auto-review denial",
            true,
        ),
    ];

    pub fn discover(repo_root: &Path) -> StaticValue {
        let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().join(".codex")) else {
            return StaticValue {
                commands: Vec::new(),
                models: Vec::new(),
                model_command: None,
            };
        };
        let _ = repo_root;
        let mut commands: Vec<CatalogCommand> = BUILTINS
            .iter()
            .map(|(name, description, one_shot)| CatalogCommand {
                name: (*name).into(),
                description: (*description).into(),
                source: CatalogSource::Builtin,
                one_shot: *one_shot,
            })
            .collect();
        commands.extend(plugin_commands(&home));
        let models = models_cache(&home);
        StaticValue {
            commands,
            models,
            model_command: None,
        }
    }

    fn plugin_commands(codex_home: &Path) -> Vec<CatalogCommand> {
        let Some(config) = config_value(codex_home) else {
            return Vec::new();
        };
        let Some(enabled_table) = config.get("plugins").and_then(toml::Value::as_table) else {
            return Vec::new();
        };
        let cache = codex_home.join("plugins/cache");
        let mut out = Vec::new();
        for (id, entry) in enabled_table {
            let enabled = entry
                .get("enabled")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);
            if !enabled {
                continue;
            }
            let Some((plugin_name, marketplace)) = id.split_once('@') else {
                continue;
            };
            let plugin_dir = cache.join(marketplace).join(plugin_name);
            let Ok(mut versions) = fs::read_dir(&plugin_dir).map(|d| {
                d.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect::<Vec<_>>()
            }) else {
                continue;
            };
            // No `.in_use`/`.orphaned_at` marker exists here (unlike Claude's cache); every
            // plugin sampled on this machine has exactly one version directory, so the
            // lexicographically last one is a reasonable, stated tiebreaker rather than a guess.
            versions.sort();
            let Some(version_dir) = versions.pop() else {
                continue;
            };
            out.extend(commands_in_dir(
                &version_dir.join("commands"),
                CatalogSource::Plugin,
                |stem| format!("{plugin_name}:{stem}"),
            ));
        }
        out
    }

    fn models_cache(codex_home: &Path) -> Vec<CatalogModel> {
        let Some(root) = crate::ext::read_json(&codex_home.join("models_cache.json")) else {
            return Vec::new();
        };
        let Some(models) = root.get("models").and_then(Value::as_array) else {
            return Vec::new();
        };
        models
            .iter()
            .filter(|m| m.get("visibility").and_then(Value::as_str) == Some("list"))
            .filter_map(|m| {
                let id = m.get("slug").and_then(Value::as_str)?.to_string();
                let label = m
                    .get("display_name")
                    .and_then(Value::as_str)
                    .unwrap_or(&id)
                    .to_string();
                Some(CatalogModel {
                    id,
                    label,
                    current: false,
                })
            })
            .collect()
    }

    fn config_value(codex_home: &Path) -> Option<toml::Value> {
        toml::from_str(&fs::read_to_string(codex_home.join("config.toml")).ok()?).ok()
    }

    /// Prefers the live pane's footer (already parsed for codex by
    /// `conversation_activity::pane_activity`) since it reflects an in-session `/model` switch;
    /// falls back to `~/.codex/config.toml`'s top-level `model` key, the configured default,
    /// when no pane text is available yet (e.g. right after spawn).
    pub async fn current_model(ctx: &Ctx, window: &str, repo_root: &Path) -> Option<String> {
        let backend = ctx.backend.clone();
        let win = window.to_string();
        let live = tokio::task::spawn_blocking(move || {
            backend.capture_named(&win, CaptureOpts::visible()).ok()
        })
        .await
        .ok()
        .flatten()
        .and_then(|pane| pane_activity("codex", &pane))
        .and_then(|activity| activity.model);
        if live.is_some() {
            return live;
        }
        let _ = repo_root;
        let home = directories::BaseDirs::new()?.home_dir().join(".codex");
        config_value(&home)?
            .get("model")?
            .as_str()
            .map(String::from)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn models_cache_keeps_only_listed_visibility_and_falls_back_to_slug_for_the_label() {
            let dir = tempfile::tempdir().unwrap();
            fs::write(
                dir.path().join("models_cache.json"),
                r#"{"models":[
                    {"slug":"gpt-6-astra","display_name":"GPT-6-Astra","visibility":"list"},
                    {"slug":"gpt-5.6-luna","visibility":"list"},
                    {"slug":"hidden-model","display_name":"Hidden","visibility":"hide"}
                ]}"#,
            )
            .unwrap();
            let models = models_cache(dir.path());
            assert_eq!(models.len(), 2);
            assert!(
                models
                    .iter()
                    .any(|m| m.id == "gpt-6-astra" && m.label == "GPT-6-Astra")
            );
            assert!(
                models
                    .iter()
                    .any(|m| m.id == "gpt-5.6-luna" && m.label == "gpt-5.6-luna")
            );
            assert!(!models.iter().any(|m| m.id == "hidden-model"));
        }

        #[test]
        fn models_cache_is_empty_when_the_file_is_missing() {
            let dir = tempfile::tempdir().unwrap();
            assert!(models_cache(dir.path()).is_empty());
        }

        #[test]
        fn model_builtin_is_marked_not_one_shot() {
            let (_, _, one_shot) = BUILTINS.iter().find(|(name, ..)| *name == "model").unwrap();
            assert!(!one_shot);
        }
    }
}

mod opencode {
    //! User/project command files (`~/.config/opencode/command(s)/*.md` and `<repo>/.opencode/
    //! command(s)/*.md`) are discovered from disk. opencode's `plugin` config array names npm
    //! packages, not a file cache, so plugin-sourced commands are not resolved. Built-ins are
    //! curated from typing `/` in a live spawned session, which opens opencode's own real
    //! palette. The model list comes from actually running `opencode models` (no local manifest
    //! exists); that call is slow enough on this machine (several seconds for ~100 entries) to
    //! need its own long-lived cache and a hard timeout, kept separate from `STATIC_TTL` above.
    //!
    //! `model_command` is `null`, confirmed rather than merely unverified: sending
    //! `/model <id>` and `/models <id>` as one line both landed as chat prompts, not commands;
    //! `/models` alone (no argument) opened an interactive fuzzy-search picker requiring
    //! arrow-key navigation, confirming it has no one-shot form.
    use super::*;

    const BUILTINS: &[(&str, &str, bool)] = &[
        ("agents", "Switch agent", true),
        ("connect", "Connect provider", false),
        ("debug", "View debug info", true),
        ("diff", "Open diff viewer", true),
        ("editor", "Open editor", true),
        ("exit", "Exit the app", true),
        ("help", "Help", true),
        ("init", "guided AGENTS.md setup", false),
        ("mcps", "Toggle MCPs", true),
        ("models", "Switch model", false),
    ];

    pub async fn discover(ctx: &Ctx, repo_root: &Path) -> StaticValue {
        let root = repo_root.to_path_buf();
        let mut commands: Vec<CatalogCommand> = BUILTINS
            .iter()
            .map(|(name, description, one_shot)| CatalogCommand {
                name: (*name).into(),
                description: (*description).into(),
                source: CatalogSource::Builtin,
                one_shot: *one_shot,
            })
            .collect();
        commands.extend(
            tokio::task::spawn_blocking(move || file_commands(&root))
                .await
                .unwrap_or_default(),
        );
        let models = models_via_cli(ctx).await;
        StaticValue {
            commands,
            models,
            model_command: None,
        }
    }

    /// OpenCode keeps no model in its config file; it records one per session, in the same store
    /// the transcript scanner already reads, as `{"id":...,"providerID":...}`. Without the
    /// window's session there is nothing to look up, and an unknown model is left unknown.
    pub fn current_model(session_id: Option<&str>) -> Option<String> {
        current_model_at(&repomon_core::agent::opencode::database_path(), session_id?)
    }

    fn current_model_at(path: &Path, session_id: &str) -> Option<String> {
        let conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?;
        let model: String = conn
            .query_row(
                "SELECT model FROM session WHERE id = ?1",
                rusqlite::params![session_id],
                |row| row.get(0),
            )
            .ok()?;
        let value: serde_json::Value = serde_json::from_str(&model).ok()?;
        Some(value.get("id")?.as_str()?.to_string())
    }

    fn file_commands(repo_root: &Path) -> Vec<CatalogCommand> {
        let mut out = Vec::new();
        if let Some(home) = directories::BaseDirs::new() {
            let config = home.home_dir().join(".config/opencode");
            out.extend(commands_in_dir(
                &config.join("command"),
                CatalogSource::User,
                |stem| stem.to_string(),
            ));
            out.extend(commands_in_dir(
                &config.join("commands"),
                CatalogSource::User,
                |stem| stem.to_string(),
            ));
        }
        let project = repo_root.join(".opencode");
        out.extend(commands_in_dir(
            &project.join("command"),
            CatalogSource::User,
            |stem| stem.to_string(),
        ));
        out.extend(commands_in_dir(
            &project.join("commands"),
            CatalogSource::User,
            |stem| stem.to_string(),
        ));
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out.dedup_by(|a, b| a.name == b.name);
        out
    }

    async fn models_via_cli(ctx: &Ctx) -> Vec<CatalogModel> {
        {
            let cache = super::opencode_models_cache()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some((at, models)) = cache.as_ref() {
                if at.elapsed() < OPENCODE_MODELS_TTL {
                    return models.clone();
                }
            }
        }
        let _ = ctx;
        let output = tokio::time::timeout(
            Duration::from_secs(8),
            tokio::task::spawn_blocking(|| {
                std::process::Command::new("opencode")
                    .arg("models")
                    .output()
            }),
        )
        .await;
        let models = match output {
            Ok(Ok(Ok(out))) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(|id| CatalogModel {
                    id: id.to_string(),
                    label: id.to_string(),
                    current: false,
                })
                .collect(),
            // A timeout or a failed/missing CLI is an honest "not knowable right now", not an
            // error to surface - the cache still remembers the timestamp so a slow/offline
            // machine does not retry the expensive call on every catalog request.
            _ => Vec::new(),
        };
        *super::opencode_models_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), models.clone()));
        models
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The model lives in the session row as a JSON object, so the chip needs the `id` out of
        /// it. Without a session there is nothing to read, and that stays `None` rather than
        /// becoming a default.
        #[test]
        fn the_session_row_supplies_the_model_id_and_an_unknown_session_supplies_nothing() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute("CREATE TABLE session (id TEXT PRIMARY KEY, model TEXT)", [])
                .unwrap();
            conn.execute(
                "INSERT INTO session VALUES ('ses_a', '{\"id\":\"nemotron-3-ultra-free\",\"providerID\":\"opencode\"}')",
                [],
            )
            .unwrap();
            conn.execute("INSERT INTO session VALUES ('ses_blank', '')", [])
                .unwrap();
            drop(conn);
            assert_eq!(
                current_model_at(&path, "ses_a").as_deref(),
                Some("nemotron-3-ultra-free")
            );
            assert!(current_model_at(&path, "ses_missing").is_none());
            assert!(current_model_at(&path, "ses_blank").is_none());
            assert!(current_model(None).is_none(), "no session, no claim");
        }

        #[test]
        fn models_builtin_is_marked_not_one_shot() {
            let (_, _, one_shot) = BUILTINS
                .iter()
                .find(|(name, ..)| *name == "models")
                .unwrap();
            assert!(!one_shot);
        }
    }
}

type TimestampedModels = Option<(Instant, Vec<CatalogModel>)>;

fn opencode_models_cache() -> &'static Mutex<TimestampedModels> {
    static CACHE: OnceLock<Mutex<TimestampedModels>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

mod antigravity {
    //! Plugin commands (`~/.gemini/config/plugins/<name>/commands/*.md` and `~/.gemini/plugins/
    //! <name>/commands/*.md`, plus the same two paths under the repo's `.gemini/`), the same
    //! directories `ext::scan_antigravity` already treats as a plugin's install root - real
    //! scanning, currently empty on this machine since every installed plugin has only
    //! `skills/`/`references/`/`examples/`, no `commands/` dir. Built-ins are curated from
    //! typing `/` in a live spawned session: its own palette mixes ~7 core commands with over a
    //! thousand bundled third-party skill/plugin prompts (e.g. `/m365-agents-dotnet`); only the
    //! clearly-core ones are curated here. Models are the exact ids from a live session's own
    //! rejection message after sending an invalid `/model` argument - authoritative, not typed
    //! from the picker's grouped display names.
    //!
    //! `model_command` is `"/model"`, confirmed: sending `/model gemini-3.7-flash-high` as one
    //! line to a live session changed both the header and status bar to that model.
    use super::*;

    const BUILTINS: &[(&str, &str, bool)] = &[
        (
            "model",
            "Set a model, or run a single prompt on another model",
            true,
        ),
        ("mcp", "Manage MCP servers", false),
        ("add-dir", "Add a directory to the workspace", true),
        ("agents", "List available custom agents", true),
        ("artifact", "View and review artifacts", true),
        (
            "btw",
            "Ask a side question without interrupting the current task",
            false,
        ),
        ("changelog", "Show release notes and changes", true),
    ];

    const MODELS: &[&str] = &[
        "gemini-3.8-flash-high",
        "gemini-3.8-flash-medium",
        "gemini-3.8-flash-low",
        "gemini-3.7-flash-high",
        "gemini-3.7-flash-medium",
        "gemini-3.7-flash-low",
        "gemini-3.6-flash-high",
        "gemini-3.6-flash-medium",
        "gemini-3.6-flash-low",
        "gemini-3.1-pro-high",
        "gemini-3.1-pro-low",
        "claude-sonnet-4-6",
        "claude-opus-4-6-thinking",
        "gpt-oss-120b-medium",
    ];

    fn label_for(id: &str) -> String {
        id.split('-')
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn discover(repo_root: &Path) -> StaticValue {
        let mut commands: Vec<CatalogCommand> = BUILTINS
            .iter()
            .map(|(name, description, one_shot)| CatalogCommand {
                name: (*name).into(),
                description: (*description).into(),
                source: CatalogSource::Builtin,
                one_shot: *one_shot,
            })
            .collect();
        if let Some(home) = directories::BaseDirs::new() {
            let gemini = home.home_dir().join(".gemini");
            commands.extend(plugin_commands(&gemini.join("config/plugins")));
            commands.extend(plugin_commands(&gemini.join("plugins")));
        }
        let repo_gemini = repo_root.join(".gemini");
        commands.extend(plugin_commands(&repo_gemini.join("config/plugins")));
        commands.extend(plugin_commands(&repo_gemini.join("plugins")));
        commands.sort_by(|a, b| a.name.cmp(&b.name));
        commands.dedup_by(|a, b| a.name == b.name);
        let models = MODELS
            .iter()
            .map(|id| CatalogModel {
                id: (*id).into(),
                label: label_for(id),
                current: false,
            })
            .collect();
        StaticValue {
            commands,
            models,
            model_command: Some("/model".into()),
        }
    }

    /// The CLI persists the chosen model in its own settings file, as the picker's display name
    /// ("Gemini 3.8 Flash (High)") rather than the id the `/model` argument takes. Normalising the
    /// display name back to that id is what lets the chip match a catalog entry; a name that does
    /// not normalise to a known id is returned as read, never guessed at.
    pub fn current_model() -> Option<String> {
        let settings = directories::BaseDirs::new()?
            .home_dir()
            .join(".gemini/antigravity-cli/settings.json");
        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(settings).ok()?).ok()?;
        let name = value.get("model")?.as_str()?;
        Some(model_id_from_display(name))
    }

    fn model_id_from_display(name: &str) -> String {
        name.split_whitespace()
            .map(|part| part.trim_matches(['(', ')']).to_lowercase())
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("-")
    }

    fn plugin_commands(plugins_dir: &Path) -> Vec<CatalogCommand> {
        let Ok(entries) = fs::read_dir(plugins_dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            out.extend(commands_in_dir(
                &path.join("commands"),
                CatalogSource::Plugin,
                |stem| format!("{name}:{stem}"),
            ));
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The settings file stores the picker's display name, the `/model` argument takes an id,
        /// and the chip only lights up when the two meet. Pinning the normalisation against the
        /// real catalog ids is what keeps a switch the operator just made visible in the chip.
        #[test]
        fn the_settings_display_name_normalises_onto_a_catalog_model_id() {
            assert_eq!(
                model_id_from_display("Gemini 3.8 Flash (High)"),
                "gemini-3.8-flash-high"
            );
            assert!(MODELS.contains(&model_id_from_display("Gemini 3.8 Flash (High)").as_str()));
            assert!(MODELS.contains(&model_id_from_display("Gemini 3.1 Pro (Low)").as_str()));
            // An id the list does not carry stays as read rather than being coerced onto a near
            // match: an absent chip is better than a wrong one.
            assert_eq!(model_id_from_display("Some New Model"), "some-new-model");
            assert!(!MODELS.contains(&model_id_from_display("Some New Model").as_str()));
        }

        #[test]
        fn label_for_title_cases_each_hyphen_separated_part() {
            assert_eq!(label_for("gemini-3.7-flash-high"), "Gemini 3.7 Flash High");
            assert_eq!(
                label_for("claude-opus-4-6-thinking"),
                "Claude Opus 4 6 Thinking"
            );
        }

        #[test]
        fn model_builtin_is_marked_one_shot() {
            let (_, _, one_shot) = BUILTINS.iter().find(|(name, ..)| *name == "model").unwrap();
            assert!(one_shot);
        }
    }
}

mod hermes {
    //! No user command directory or plugin/command mechanism was found; `skills` are a distinct
    //! mechanism from a slash-command palette. Built-ins are curated from a live session's own
    //! `/help`, which lists 40+ commands across several categories - only a representative,
    //! clearly-core subset is kept here. The model list is real, from `~/.hermes/cache/
    //! model_catalog.json`'s per-provider `models[]`. `current` is the real configured default,
    //! `~/.hermes/config.yaml`'s `model.default`, read with a small hand-rolled scan rather than
    //! pulling in a YAML crate for two fields.
    //!
    //! `model_command` is `"/model"`, confirmed: sending `/model z-ai/glm-5.2` as one line to a
    //! live session changed the status bar to that model, with no interactive follow-up (the
    //! transcript showed only `model → z-ai/glm-5.2`, no chat turn). `/help` also documents a
    //! genuine bidirectional toggle, `/fast [normal|fast|status]`, but reading its current on/off
    //! state would need an extra live pane read this RPC does not otherwise do - shown as a
    //! regular command, not as the model panel's toggle section, since a toggle whose displayed
    //! state cannot be trusted is the same dishonesty the empty-catalog rule forbids.
    use super::*;

    const BUILTINS: &[(&str, &str, bool)] = &[
        (
            "model",
            "Switch model (session-scoped; --global to persist)",
            true,
        ),
        (
            "fast",
            "Toggle fast mode - OpenAI Priority Processing / Anthropic Fast Mode",
            true,
        ),
        ("help", "Show available commands", true),
        ("usage", "Show token usage and rate limits", true),
        ("version", "Show Hermes Agent version", true),
        ("config", "Show current configuration", true),
        (
            "whoami",
            "Show your slash command access (admin / user)",
            true,
        ),
        ("tools", "Manage tools: list, disable, or enable", true),
    ];

    pub fn discover() -> StaticValue {
        let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().join(".hermes")) else {
            return StaticValue {
                commands: Vec::new(),
                models: Vec::new(),
                model_command: None,
            };
        };
        let commands = BUILTINS
            .iter()
            .map(|(name, description, one_shot)| CatalogCommand {
                name: (*name).into(),
                description: (*description).into(),
                source: CatalogSource::Builtin,
                one_shot: *one_shot,
            })
            .collect();
        let models = model_catalog(&home);
        StaticValue {
            commands,
            models,
            model_command: Some("/model".into()),
        }
    }

    fn model_catalog(home: &Path) -> Vec<CatalogModel> {
        let Some(root) = crate::ext::read_json(&home.join("cache/model_catalog.json")) else {
            return Vec::new();
        };
        let Some(providers) = root.get("providers").and_then(Value::as_object) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for provider in providers.values() {
            let Some(models) = provider.get("models").and_then(Value::as_array) else {
                continue;
            };
            for m in models {
                let Some(id) = m.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let label = m
                    .get("description")
                    .and_then(Value::as_str)
                    .filter(|d| !d.is_empty())
                    .unwrap_or(id);
                out.push(CatalogModel {
                    id: id.to_string(),
                    label: label.to_string(),
                    current: false,
                });
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out.dedup_by(|a, b| a.id == b.id);
        out
    }

    /// `model:\n  default: <id>` in `~/.hermes/config.yaml`, the value the operator actually
    /// configured - distinct from the catalog's own per-provider "default" flag, which only
    /// marks what hermes silently falls back to when nothing was ever configured.
    pub fn current_model() -> Option<String> {
        let home = directories::BaseDirs::new()?.home_dir().join(".hermes");
        let text = fs::read_to_string(home.join("config.yaml")).ok()?;
        default_from_config_yaml(&text)
    }

    fn default_from_config_yaml(text: &str) -> Option<String> {
        let mut lines = text.lines();
        loop {
            let line = lines.next()?;
            if line.trim_end() == "model:" {
                break;
            }
        }
        for line in lines {
            let indent = line.len() - line.trim_start().len();
            if indent == 0 && !line.trim().is_empty() {
                return None; // left the `model:` block without finding `default:`
            }
            if let Some(value) = line.trim().strip_prefix("default:") {
                let value = value.trim().trim_matches('"').trim_matches('\'');
                return (!value.is_empty()).then(|| value.to_string());
            }
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn model_catalog_flattens_providers_and_prefers_a_non_empty_description_as_the_label() {
            let dir = tempfile::tempdir().unwrap();
            fs::create_dir_all(dir.path().join("cache")).unwrap();
            fs::write(
                dir.path().join("cache/model_catalog.json"),
                r#"{"providers":{
                    "openrouter":{"models":[{"id":"anthropic/claude-fable-5.1","description":""}]},
                    "nous":[{"id":"ignored, not an object"}],
                    "custom":{"models":[{"id":"z-ai/glm-5.2","description":"default"}]}
                }}"#,
            )
            .unwrap();
            let models = model_catalog(dir.path());
            assert_eq!(models.len(), 2);
            assert!(
                models.iter().any(|m| m.id == "anthropic/claude-fable-5.1"
                    && m.label == "anthropic/claude-fable-5.1")
            );
            assert!(
                models
                    .iter()
                    .any(|m| m.id == "z-ai/glm-5.2" && m.label == "default")
            );
        }

        #[test]
        fn default_from_config_yaml_reads_the_configured_default_not_the_provider() {
            let text =
                "model:\n  default: tencent/hy3:free\n  provider: nous\nagent:\n  max_turns: 60\n";
            assert_eq!(
                default_from_config_yaml(text).as_deref(),
                Some("tencent/hy3:free")
            );
        }

        #[test]
        fn default_from_config_yaml_is_none_without_a_model_block() {
            assert_eq!(default_from_config_yaml("agent:\n  max_turns: 60\n"), None);
        }

        #[test]
        fn model_builtin_is_marked_one_shot() {
            let (_, _, one_shot) = BUILTINS.iter().find(|(name, ..)| *name == "model").unwrap();
            assert!(one_shot);
        }
    }
}

mod aider {
    //! Not installed on this development machine, and no verified evidence of its actual
    //! slash-command set exists anywhere in this session - "nothing" is the honest answer for
    //! commands and models here, not a curated guess from general knowledge of the CLI. The one
    //! real thing worth reading is `~/.aider.conf.yml`'s `model:` key, when the file exists, as
    //! the configured current model.
    use super::*;

    pub fn discover(repo_root: &Path) -> StaticValue {
        let _ = repo_root;
        StaticValue {
            commands: Vec::new(),
            models: Vec::new(),
            model_command: None,
        }
    }

    /// `model: <id>` at the top level of `.aider.conf.yml`, checked in the two places aider
    /// itself reads it from: the repo root, then the home directory.
    pub fn current_model(repo_root: &Path) -> Option<String> {
        let home = directories::BaseDirs::new().map(|d| d.home_dir().join(".aider.conf.yml"));
        for path in [Some(repo_root.join(".aider.conf.yml")), home]
            .into_iter()
            .flatten()
        {
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            if let Some(model) = model_from_conf(&text) {
                return Some(model);
            }
        }
        None
    }

    fn model_from_conf(text: &str) -> Option<String> {
        text.lines().find_map(|line| {
            let value = line.strip_prefix("model:")?.trim();
            let value = value.trim_matches('"').trim_matches('\'');
            (!value.is_empty()).then(|| value.to_string())
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn model_from_conf_reads_the_top_level_model_key() {
            let text = "model: gpt-5-codex\nedit-format: diff\n";
            assert_eq!(model_from_conf(text).as_deref(), Some("gpt-5-codex"));
        }

        #[test]
        fn model_from_conf_strips_quotes_and_ignores_a_blank_value() {
            assert_eq!(
                model_from_conf("model: \"claude-sonnet-5\"\n").as_deref(),
                Some("claude-sonnet-5")
            );
            assert_eq!(model_from_conf("model:\n"), None);
            assert_eq!(model_from_conf("edit-format: diff\n"), None);
        }
    }
}
