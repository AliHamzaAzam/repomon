//! JSON-RPC method dispatch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use repomon_core::agent::{self, shell_quote};
use repomon_core::git::reader;
use repomon_core::model::{
    AgentChoice, AgentKind, AgentSession, AgentStatus, BrowseEntry, BrowseResult, Commit,
    CreateLaneParams, Lane, RepoId, TimeRange,
};
use repomon_core::protocol::RpcError;
use repomon_core::{analytics, session, Indexer, TmuxRuntime};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::Ctx;

fn internal<E: std::fmt::Display>(e: E) -> RpcError {
    RpcError::internal(e.to_string())
}

fn parse<T: DeserializeOwned>(params: Option<Value>) -> Result<T, RpcError> {
    serde_json::from_value(params.unwrap_or(Value::Null))
        .map_err(|e| RpcError::invalid_params(e.to_string()))
}

fn to_value<T: serde::Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(internal)
}

/// The editable subset of the config exposed to the Settings view.
fn config_json(cfg: &repomon_core::config::Config) -> Value {
    json!({
        "accent": cfg.accent,
        "auto_continue": cfg.auto_continue,
        "auto_continue_message": cfg.auto_continue_message,
        "default_agent": cfg.default_agent,
        "worktree_template": cfg.worktree_template,
        "spawn_prompt": cfg.spawn_prompt,
        "notify_enabled": cfg.notify_enabled,
        "notify_needs_you": cfg.notify_needs_you,
        "notify_rate_limited": cfg.notify_rate_limited,
        "notify_resumed": cfg.notify_resumed,
        "notify_idle": cfg.notify_idle,
        "notify_sound": cfg.notify_sound,
        "notify_show_why": cfg.notify_show_why,
        "notify_coalesce": cfg.notify_coalesce,
        "notify_click_focus": cfg.notify_click_focus,
        "notify_subagents": cfg.notify_subagents,
        "usage_probe": cfg.usage_probe,
        "expand_agents": cfg.expand_agents,
    })
}

#[derive(Deserialize)]
struct RepoAdd {
    path: String,
}
#[derive(Deserialize)]
struct RepoRemove {
    repo_id: RepoId,
}
#[derive(Deserialize)]
struct Discover {
    root: String,
    #[serde(default = "default_depth")]
    max_depth: usize,
}
fn default_depth() -> usize {
    4
}
#[derive(Deserialize)]
struct LaneId {
    lane_id: repomon_core::model::LaneId,
}
#[derive(Deserialize)]
struct LaneDelete {
    lane_id: repomon_core::model::LaneId,
    #[serde(default)]
    also_delete_branch: bool,
}
#[derive(Deserialize)]
struct CommitRange {
    from_iso: String,
    to_iso: String,
    #[serde(default)]
    repo_ids: Option<Vec<RepoId>>,
}
#[derive(Deserialize)]
struct AgentSpawn {
    lane_id: repomon_core::model::LaneId,
    agent: String,
    #[serde(default)]
    task: Option<String>,
}
#[derive(Deserialize)]
struct AgentInput {
    lane_id: repomon_core::model::LaneId,
    text: String,
    /// Press Enter after the text (default). `false` just inserts it (e.g. a pasted path).
    #[serde(default = "default_true")]
    enter: bool,
    /// Route to a specific agent window (several can share a lane); `None` = first slot.
    #[serde(default)]
    window: Option<String>,
}
fn default_true() -> bool {
    true
}
#[derive(Deserialize)]
struct AgentSignal {
    lane_id: repomon_core::model::LaneId,
    key: String,
    #[serde(default)]
    window: Option<String>,
}
#[derive(Deserialize)]
struct AgentKey {
    lane_id: repomon_core::model::LaneId,
    key: String,
    #[serde(default)]
    literal: bool,
    #[serde(default)]
    window: Option<String>,
}
#[derive(Deserialize)]
struct AgentCapture {
    lane_id: repomon_core::model::LaneId,
    #[serde(default)]
    lines: Option<u32>,
    #[serde(default)]
    window: Option<String>,
}
#[derive(Deserialize)]
struct AgentStop {
    lane_id: repomon_core::model::LaneId,
    /// Stop one specific agent window; `None` stops the lane's first slot.
    #[serde(default)]
    window: Option<String>,
}
#[derive(Deserialize)]
struct AgentTarget {
    lane_id: repomon_core::model::LaneId,
    #[serde(default)]
    window: Option<String>,
}
#[derive(Deserialize)]
struct AgentResize {
    lane_id: repomon_core::model::LaneId,
    cols: u16,
    rows: u16,
    #[serde(default)]
    window: Option<String>,
}
#[derive(Deserialize)]
struct AgentScroll {
    lane_id: repomon_core::model::LaneId,
    up: bool,
    #[serde(default = "default_scroll_ticks")]
    ticks: u32,
    #[serde(default)]
    window: Option<String>,
}
fn default_scroll_ticks() -> u32 {
    1
}
#[derive(Deserialize)]
struct AgentAutoContinue {
    lane_id: repomon_core::model::LaneId,
    enabled: bool,
}
#[derive(Deserialize)]
struct AgentAdd {
    name: String,
    command: String,
}
#[derive(Deserialize)]
struct AgentRemove {
    name: String,
}
#[derive(Deserialize)]
struct AgentSetDefault {
    #[serde(default)]
    name: Option<String>,
}
/// A partial config update from the Settings view — only the present fields are applied.
#[derive(Deserialize)]
struct ConfigSet {
    #[serde(default)]
    accent: Option<String>,
    #[serde(default)]
    auto_continue: Option<bool>,
    #[serde(default)]
    auto_continue_message: Option<String>,
    #[serde(default)]
    default_agent: Option<String>,
    #[serde(default)]
    worktree_template: Option<String>,
    #[serde(default)]
    spawn_prompt: Option<bool>,
    #[serde(default)]
    notify_enabled: Option<bool>,
    #[serde(default)]
    notify_needs_you: Option<bool>,
    #[serde(default)]
    notify_rate_limited: Option<bool>,
    #[serde(default)]
    notify_resumed: Option<bool>,
    #[serde(default)]
    notify_idle: Option<bool>,
    #[serde(default)]
    notify_sound: Option<bool>,
    #[serde(default)]
    notify_show_why: Option<bool>,
    #[serde(default)]
    notify_coalesce: Option<bool>,
    #[serde(default)]
    notify_click_focus: Option<bool>,
    #[serde(default)]
    notify_subagents: Option<bool>,
    #[serde(default)]
    usage_probe: Option<bool>,
    #[serde(default)]
    expand_agents: Option<bool>,
}
#[derive(Deserialize)]
struct PushDevice {
    device_token: String,
}
#[derive(Deserialize)]
struct SessionRename {
    /// The transcript session id to label (durable across restarts).
    session_id: String,
    /// The new label; `None`/absent or empty clears it.
    #[serde(default)]
    label: Option<String>,
}
#[derive(Deserialize)]
struct AgentTranscript {
    lane_id: repomon_core::model::LaneId,
    /// Which session's transcript; `None` = the lane's most recent.
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default = "default_transcript_limit")]
    limit: usize,
}
fn default_transcript_limit() -> usize {
    50
}
#[derive(Deserialize)]
struct AgentAdopt {
    lane_id: repomon_core::model::LaneId,
    /// Resume this exact session (`claude --resume <id>`); `None` resumes the most recent.
    #[serde(default)]
    session_id: Option<String>,
}
#[derive(Deserialize)]
struct AgentPin {
    lane_id: repomon_core::model::LaneId,
    pinned: bool,
}
#[derive(Deserialize)]
struct TerminalId {
    id: String,
}
#[derive(Deserialize)]
struct ViewportSet {
    lane_ids: Vec<repomon_core::model::LaneId>,
    /// Which agent window the focused lane's pane should stream (Tab cycling in Focus/Split);
    /// other viewport lanes stream their first slot.
    #[serde(default)]
    focus_lane: Option<repomon_core::model::LaneId>,
    #[serde(default)]
    focus_window: Option<String>,
}
#[derive(Deserialize)]
struct LaneMerge {
    lane_id: repomon_core::model::LaneId,
    #[serde(default)]
    into: Option<String>,
}
#[derive(Deserialize)]
struct Search {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    50
}
#[derive(Deserialize)]
struct CommitRecent {
    #[serde(default)]
    lane_id: Option<repomon_core::model::LaneId>,
    #[serde(default)]
    repo_id: Option<RepoId>,
    #[serde(default = "default_recent_limit")]
    limit: usize,
}
fn default_recent_limit() -> usize {
    8
}
#[derive(Deserialize)]
struct TimelineParams {
    from_iso: String,
    to_iso: String,
    #[serde(default = "default_bucket")]
    bucket_secs: i64,
}
fn default_bucket() -> i64 {
    3600
}
#[derive(Deserialize)]
struct SessionsParams {
    from_iso: String,
    to_iso: String,
}
#[derive(Deserialize)]
struct Browse {
    #[serde(default)]
    path: Option<String>,
}

/// Dispatch a single request to its handler.
pub async fn dispatch(ctx: &Ctx, method: &str, params: Option<Value>) -> Result<Value, RpcError> {
    match method {
        // ---- system ----
        // The local TUI calls this just before parking in a full-screen tmux attach (where it
        // stops sending its lane.list heartbeat). `socket` special-cases the method to age out
        // `local_watcher_seen` so the daemon takes over desktop popups on its very next tick
        // instead of waiting out LOCAL_TTL — closing the handoff gap. The dispatch is a no-op ack.
        "watcher.park" => to_value(()),

        // ---- repos ----
        "repo.list" => to_value(ctx.registry.list().await.map_err(internal)?),
        "repo.add" => {
            let p: RepoAdd = parse(params)?;
            let repo = ctx
                .registry
                .add(std::path::Path::new(&p.path))
                .await
                .map_err(internal)?;
            // Start watching the new repo's tree at runtime (the watcher otherwise only knows the
            // repos present at startup).
            if let Some(w) = ctx.watcher.lock().await.as_mut() {
                let _ = w.watch_path(&repo.path);
            }
            ctx.broadcast(crate::pubsub::topic::REPO_ADDED, json!({ "repo": repo }));
            // Index the new repo's history in the background.
            let indexer = Indexer::new(ctx.store.clone(), ctx.registry.clone());
            let repo_for_index = repo.clone();
            tokio::spawn(async move {
                let _ = indexer.sync(&repo_for_index).await;
            });
            to_value(repo)
        }
        "repo.remove" => {
            let p: RepoRemove = parse(params)?;
            // Stop watching the repo's tree before dropping it, so the file watcher isn't left
            // churning fsevents over a repo that's no longer registered.
            if let Ok(repo) = ctx.store.get_repo(p.repo_id).await {
                if let Some(w) = ctx.watcher.lock().await.as_mut() {
                    let _ = w.unwatch_path(&repo.path);
                }
            }
            ctx.registry.remove(p.repo_id).await.map_err(internal)?;
            ctx.broadcast(
                crate::pubsub::topic::REPO_REMOVED,
                json!({ "repo_id": p.repo_id }),
            );
            Ok(Value::Null)
        }
        "repo.discover" => {
            let p: Discover = parse(params)?;
            let found = ctx
                .registry
                .discover(std::path::Path::new(&p.root), p.max_depth)
                .await
                .map_err(internal)?;
            let paths: Vec<String> = found
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            to_value(paths)
        }

        // ---- lanes ----
        "lane.list" => to_value(lanes_with_agents(ctx).await?),
        "lane.get" => {
            let p: LaneId = parse(params)?;
            let lane = ctx.lanes.get(p.lane_id).await.map_err(internal)?;
            let mut one = vec![lane];
            overlay_agents(ctx, &mut one).await;
            to_value(one.into_iter().next().unwrap())
        }
        "lane.create" => {
            let p: CreateLaneParams = parse(params)?;
            let lane = ctx.lanes.create(p).await.map_err(internal)?;
            ctx.broadcast(crate::pubsub::topic::LANE_CREATED, json!({ "lane": lane }));
            ctx.invalidate_overlay().await;
            to_value(lane)
        }
        "lane.delete" => {
            let p: LaneDelete = parse(params)?;
            ctx.lanes
                .delete(p.lane_id, p.also_delete_branch)
                .await
                .map_err(internal)?;
            ctx.broadcast(
                crate::pubsub::topic::LANE_DELETED,
                json!({ "lane_id": p.lane_id }),
            );
            ctx.invalidate_overlay().await;
            Ok(Value::Null)
        }
        "lane.focus" => {
            let p: LaneId = parse(params)?;
            let path = ctx.lanes.focus(p.lane_id).await.map_err(internal)?;
            Ok(json!({ "path": path.to_string_lossy() }))
        }
        "lane.merge" => {
            let p: LaneMerge = parse(params)?;
            let message = ctx.lanes.merge(p.lane_id, p.into).await.map_err(internal)?;
            Ok(json!({ "message": message }))
        }

        // ---- commits (computed live via gix) ----
        "commit.today" => {
            let range = today_range();
            to_value(commits_in_range(ctx, range, None).await?)
        }
        "commit.range" => {
            let p: CommitRange = parse(params)?;
            let from = parse_iso(&p.from_iso)?;
            let to = parse_iso(&p.to_iso)?;
            to_value(commits_in_range(ctx, TimeRange { from, to }, p.repo_ids).await?)
        }
        "commit.search" => {
            let p: Search = parse(params)?;
            to_value(
                ctx.store
                    .search_commits(p.query, p.limit)
                    .await
                    .map_err(internal)?,
            )
        }
        "commit.recent" => {
            let p: CommitRecent = parse(params)?;
            // A lane shows its worktree's branch history; otherwise the repo's main HEAD.
            let (path, repo_id) = if let Some(lid) = p.lane_id {
                let lane = ctx.lanes.get(lid).await.map_err(internal)?;
                (lane.worktree.path.clone(), lane.repo.id)
            } else if let Some(rid) = p.repo_id {
                let repo = ctx
                    .registry
                    .list()
                    .await
                    .map_err(internal)?
                    .into_iter()
                    .find(|r| r.id == rid)
                    .ok_or_else(|| RpcError::invalid_params(format!("no repo {rid}")))?;
                (repo.path.clone(), repo.id)
            } else {
                return Err(RpcError::invalid_params("lane_id or repo_id is required"));
            };
            let limit = p.limit;
            let commits = tokio::task::spawn_blocking(move || {
                reader::read_recent_commits(&path, repo_id, limit)
            })
            .await
            .map_err(internal)?
            .unwrap_or_default();
            to_value(commits)
        }

        // ---- dashboard (Phase 3, from the indexed store) ----
        "timeline" => {
            let p: TimelineParams = parse(params)?;
            let range = TimeRange {
                from: parse_iso(&p.from_iso)?,
                to: parse_iso(&p.to_iso)?,
            };
            let commits = ctx
                .store
                .commits_in_range(range, None)
                .await
                .map_err(internal)?;
            let names = repo_names(ctx).await;
            to_value(analytics::build_timeline(
                &commits,
                &names,
                range.from,
                range.to,
                p.bucket_secs,
            ))
        }
        "sessions" => {
            let p: SessionsParams = parse(params)?;
            let range = TimeRange {
                from: parse_iso(&p.from_iso)?,
                to: parse_iso(&p.to_iso)?,
            };
            let commits = ctx
                .store
                .commits_in_range(range, None)
                .await
                .map_err(internal)?;
            let names = repo_names(ctx).await;
            to_value(session::detect(&commits, &names))
        }

        // ---- agents (tmux-backed runtime) ----
        "agent.detect" => {
            let cfg = ctx.config.read().await;
            let default = cfg.default_agent.clone();
            let is_default = |name: &str| default.as_deref() == Some(name);
            let mut choices: Vec<AgentChoice> = Vec::new();
            // One Claude entry per detected config dir (default + ~/.claude-* + $CLAUDE_CONFIG_DIR).
            for (name, command) in agent::claude::agent_variants() {
                choices.push(AgentChoice {
                    detected: on_path(&command),
                    default: is_default(&name),
                    name,
                    command,
                    custom: false,
                });
            }
            for kind in [AgentKind::Codex, AgentKind::Aider] {
                let command = kind.command().to_string();
                let name = kind.as_str().into_owned();
                choices.push(AgentChoice {
                    detected: on_path(&command),
                    default: is_default(&name),
                    name,
                    command,
                    custom: false,
                });
            }
            let mut customs: Vec<_> = cfg.agents.iter().collect();
            customs.sort_by_key(|(name, _)| name.to_string());
            for (name, command) in customs {
                choices.push(AgentChoice {
                    detected: on_path(command),
                    default: is_default(name),
                    name: name.clone(),
                    command: command.clone(),
                    custom: true,
                });
            }
            to_value(choices)
        }
        "agent.add" => {
            let p: AgentAdd = parse(params)?;
            let name = p.name.trim().to_string();
            let command = p.command.trim().to_string();
            if name.is_empty() || command.is_empty() {
                return Err(RpcError::invalid_params("name and command are required"));
            }
            if is_builtin(&name) {
                return Err(RpcError::invalid_params(format!(
                    "'{name}' is a built-in agent name; pick a different name"
                )));
            }
            {
                let mut cfg = ctx.config.write().await;
                let prev = cfg.agents.insert(name.clone(), command.clone());
                if let Err(e) = cfg.save_to(&ctx.config_path) {
                    match prev {
                        Some(v) => {
                            cfg.agents.insert(name.clone(), v);
                        }
                        None => {
                            cfg.agents.remove(&name);
                        }
                    }
                    return Err(internal(e));
                }
            }
            ctx.broadcast(crate::pubsub::topic::AGENT_CHANGED, json!({ "name": name }));
            Ok(Value::Null)
        }
        "agent.remove" => {
            let p: AgentRemove = parse(params)?;
            if is_builtin(&p.name) {
                return Err(RpcError::invalid_params("cannot remove a built-in agent"));
            }
            {
                let mut cfg = ctx.config.write().await;
                let prev = match cfg.agents.remove(&p.name) {
                    Some(v) => v,
                    None => {
                        return Err(RpcError::invalid_params(format!(
                            "no custom agent named '{}'",
                            p.name
                        )))
                    }
                };
                let prev_default = cfg.default_agent.clone();
                if cfg.default_agent.as_deref() == Some(p.name.as_str()) {
                    cfg.default_agent = None;
                }
                if let Err(e) = cfg.save_to(&ctx.config_path) {
                    cfg.agents.insert(p.name.clone(), prev);
                    cfg.default_agent = prev_default;
                    return Err(internal(e));
                }
            }
            ctx.broadcast(
                crate::pubsub::topic::AGENT_CHANGED,
                json!({ "name": p.name }),
            );
            Ok(Value::Null)
        }
        "agent.set_default" => {
            let p: AgentSetDefault = parse(params)?;
            {
                let mut cfg = ctx.config.write().await;
                if let Some(name) = &p.name {
                    if !is_builtin(name) && !cfg.agents.contains_key(name) {
                        return Err(RpcError::invalid_params(format!("unknown agent '{name}'")));
                    }
                }
                let prev = cfg.default_agent.clone();
                cfg.default_agent = p.name.clone();
                if let Err(e) = cfg.save_to(&ctx.config_path) {
                    cfg.default_agent = prev;
                    return Err(internal(e));
                }
            }
            ctx.broadcast(
                crate::pubsub::topic::AGENT_CHANGED,
                json!({ "default": p.name }),
            );
            Ok(Value::Null)
        }
        "config.get" => {
            let cfg = ctx.config.read().await;
            Ok(config_json(&cfg))
        }
        "config.set" => {
            let p: ConfigSet = parse(params)?;
            {
                let mut cfg = ctx.config.write().await;
                let prev = cfg.clone();
                if let Some(a) = p.accent {
                    cfg.accent = Some(a);
                }
                if let Some(b) = p.auto_continue {
                    cfg.auto_continue = b;
                }
                if let Some(m) = p.auto_continue_message {
                    cfg.auto_continue_message = m;
                }
                if let Some(d) = p.default_agent {
                    cfg.default_agent = Some(d);
                }
                if let Some(w) = p.worktree_template {
                    cfg.worktree_template = w;
                }
                if let Some(b) = p.spawn_prompt {
                    cfg.spawn_prompt = b;
                }
                if let Some(b) = p.notify_enabled {
                    cfg.notify_enabled = b;
                }
                if let Some(b) = p.notify_needs_you {
                    cfg.notify_needs_you = b;
                }
                if let Some(b) = p.notify_rate_limited {
                    cfg.notify_rate_limited = b;
                }
                if let Some(b) = p.notify_resumed {
                    cfg.notify_resumed = b;
                }
                if let Some(b) = p.notify_idle {
                    cfg.notify_idle = b;
                }
                if let Some(b) = p.notify_sound {
                    cfg.notify_sound = b;
                }
                if let Some(b) = p.notify_show_why {
                    cfg.notify_show_why = b;
                }
                if let Some(b) = p.notify_coalesce {
                    cfg.notify_coalesce = b;
                }
                if let Some(b) = p.notify_click_focus {
                    cfg.notify_click_focus = b;
                }
                if let Some(b) = p.notify_subagents {
                    cfg.notify_subagents = b;
                }
                if let Some(b) = p.usage_probe {
                    cfg.usage_probe = b;
                }
                if let Some(b) = p.expand_agents {
                    cfg.expand_agents = b;
                }
                if let Err(e) = cfg.save_to(&ctx.config_path) {
                    *cfg = prev;
                    return Err(internal(e));
                }
            }
            let cfg = ctx.config.read().await;
            Ok(config_json(&cfg))
        }
        "agent.spawn" => {
            let p: AgentSpawn = parse(params)?;
            let path = ctx.lanes.focus(p.lane_id).await.map_err(internal)?;
            // Resolve the chosen name to a command: a config custom wins, then an autodetected
            // Claude variant (e.g. claude-work → `CLAUDE_CONFIG_DIR=… claude`), else the kind.
            let mut command = {
                let cfg = ctx.config.read().await;
                if let Some(c) = cfg.agents.get(&p.agent) {
                    c.clone()
                } else if let Some((_, cmd)) = agent::claude::agent_variants()
                    .into_iter()
                    .find(|(n, _)| n == &p.agent)
                {
                    cmd
                } else {
                    AgentKind::from_kind_str(&p.agent).command().to_string()
                }
            };
            if let Some(task) = p.task.as_deref().filter(|t| !t.is_empty()) {
                command = format!("{command} {}", shell_quote(task));
            }
            let tmux = ctx.tmux.clone();
            let lane = p.lane_id;
            let window = tokio::task::spawn_blocking(move || tmux.spawn(lane, &path, &command))
                .await
                .map_err(internal)?
                .map_err(internal)?;
            let _ = ctx
                .store
                .set_lane_tmux_window(p.lane_id, Some(window.clone()))
                .await;
            let _ = ctx
                .store
                .set_lane_agent_kind(p.lane_id, Some(p.agent.clone()))
                .await;
            ctx.broadcast(
                crate::pubsub::topic::AGENT_STATUS,
                json!({ "lane_id": p.lane_id, "status": "running" }),
            );
            ctx.invalidate_overlay().await;
            Ok(json!({ "lane_id": p.lane_id, "window": window, "agent": p.agent }))
        }
        "agent.adopt" => {
            // Take over an agent running in another terminal: resume its conversation in a
            // managed tmux lane. With a session id we resume that exact session
            // (`claude --resume <id>`); otherwise the most recent one (`--continue`). Either
            // way we resolve which Claude account (config dir) it belongs to so a work-account
            // session resumes against ~/.claude-work.
            let p: AgentAdopt = parse(params)?;
            let path = ctx.lanes.focus(p.lane_id).await.map_err(internal)?;
            let (default_agent, customs) = {
                let cfg = ctx.config.read().await;
                (cfg.default_agent.clone(), cfg.agents.clone())
            };
            // `command` is ultimately run via `sh -c` by tmux, so a session id that isn't a plain
            // transcript id (UUID / `[A-Za-z0-9_-]`) could inject shell. Reject it up front.
            if let Some(sid) = &p.session_id {
                if !valid_session_id(sid) {
                    return Err(RpcError::invalid_params("invalid session_id"));
                }
            }
            let detect = path.clone();
            let session_id = p.session_id.clone();
            let command = tokio::task::spawn_blocking(move || {
                // Which account (config dir) the session belongs to, and how to resume it.
                let (config_dir, resume) = match &session_id {
                    Some(sid) => (
                        agent::claude::config_base_for_session(&detect, sid).flatten(),
                        // Validated above; quote anyway as defense-in-depth.
                        format!("--resume {}", agent::tmux::shell_quote(sid)),
                    ),
                    None => (
                        agent::claude::summary_for(&detect).and_then(|s| s.config_dir),
                        "--continue".to_string(),
                    ),
                };
                let base = adopt_base_command(&default_agent, &customs, &config_dir);
                format!("{base} {resume}")
            })
            .await
            .map_err(internal)?;
            let tmux = ctx.tmux.clone();
            let lane = p.lane_id;
            let window = tokio::task::spawn_blocking(move || tmux.spawn(lane, &path, &command))
                .await
                .map_err(internal)?
                .map_err(internal)?;
            let _ = ctx
                .store
                .set_lane_tmux_window(p.lane_id, Some(window.clone()))
                .await;
            let _ = ctx
                .store
                .set_lane_agent_kind(p.lane_id, Some("claude-code".to_string()))
                .await;
            ctx.broadcast(
                crate::pubsub::topic::AGENT_STATUS,
                json!({ "lane_id": p.lane_id, "status": "running" }),
            );
            ctx.invalidate_overlay().await;
            Ok(json!({ "lane_id": p.lane_id, "window": window }))
        }
        "agent.capture" => {
            let p: AgentCapture = parse(params)?;
            let tmux = ctx.tmux.clone();
            let lines = p.lines;
            let window = p
                .window
                .unwrap_or_else(|| TmuxRuntime::window_name(p.lane_id));
            let content = tokio::task::spawn_blocking(move || tmux.capture_named(&window, lines))
                .await
                .map_err(internal)?
                .map_err(internal)?;
            Ok(json!({ "content": content }))
        }
        "agent.send_input" => {
            let p: AgentInput = parse(params)?;
            let tmux = ctx.tmux.clone();
            let (lane, text, enter) = (p.lane_id, p.text, p.enter);
            let window = p.window.unwrap_or_else(|| TmuxRuntime::window_name(lane));
            tokio::task::spawn_blocking(move || {
                if enter {
                    tmux.send_text_named(&window, &text)
                } else {
                    tmux.send_literal_named(&window, &text)
                }
            })
            .await
            .map_err(internal)?
            .map_err(internal)?;
            ctx.input_seen.lock().await.insert(lane, std::time::Instant::now());
            Ok(Value::Null)
        }
        "agent.signal" => {
            let p: AgentSignal = parse(params)?;
            let tmux = ctx.tmux.clone();
            let (lane, key) = (p.lane_id, p.key);
            let window = p.window.unwrap_or_else(|| TmuxRuntime::window_name(lane));
            tokio::task::spawn_blocking(move || tmux.send_key_named(&window, &key))
                .await
                .map_err(internal)?
                .map_err(internal)?;
            ctx.input_seen.lock().await.insert(lane, std::time::Instant::now());
            Ok(Value::Null)
        }
        "agent.key" => {
            let p: AgentKey = parse(params)?;
            let tmux = ctx.tmux.clone();
            let (lane, key, literal) = (p.lane_id, p.key, p.literal);
            let window = p.window.unwrap_or_else(|| TmuxRuntime::window_name(lane));
            tokio::task::spawn_blocking(move || {
                if literal {
                    tmux.send_literal_named(&window, &key)
                } else {
                    tmux.send_key_named(&window, &key)
                }
            })
            .await
            .map_err(internal)?
            .map_err(internal)?;
            ctx.input_seen.lock().await.insert(lane, std::time::Instant::now());
            Ok(Value::Null)
        }
        "agent.stop" => {
            let p: AgentStop = parse(params)?;
            let tmux = ctx.tmux.clone();
            let lane = p.lane_id;
            let window = p.window.unwrap_or_else(|| TmuxRuntime::window_name(lane));
            let remaining = tokio::task::spawn_blocking(move || {
                let _ = tmux.kill_named(&window);
                tmux.windows_for(lane).unwrap_or_default().len()
            })
            .await
            .unwrap_or(0);
            if remaining == 0 {
                let _ = ctx.store.set_lane_tmux_window(p.lane_id, None).await;
            }
            ctx.broadcast(
                crate::pubsub::topic::AGENT_STATUS,
                json!({ "lane_id": p.lane_id, "status": "ended" }),
            );
            ctx.invalidate_overlay().await;
            Ok(Value::Null)
        }
        "agent.pin" => {
            let p: AgentPin = parse(params)?;
            ctx.store
                .set_lane_pinned(p.lane_id, p.pinned)
                .await
                .map_err(internal)?;
            Ok(Value::Null)
        }
        "agent.target" => {
            let p: AgentTarget = parse(params)?;
            let tmux = ctx.tmux.clone();
            let window = p
                .window
                .unwrap_or_else(|| TmuxRuntime::window_name(p.lane_id));
            // This is the pre-attach hook (the TUI calls it right before `tmux attach`). The
            // mediated view sizes the window to its pane with `agent.resize` (which sets
            // window-size manual); restore client-follow so the attaching real terminal renders the
            // agent at full size. The TUI re-fits it on return.
            let w = window.clone();
            let available = tokio::task::spawn_blocking(move || {
                let _ = tmux.follow_client_named(&w);
                tmux.has_named(&w)
            })
            .await
            .map_err(internal)?;
            let target = format!("{}:={}", ctx.tmux.session(), window);
            Ok(json!({ "target": target, "available": available }))
        }
        "agent.resize" => {
            let p: AgentResize = parse(params)?;
            let tmux = ctx.tmux.clone();
            let window = p
                .window
                .unwrap_or_else(|| TmuxRuntime::window_name(p.lane_id));
            // Clamp to a sane floor so a momentary tiny layout can't shrink the agent to nothing.
            let (cols, rows) = (p.cols.max(20), p.rows.max(4));
            tokio::task::spawn_blocking(move || tmux.resize_named(&window, cols, rows))
                .await
                .map_err(internal)?
                .map_err(internal)?;
            Ok(Value::Null)
        }
        "agent.scroll" => {
            let p: AgentScroll = parse(params)?;
            let tmux = ctx.tmux.clone();
            let lane = p.lane_id;
            let window = p.window.unwrap_or_else(|| TmuxRuntime::window_name(lane));
            let (up, ticks) = (p.up, p.ticks.min(40));
            // Only forward to a full-screen agent (alternate screen) — it owns its scrollback, so
            // it can scroll itself. A plain shell would just get junk on its command line; the
            // caller falls back to the capture-based scroll when `forwarded` is false.
            let forwarded = tokio::task::spawn_blocking(move || {
                if tmux.alternate_on_named(&window) {
                    let _ = tmux.scroll_wheel_named(&window, up, ticks);
                    true
                } else {
                    false
                }
            })
            .await
            .map_err(internal)?;
            if forwarded {
                // Fast-poll the pane so the scrolled view shows immediately (reuses the typing
                // cadence path).
                ctx.input_seen
                    .lock()
                    .await
                    .insert(lane, std::time::Instant::now());
            }
            Ok(json!({ "forwarded": forwarded }))
        }
        // Arm/disarm auto-continue (resume on usage limit) for one lane, this session.
        "agent.auto_continue" => {
            let p: AgentAutoContinue = parse(params)?;
            {
                let mut off = ctx.auto_continue_off.lock().await;
                if p.enabled {
                    off.remove(&p.lane_id);
                } else {
                    off.insert(p.lane_id);
                    // Drop any active pause so the lane reverts to its natural status now.
                    ctx.rate_limits.lock().await.remove(&p.lane_id);
                }
            }
            ctx.broadcast(
                crate::pubsub::topic::AGENT_STATUS,
                json!({ "lane_id": p.lane_id, "status": "auto-continue" }),
            );
            Ok(Value::Null)
        }

        // ---- plain terminals (a shell per worktree, several allowed) ----
        "terminal.open" => {
            let p: LaneId = parse(params)?;
            let path = ctx.lanes.focus(p.lane_id).await.map_err(internal)?;
            let prefix = format!("term-{}-", p.lane_id);
            let tmux = ctx.tmux.clone();
            // Next free sequence for this lane's terminals.
            let existing = {
                let t = tmux.clone();
                tokio::task::spawn_blocking(move || t.list_windows().unwrap_or_default())
                    .await
                    .map_err(internal)?
            };
            let next = existing
                .iter()
                .filter_map(|w| w.strip_prefix(&prefix))
                .filter_map(|s| s.parse::<u32>().ok())
                .max()
                .unwrap_or(0)
                + 1;
            let name = format!("term-{}-{next}", p.lane_id);
            let target = {
                let name = name.clone();
                tokio::task::spawn_blocking(move || tmux.open_named(&name, &path))
                    .await
                    .map_err(internal)?
                    .map_err(internal)?
            };
            Ok(json!({ "id": name, "target": target }))
        }
        "terminal.list" => {
            let p: LaneId = parse(params)?;
            let prefix = format!("term-{}-", p.lane_id);
            let tmux = ctx.tmux.clone();
            let wins = tokio::task::spawn_blocking(move || tmux.list_windows().unwrap_or_default())
                .await
                .map_err(internal)?;
            let mut terms: Vec<String> = wins
                .into_iter()
                .filter(|w| w.starts_with(&prefix))
                .collect();
            terms.sort();
            to_value(terms)
        }
        "terminal.close" => {
            let p: TerminalId = parse(params)?;
            let tmux = ctx.tmux.clone();
            let id = p.id;
            let _ = tokio::task::spawn_blocking(move || tmux.kill_named(&id)).await;
            Ok(Value::Null)
        }
        "terminal.target" => {
            let p: TerminalId = parse(params)?;
            let tmux = ctx.tmux.clone();
            let id = p.id.clone();
            let available = tokio::task::spawn_blocking(move || tmux.has_named(&id))
                .await
                .map_err(internal)?;
            Ok(json!({ "target": ctx.tmux.target_named(&p.id), "available": available }))
        }

        // ---- interactive repo browser ----
        "fs.browse" => {
            let p: Browse = parse(params)?;
            let added: std::collections::HashSet<PathBuf> = ctx
                .registry
                .list()
                .await
                .map_err(internal)?
                .into_iter()
                .map(|r| r.path)
                .collect();
            let start = p.path.map(PathBuf::from);
            tokio::task::spawn_blocking(move || browse_dir(start, &added))
                .await
                .map_err(internal)
                .and_then(to_value)
        }

        // ---- subscription is handled in the socket layer ----
        "subscribe" => Ok(Value::Null),
        // Liveness probe for remote clients (the WS bridge) and a cheap connectivity check.
        "ping" => Ok(json!("pong")),
        // The conversation itself, for clients that render text natively (the mobile chat
        // view) instead of a desktop-width pane capture.
        "agent.transcript" => {
            let p: AgentTranscript = parse(params)?;
            let path = ctx.lanes.focus(p.lane_id).await.map_err(internal)?;
            let items = tokio::task::spawn_blocking(move || {
                let within = chrono::Duration::hours(SESSION_WINDOW_HOURS);
                let summaries = agent::claude::summaries_for(&path, within, MAX_SESSIONS_PER_LANE);
                let manifest = match &p.session_id {
                    Some(id) => summaries
                        .iter()
                        .find(|s| s.session_id.as_deref() == Some(id.as_str()))
                        .map(|s| s.manifest_path.clone()),
                    None => summaries.first().map(|s| s.manifest_path.clone()),
                };
                manifest
                    .map(|m| agent::claude::transcript_tail(&m, p.limit))
                    .unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            to_value(items)
        }
        // Push-notification device registration (the iOS companion).
        "push.register" => {
            let p: PushDevice = parse(params)?;
            ctx.store
                .register_device(p.device_token)
                .await
                .map_err(internal)?;
            Ok(Value::Null)
        }
        "push.unregister" => {
            let p: PushDevice = parse(params)?;
            ctx.store
                .unregister_device(p.device_token)
                .await
                .map_err(internal)?;
            Ok(Value::Null)
        }
        "viewport.set" => {
            let p: ViewportSet = parse(params)?;
            *ctx.viewport.lock().await = p.lane_ids;
            *ctx.viewport_focus.lock().await = p.focus_lane.zip(p.focus_window);
            Ok(Value::Null)
        }

        // ---- daemon ----
        "daemon.status" => {
            let repos = ctx.registry.list().await.map_err(internal)?.len();
            let lanes = ctx.lanes.list().await.map_err(internal)?.len();
            let db_size = ctx
                .db_path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .map(|m| m.len())
                .unwrap_or(0);
            Ok(json!({
                "uptime_secs": ctx.started.elapsed().as_secs(),
                "repos": repos,
                "lanes": lanes,
                "db_size_bytes": db_size,
                "version": repomon_core::version(),
            }))
        }
        "daemon.shutdown" => {
            ctx.request_shutdown();
            Ok(Value::Null)
        }

        // ---- usage ----
        // Per-account Claude usage scraped from `/usage` (empty unless [usage_probe] is on and a
        // TUI is attached). The TUI matches an entry's `key` to the focused agent's `config_dir`.
        "usage.get" => {
            let usage = ctx.usage.lock().await;
            let mut out: Vec<agent::AccountUsage> = usage
                .iter()
                .map(|(key, e)| agent::AccountUsage {
                    key: key.clone(),
                    label: e.label.clone(),
                    report: e.report.clone(),
                    age_secs: e.fetched_at.elapsed().as_secs(),
                })
                .collect();
            out.sort_by(|a, b| a.key.cmp(&b.key));
            to_value(out)
        }

        // Set/clear a user label for a session (keyed by transcript session_id; persisted).
        "session.rename" => {
            let p: SessionRename = parse(params)?;
            let label = p.label.map(|l| l.trim().to_string()).filter(|l| !l.is_empty());
            ctx.store
                .set_session_label(p.session_id, label)
                .await
                .map_err(internal)?;
            ctx.invalidate_overlay().await;
            ctx.broadcast(crate::pubsub::topic::AGENT_STATUS, json!({ "renamed": true }));
            Ok(Value::Null)
        }

        other => Err(RpcError::method_not_found(other)),
    }
}

/// Overlay live agent sessions onto lanes: rich status from the monitors (Claude transcript,
/// Aider history, …), falling back to "is the repomon-spawned tmux window alive?" for any
/// other kind. Reads run off the runtime thread.
/// How far back a transcript can have last changed and still count as a live session, and the
/// cap on how many concurrent sessions to surface per worktree.
const SESSION_WINDOW_HOURS: i64 = 6;
const MAX_SESSIONS_PER_LANE: usize = 8;
/// How recently a worktree's files must have changed to infer an *active* (but unidentified)
/// agent in it — the fallback that surfaces Claude Code worktree-isolated subagents, which leave
/// no transcript or process of their own. Short, so the indicator tracks actual work.
const ACTIVITY_WINDOW_SECS: i64 = 90;
/// Extra grace before an inferred (file-activity) session is dropped, so a brief lull between a
/// subagent's edits doesn't read as a finish and flap the session present→absent→present (which,
/// with subagent notifications on, would fire an Idle on each lull).
const INFERRED_GRACE_SECS: i64 = 30;

/// TTL for the cached lane overlay. Short enough that a freshly-spawned agent's window placeholder
/// (and exited-agent / rate-limit transitions) still surface within a refresh or two; long enough
/// that several clients polling ~1s apart share a single tmux/lsof/transcript scan.
const OVERLAY_TTL: std::time::Duration = std::time::Duration::from_millis(750);

/// The full lane list with live agent sessions overlaid — what `lane.list` serves — from a
/// short-TTL cache so a stream of per-second client polls collapses into ~1 scan per TTL. Stale
/// concurrent callers may each recompute (bounded, rare); we accept that over single-flight to
/// avoid a leader-failure deadlock. Structural changes call [`Ctx::invalidate_overlay`].
pub(crate) async fn lanes_with_agents(ctx: &Ctx) -> Result<Vec<Lane>, RpcError> {
    {
        let cache = ctx.overlay_cache.lock().await;
        if let Some((t, lanes)) = &*cache {
            if t.elapsed() < OVERLAY_TTL {
                return Ok(lanes.clone());
            }
        }
    }
    lanes_with_agents_fresh(ctx).await
}

/// Recompute the overlay from scratch and refresh the cache. Used by callers that must never read a
/// stale snapshot — notably `notify_watch`, whose edge detection would miss a transition if two
/// ticks reused the same cached list.
pub(crate) async fn lanes_with_agents_fresh(ctx: &Ctx) -> Result<Vec<Lane>, RpcError> {
    let mut lanes = ctx.lanes.list().await.map_err(internal)?;
    overlay_agents(ctx, &mut lanes).await;
    *ctx.overlay_cache.lock().await = Some((std::time::Instant::now(), lanes.clone()));
    Ok(lanes)
}

async fn overlay_agents(ctx: &Ctx, lanes: &mut [Lane]) {
    let paths: Vec<std::path::PathBuf> = lanes.iter().map(|l| l.worktree.path.clone()).collect();
    // All recently-active Claude sessions per worktree (one per transcript), so several
    // concurrent agents in one worktree each show up. Falls back to the generic monitor
    // (which also covers aider) when there's nothing recent from Claude.
    let scan_paths = paths.clone();
    let fresh_sessions: Result<Vec<Vec<_>>, String> = match tokio::task::spawn_blocking(move || {
        let within = chrono::Duration::hours(SESSION_WINDOW_HOURS);
        paths
            .iter()
            .map(|p| {
                // Catch a panic in one lane's transcript parse so it can't empty the whole batch
                // (the outer join would otherwise return `Err` and drop every lane's sessions).
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let recent = agent::claude::summaries_for(p, within, MAX_SESSIONS_PER_LANE);
                    if recent.is_empty() {
                        agent::summary_for(p).into_iter().collect()
                    } else {
                        recent
                    }
                }))
                .unwrap_or_default()
            })
            .collect::<Vec<Vec<_>>>()
    })
    .await
    {
        Ok(v) => Ok(v),
        Err(e) => Err(e.to_string()),
    };
    // On a scan-task failure, reuse the last-good per-worktree sessions rather than collapsing
    // every lane to empty (which detaches the TUI and fires stale notifications).
    let per_lane = {
        let mut last_good = ctx.last_good_sessions.lock().await;
        reuse_per_path_on_failure(fresh_sessions, &scan_paths, &mut last_good)
    };

    let metas = ctx.store.list_lane_meta().await.unwrap_or_default();
    // User-set session labels (keyed by transcript session_id), overlaid below.
    let labels = ctx.store.list_session_labels().await.unwrap_or_default();
    let tmux = ctx.tmux.clone();
    // Distinguish a *failed* probe from a genuinely empty server: on failure reuse the last-good
    // window set for this tick (a transient tmux fork/connection fault must not momentarily drop
    // every managed agent — that flips sessions to `external`, detaches the focused TUI, and fires
    // stale notifications). A real empty result still clears.
    let fresh: Result<Vec<String>, String> =
        match tokio::task::spawn_blocking(move || tmux.list_windows()).await {
            Ok(Ok(w)) => Ok(w),
            Ok(Err(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        };
    if let Err(ref e) = fresh {
        tracing::warn!("tmux list-windows failed; reusing last-good window set this overlay: {e}");
    }
    let windows = {
        let mut last_good = ctx.last_good_windows.lock().await;
        let mut empty_misses = ctx.window_empty_misses.lock().await;
        resolve_windows(fresh, &mut last_good, &mut empty_misses)
    };
    // If a managed (`lane-…`) window vanished since the last overlay — an agent `/exit`ed or was
    // stopped — the cached live-process count is now stale-high and would keep the dead session in
    // the lane's `×N` count for up to the cache TTL. Drop the cache so `live_cwds_cached` recomputes
    // fresh on the very next line, and the gone agent disappears within one refresh.
    let managed_now: std::collections::HashSet<String> = windows
        .iter()
        .filter(|w| w.starts_with("lane-"))
        .cloned()
        .collect();
    {
        let mut prev = ctx.last_managed_windows.lock().await;
        if prev.difference(&managed_now).next().is_some() {
            *ctx.live_cwds.lock().await = None;
            // Also drop the sticky-high counts so a `/exit`ed managed agent disappears within one
            // refresh instead of being held for the grace (tmux closes the window as its process
            // dies, so this is the genuine-exit signal — see `live_cwds_cached`).
            ctx.cwds_sticky.lock().await.clear();
        }
        *prev = managed_now;
    }
    // A `/exit`ed session leaves a recently-modified transcript behind but is no longer
    // running. claude's cwd is the worktree, so the number of live claude processes there
    // bounds how many sessions are actually running — keep that many of the most recent.
    let live = live_cwds_cached(ctx).await;

    // Usage-limit pauses (from the auto-continue watcher): when a managed lane is paused and
    // auto-continue is armed, its managed session shows as RateLimited with a resume time.
    let rate_limits = ctx.rate_limits.lock().await.clone();
    let auto_off = ctx.auto_continue_off.lock().await.clone();
    let global_auto = ctx.config.read().await.auto_continue;

    for (lane, mut summaries) in lanes.iter_mut().zip(per_lane) {
        // The lane's managed agent windows, in slot order (= spawn order). A window only exists
        // while its agent's process lives (tmux closes it on exit), so it doubles as proof of
        // liveness and as the routing target for keys/captures.
        let lane_windows = TmuxRuntime::lane_windows_in(&windows, lane.id);
        let managed_n = lane_windows.len();
        // Live `claude` processes whose cwd is this worktree bound how many of its sessions are
        // running (a `/exit`ed one leaves a recent transcript but no process). But never drop a
        // transcript that pairs to a live managed window — keep at least one per window — so a
        // freshly-spawned second agent isn't hidden for up to ~10s by the cached process count.
        let alive = live.as_ref().and_then(|m| {
            // A canonicalize failure (worktree path momentarily unreadable) must NOT degrade to a
            // key miss → count 0 → `truncate(0)` that drops the lane's sessions. Skip filtering
            // this tick instead (return None), like the probe-unavailable (`live == None`) path.
            let key = lane.worktree.path.canonicalize().ok()?;
            Some(m.get(&key).copied().unwrap_or(0))
        });
        if let Some(alive) = alive {
            summaries.truncate(alive.max(managed_n)); // sorted newest-first
        }
        if !summaries.is_empty() {
            // Pair the newest `k` transcripts with the `k` windows, oldest with oldest (slot order
            // tracks spawn order, transcripts arrive newest-first). A heuristic, but it routes
            // keys/captures to the right pane in practice.
            let paired = summaries.len().min(managed_n);
            for (idx, s) in summaries.into_iter().enumerate() {
                if s.last_activity > lane.last_activity_at {
                    lane.last_activity_at = s.last_activity;
                }
                let mut session = s.into_session(lane.repo.id, lane.worktree.id);
                session.custom_label = session
                    .session_id
                    .as_ref()
                    .and_then(|id| labels.get(id).cloned());
                if idx < paired {
                    session.external = false;
                    session.tmux_window = Some(lane_windows[paired - 1 - idx].clone());
                } else {
                    session.external = true;
                }
                lane.agent_sessions.push(session);
            }
            // A second agent spawned into this worktree gets its own window but hasn't written a
            // transcript yet (claude creates the .jsonl a beat after launch). Surface it right
            // away as a window-only placeholder so it isn't invisible until then. At most one
            // (the `SessKey::Fallback` model allows a single no-transcript session per lane): the
            // newest unpaired window, which is the slot the latest spawn took.
            if let Some(w) = placeholder_window_index(paired, managed_n) {
                let kind = lane_meta_kind(&metas, lane.id);
                lane.agent_sessions
                    .push(window_placeholder_session(lane, kind, lane_windows[w].clone()));
            }
        } else if managed_n > 0 {
            // No parseable transcript: surface a repomon-spawned agent if its window is alive.
            let kind = lane_meta_kind(&metas, lane.id);
            lane.agent_sessions
                .push(window_placeholder_session(lane, kind, lane_windows[0].clone()));
        } else if let Some(changed) = lane.state.last_change_at {
            // No identified agent, but a *non-main* worktree's files changed very recently — infer
            // an active agent we can't name (e.g. a Claude Code worktree-isolated subagent, which
            // runs inside its parent's process and leaves no transcript or process here). The main
            // checkout is excluded so hand-edits there don't masquerade as an agent.
            let active = !lane.worktree.is_main
                && (chrono::Utc::now() - changed).num_seconds()
                    < ACTIVITY_WINDOW_SECS + INFERRED_GRACE_SECS;
            if active {
                if changed > lane.last_activity_at {
                    lane.last_activity_at = changed;
                }
                lane.agent_sessions.push(AgentSession {
                    id: 0,
                    agent: AgentKind::Other("active".into()),
                    repo_id: lane.repo.id,
                    worktree_id: Some(lane.worktree.id),
                    started_at: changed,
                    last_activity_at: changed,
                    ended_at: None,
                    manifest_path: std::path::PathBuf::new(),
                    tool_call_count: 0,
                    title: Some("active — file activity".into()),
                    status: AgentStatus::Running,
                    external: true,
                    session_id: None,
                    resume_at: None,
                    inferred: true,
                    tmux_window: None,
                    last_message: None,
                    pending_prompt: None,
                    config_dir: None,
                    custom_label: None,
                });
            }
        }

        // Overlay a usage-limit pause onto the managed (non-external) session.
        if let Some(rl) = rate_limits.get(&lane.id) {
            let armed = global_auto && !auto_off.contains(&lane.id);
            if armed {
                if let Some(sess) = lane.agent_sessions.iter_mut().find(|s| !s.external) {
                    sess.status = AgentStatus::RateLimited;
                    sess.resume_at = rl.reset_at;
                }
            }
        }
    }

    // Interactive dialogs: a transcript that ends in a tool call reads **Running**, but the
    // pane may be sitting on a permission "Do you want…?" dialog; a turn ending in text reads
    // **Waiting**, but the pane may be showing an option menu (plan approval, a question with
    // choices). Neither is in the JSONL. Sniff the panes of managed Running/Waiting sessions:
    // a detected dialog sets `pending_prompt` (clients gate approve/menu controls on it),
    // becomes the notification-ready "why", and flips Running → Waiting.
    let candidates: Vec<(usize, usize, String, AgentStatus)> = lanes
        .iter()
        .enumerate()
        .flat_map(|(li, lane)| {
            lane.agent_sessions
                .iter()
                .enumerate()
                .filter_map(move |(si, s)| {
                    let sniffable = !s.external
                        && !s.inferred
                        && matches!(s.status, AgentStatus::Running | AgentStatus::Waiting);
                    sniffable
                        .then(|| s.tmux_window.clone().map(|w| (li, si, w, s.status)))
                        .flatten()
                })
        })
        .collect();
    if !candidates.is_empty() {
        // The sniff is a `capture-pane` per Running/Waiting session — the bulk of the overlay's
        // subprocess cost. Reuse a recent result per window and only re-capture stale ones, so
        // rapid overlays (notify_watch + client polls) share one sniff per window per TTL.
        const SNIFF_TTL: std::time::Duration = std::time::Duration::from_secs(20);
        // A Running session is the one that can *newly* raise a dialog (its transcript ends in a
        // tool call, but the pane may be on a permission/plan/menu prompt that only the sniff
        // sees), so a NeedsYou can be up to SNIFF_TTL late. Re-capture those on a much shorter TTL
        // to cut that latency; a session already classified Waiting has its dialog confirmed, so
        // let its result ride the full TTL. The extra captures are bounded — only while a session
        // is actively Running — and the notification engine's activity latch absorbs any added
        // flap from sniffing more often.
        const RUNNING_SNIFF_TTL: std::time::Duration = std::time::Duration::from_secs(5);
        let mut prompts: Vec<Option<String>> = Vec::with_capacity(candidates.len());
        let mut misses: Vec<usize> = Vec::new();
        {
            let cache = ctx.prompt_cache.lock().await;
            for (idx, (_, _, w, status)) in candidates.iter().enumerate() {
                let ttl = if *status == AgentStatus::Running {
                    RUNNING_SNIFF_TTL
                } else {
                    SNIFF_TTL
                };
                match cache.get(w) {
                    Some((t, p)) if t.elapsed() < ttl => prompts.push(p.clone()),
                    _ => {
                        prompts.push(None);
                        misses.push(idx);
                    }
                }
            }
        }
        if !misses.is_empty() {
            let tmux = ctx.tmux.clone();
            let windows: Vec<String> = misses.iter().map(|&i| candidates[i].2.clone()).collect();
            let fresh = tokio::task::spawn_blocking(move || {
                windows
                    .iter()
                    .map(|w| {
                        tmux.capture_named(w, Some(45))
                            .ok()
                            .and_then(|p| agent::prompt::detect_pending_prompt(&p))
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            let mut cache = ctx.prompt_cache.lock().await;
            for (&i, p) in misses.iter().zip(fresh) {
                cache.insert(candidates[i].2.clone(), (std::time::Instant::now(), p.clone()));
                prompts[i] = p;
            }
        }
        // Prune the sniff cache so it can't grow without bound — every window name ever sniffed
        // would otherwise leak an entry. Drop any window no longer present in the current tmux set,
        // and any whose result is older than the longest sniff TTL (it would be re-captured anyway).
        {
            let live: std::collections::HashSet<&str> = windows.iter().map(String::as_str).collect();
            let mut cache = ctx.prompt_cache.lock().await;
            cache.retain(|w, (t, _)| live.contains(w.as_str()) && t.elapsed() < SNIFF_TTL);
        }
        for ((li, si, _, _), found) in candidates.into_iter().zip(prompts) {
            if let Some(summary) = found {
                let s = &mut lanes[li].agent_sessions[si];
                s.status = AgentStatus::Waiting;
                s.last_message = Some(summary.clone());
                s.pending_prompt = Some(summary);
            }
        }
    }
}

/// List the subdirectories of `start` (default: $HOME) for the repo browser, marking which
/// are git repos and which are already registered.
fn browse_dir(start: Option<PathBuf>, added: &std::collections::HashSet<PathBuf>) -> BrowseResult {
    let path = start
        .filter(|p| p.is_dir())
        .or_else(|| std::env::var("HOME").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/"));
    let path = path.canonicalize().unwrap_or(path);
    let parent = path.parent().map(Path::to_path_buf);

    let mut entries = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&path) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let name = match p.file_name().and_then(|s| s.to_str()) {
                Some(n) if !n.starts_with('.') => n.to_string(),
                _ => continue, // skip hidden / non-utf8
            };
            let canon = p.canonicalize().unwrap_or_else(|_| p.clone());
            let is_repo = p.join(".git").exists();
            let is_added = added.contains(&canon) || added.contains(&p);
            entries.push(BrowseEntry {
                name,
                path: p,
                is_repo,
                added: is_added,
            });
        }
    }
    // Repos first, then plain dirs; alphabetical within each.
    entries.sort_by(|a, b| {
        b.is_repo
            .cmp(&a.is_repo)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    BrowseResult {
        path,
        parent,
        entries,
    }
}

/// Built-in agent kinds with a fixed binary name. Claude is handled separately (one variant
/// per detected config dir). These names can't be used for a custom agent.
const BUILTIN_AGENTS: [&str; 2] = ["codex", "aider"];

/// A name is reserved (can't be added/removed as a custom) if it's a fixed built-in or one of
/// the autodetected Claude variants (claude-code, claude-work, …).
fn is_builtin(name: &str) -> bool {
    BUILTIN_AGENTS.contains(&name)
        || agent::claude::agent_variants()
            .iter()
            .any(|(n, _)| n == name)
}

/// The base command to resume an adopted Claude session, matching the *account* (config dir)
/// the session belongs to — and reusing the user's configured agent for that account so any
/// flags they set (e.g. `--dangerously-skip-permissions`) carry over. Falls back to a bare
/// `[CLAUDE_CONFIG_DIR=…] claude`.
fn adopt_base_command(
    default_agent: &Option<String>,
    customs: &HashMap<String, String>,
    config_dir: &Option<PathBuf>,
) -> String {
    let want = config_dir
        .as_ref()
        .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()));
    // Prefer the configured default agent, then autodetected variants, then customs.
    let mut candidates: Vec<String> = Vec::new();
    if let Some(name) = default_agent {
        if let Some(c) = customs.get(name) {
            candidates.push(c.clone());
        } else if let Some((_, c)) = agent::claude::agent_variants()
            .into_iter()
            .find(|(n, _)| n == name)
        {
            candidates.push(c);
        }
    }
    candidates.extend(agent::claude::agent_variants().into_iter().map(|(_, c)| c));
    candidates.extend(customs.values().cloned());

    pick_for_account(&candidates, &want).unwrap_or_else(|| match config_dir {
        Some(d) => format!("CLAUDE_CONFIG_DIR={} claude", d.display()),
        None => "claude".to_string(),
    })
}

/// The account (CLAUDE_CONFIG_DIR, canonicalized) a command targets, or `None` for the default.
fn command_account(cmd: &str) -> Option<PathBuf> {
    cmd.split_whitespace()
        .find_map(|t| t.strip_prefix("CLAUDE_CONFIG_DIR=").map(PathBuf::from))
        .map(|p| p.canonicalize().unwrap_or(p))
}

/// The first claude command from `candidates` whose account matches `want`.
fn pick_for_account(candidates: &[String], want: &Option<PathBuf>) -> Option<String> {
    candidates
        .iter()
        .find(|c| program_of(c) == Some("claude") && &command_account(c) == want)
        .cloned()
}

/// Does a token look like a leading `VAR=value` env assignment (e.g. `CLAUDE_CONFIG_DIR=…`)?
fn is_env_assignment(tok: &str) -> bool {
    match tok.split_once('=') {
        Some((k, _)) => !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        None => false,
    }
}

/// The program a command runs, skipping leading env assignments so commands like
/// `CLAUDE_CONFIG_DIR=~/.claude-work claude` resolve to `claude`.
fn program_of(command: &str) -> Option<&str> {
    command.split_whitespace().find(|t| !is_env_assignment(t))
}

/// The agent kind repomon last spawned in a lane (from its persisted meta), defaulting to Claude
/// when nothing was recorded — used to label a window-only placeholder session.
fn lane_meta_kind(
    metas: &[repomon_core::model::LaneMeta],
    lane_id: repomon_core::model::LaneId,
) -> AgentKind {
    metas
        .iter()
        .find(|m| m.id == lane_id)
        .and_then(|m| m.agent_kind.clone())
        .map(|k| AgentKind::from_kind_str(&k))
        .unwrap_or(AgentKind::ClaudeCode)
}

/// A window-only placeholder agent: a repomon-spawned session whose tmux window is alive but
/// whose transcript hasn't appeared yet (just launched), so it shows immediately instead of
/// staying invisible until the `.jsonl` lands. Managed (`external: false`), no transcript id,
/// and not `inferred` (it's a real spawn, not a guess from file activity).
fn window_placeholder_session(lane: &Lane, kind: AgentKind, window: String) -> AgentSession {
    AgentSession {
        id: 0,
        agent: kind,
        repo_id: lane.repo.id,
        worktree_id: Some(lane.worktree.id),
        started_at: lane.last_activity_at,
        last_activity_at: lane.last_activity_at,
        ended_at: None,
        manifest_path: std::path::PathBuf::new(),
        tool_call_count: 0,
        title: None,
        status: AgentStatus::Running,
        external: false,
        session_id: None,
        resume_at: None,
        inferred: false,
        tmux_window: Some(window),
        last_message: None,
        pending_prompt: None,
        config_dir: None,
        custom_label: None,
    }
}

/// Whether a lane needs a window-only placeholder for a just-spawned agent whose transcript
/// hasn't appeared yet, and which managed-window index it maps to. `shown` = transcript-backed
/// sessions already emitted; `managed_n` = live managed windows. A managed window exists only
/// while its agent's process lives (tmux closes it on exit), so an unpaired window is a real
/// agent that simply hasn't written its `.jsonl` yet. Returns the newest unpaired window's index
/// (at most one, per the `SessKey::Fallback` single-no-transcript-session model), or `None`.
fn placeholder_window_index(shown: usize, managed_n: usize) -> Option<usize> {
    (managed_n > shown).then(|| managed_n - 1)
}

/// A Claude session id is safe to interpolate into a resume command (`claude --resume <id>`).
/// Transcript ids are UUIDs / `[A-Za-z0-9_-]`; anything else (whitespace, `;`, `$`, quotes, `|`,
/// backticks…) is rejected so `agent.adopt` can't be turned into shell injection — the command is
/// ultimately run via `sh -c` by tmux. Empty is invalid.
fn valid_session_id(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Pick the tmux window list for this overlay tick. On a successful probe, return the fresh list
/// and remember it as last-good. On a probe *failure* (a transient fork/connection fault, e.g.
/// `tmux` failing to spawn under load — distinct from a genuinely empty server), reuse the
/// last-good list so a single bad snapshot doesn't momentarily drop every managed agent — which
/// would flip sessions to `external`, detach the focused TUI, and fire stale notifications.
fn resolve_windows(
    fresh: Result<Vec<String>, String>,
    last_good: &mut Vec<String>,
    empty_misses: &mut u8,
) -> Vec<String> {
    match fresh {
        // Transient probe fault (fork/connection): reuse last-good; don't touch the empty counter.
        Err(_) => last_good.clone(),
        // A sudden total vanish of every window is usually a tmux server bounce (e.g. the user ran
        // `tmux kill-server`), not all agents exiting at once. Treat the first empties as a blip —
        // reuse last-good — and accept the empty only after EMPTY_WINDOWS_CONFIRM in a row, so a
        // bounce doesn't drop every managed session for a tick (which detaches the TUI and fires a
        // wave of stale Idle notifications).
        Ok(w) if w.is_empty() && !last_good.is_empty() => {
            *empty_misses = empty_misses.saturating_add(1);
            if *empty_misses >= EMPTY_WINDOWS_CONFIRM {
                last_good.clear();
                Vec::new()
            } else {
                last_good.clone()
            }
        }
        Ok(w) => {
            *empty_misses = 0;
            *last_good = w.clone();
            w
        }
    }
}

/// Consecutive empty `list_windows` results before we believe the tmux server genuinely has no
/// windows (vs. a transient bounce).
const EMPTY_WINDOWS_CONFIRM: u8 = 2;

/// Per-path analogue of [`resolve_windows`] for the transcript scan: on success, remember each
/// path's result as last-good; on a scan-task failure (a join error / panic that escaped the
/// per-lane `catch_unwind`), reuse the last-good per path so the whole fleet doesn't collapse to
/// empty for that tick. Unknown paths fall back to empty.
fn reuse_per_path_on_failure<T: Clone>(
    fresh: Result<Vec<Vec<T>>, String>,
    paths: &[std::path::PathBuf],
    last_good: &mut HashMap<std::path::PathBuf, Vec<T>>,
) -> Vec<Vec<T>> {
    match fresh {
        Ok(per_lane) => {
            for (p, v) in paths.iter().zip(&per_lane) {
                last_good.insert(p.clone(), v.clone());
            }
            per_lane
        }
        Err(_) => paths
            .iter()
            .map(|p| last_good.get(p).cloned().unwrap_or_default())
            .collect(),
    }
}

/// How many live `claude` CLI processes have each working directory. claude doesn't hold its
/// transcript open, but its cwd is the worktree it runs in — so the count per worktree bounds
/// how many of that worktree's sessions are actually running. `None` if the probe couldn't
/// run (then we don't filter); `Some({})` means no claude is running.
fn live_claude_cwds() -> Option<HashMap<PathBuf, usize>> {
    use std::process::Command;
    let pgrep = Command::new("pgrep").args(["-x", "claude"]).output().ok()?;
    // pgrep exits 1 when there are no matches — that's a clean "none", not a failure.
    let pids: Vec<&str> = std::str::from_utf8(&pgrep.stdout)
        .ok()?
        .split_whitespace()
        .collect();
    let mut counts: HashMap<PathBuf, usize> = HashMap::new();
    if pids.is_empty() {
        return Some(counts);
    }
    // One lsof call listing just each process's cwd (one `n<path>` line per process).
    let lsof = Command::new("lsof")
        .args(["-a", "-d", "cwd", "-Fn", "-p"])
        .arg(pids.join(","))
        .output()
        .ok()?;
    for line in std::str::from_utf8(&lsof.stdout).unwrap_or("").lines() {
        if let Some(name) = line.strip_prefix('n') {
            let p = PathBuf::from(name);
            let key = p.canonicalize().unwrap_or(p);
            *counts.entry(key).or_insert(0) += 1;
        }
    }
    Some(counts)
}

/// Cached [`live_claude_cwds`] with a 10s TTL (plus a 30s sticky-high grace against undercounts),
/// so frequent `lane.list` calls don't hammer `lsof`.
async fn live_cwds_cached(ctx: &Ctx) -> Option<HashMap<PathBuf, usize>> {
    {
        let cache = ctx.live_cwds.lock().await;
        if let Some((t, map)) = &*cache {
            // pgrep+lsof is slow (lsof spikes to 100-500ms on macOS); keep it well off the hot
            // path. A `/exit`-ed session may linger up to this long — acceptable.
            if t.elapsed() < std::time::Duration::from_secs(10) {
                return Some(map.clone());
            }
        }
    }
    let map = match tokio::task::spawn_blocking(live_claude_cwds).await.ok().flatten() {
        Some(m) => m,
        None => {
            // The probe couldn't run (pgrep/lsof spawn failed, e.g. under load). Returning None
            // means "don't filter" — callers keep all recent sessions rather than truncating to a
            // bogus low count — but it was silent; log it so a recurring flap is visible.
            tracing::warn!("live claude-process probe (pgrep/lsof) failed; not truncating sessions");
            return None;
        }
    };
    // Sticky-high: a single `pgrep`/`lsof` undercount must not drop a session from the overlay
    // (then re-add it next probe), which churns the lane list and used to re-fire alerts. Hold each
    // worktree's highest recently-observed count for a short grace, so one bad sample can't hide a
    // session; a genuine count drop decays after the grace. Managed exits stay prompt because the
    // managed-window-vanish path clears this map (and tmux closes the window the moment the process
    // dies), so this lingering only ever affects external sessions — acceptable, like the cache TTL.
    const STICKY_GRACE: std::time::Duration = std::time::Duration::from_secs(30);
    let now = std::time::Instant::now();
    let mut effective = map.clone();
    {
        let mut sticky = ctx.cwds_sticky.lock().await;
        // Refresh a worktree's held high only when this sample meets or exceeds it — an under-read
        // leaves the high's timestamp untouched so it can age out (real exits eventually decay).
        for (k, &c) in &map {
            let refresh = sticky.get(k).map(|(hi, _)| c >= *hi).unwrap_or(true);
            if refresh {
                sticky.insert(k.clone(), (c, now));
            }
        }
        sticky.retain(|_, (_, seen)| seen.elapsed() < STICKY_GRACE);
        // Lift the fresh count to the surviving held high (covers worktrees missing from `map`).
        for (k, (hi, _)) in sticky.iter() {
            let e = effective.entry(k.clone()).or_insert(0);
            *e = (*e).max(*hi);
        }
    }
    *ctx.live_cwds.lock().await = Some((now, effective.clone()));
    Some(effective)
}

/// Is the command's program on PATH (or an absolute/relative path that exists)?
fn on_path(command: &str) -> bool {
    let prog = match program_of(command) {
        Some(p) => p,
        None => return false,
    };
    if prog.contains('/') {
        return Path::new(prog).exists();
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(prog).is_file()))
        .unwrap_or(false)
}

async fn repo_names(ctx: &Ctx) -> HashMap<RepoId, String> {
    ctx.registry
        .list()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| (r.id, r.name))
        .collect()
}

fn parse_iso(s: &str) -> Result<chrono::DateTime<chrono::Utc>, RpcError> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&chrono::Utc))
        .map_err(|e| RpcError::invalid_params(format!("bad timestamp {s:?}: {e}")))
}

/// The current local day, in UTC: [local midnight, next local midnight). Using the next
/// midnight as the exclusive end (rather than `now`) avoids dropping a commit made in the
/// same whole second as the query.
fn today_range() -> TimeRange {
    use chrono::{Local, TimeZone, Utc};
    let now_local = Local::now();
    let midnight_naive = now_local.date_naive().and_hms_opt(0, 0, 0).unwrap();
    let from = Local
        .from_local_datetime(&midnight_naive)
        .single()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    TimeRange {
        from,
        to: from + chrono::Duration::days(1),
    }
}

/// Aggregate commits across all (or selected) repos for `range`, newest first.
async fn commits_in_range(
    ctx: &Ctx,
    range: TimeRange,
    repo_ids: Option<Vec<RepoId>>,
) -> Result<Vec<Commit>, RpcError> {
    let repos = ctx.registry.list().await.map_err(internal)?;
    let mut out: Vec<Commit> = Vec::new();
    for repo in repos {
        if let Some(ids) = &repo_ids {
            if !ids.contains(&repo.id) {
                continue;
            }
        }
        let path: PathBuf = repo.path.clone();
        let id = repo.id;
        let commits =
            tokio::task::spawn_blocking(move || reader::read_commits_in_range(&path, id, range))
                .await
                .map_err(internal)?
                .unwrap_or_default();
        out.extend(commits);
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.time));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_validation_blocks_injection() {
        // Real transcript ids pass.
        assert!(valid_session_id("44ba81d8-be2c-4f0b-b9b3-c228fa53cc79"));
        assert!(valid_session_id("abc_123-DEF"));
        // Anything that could break out of `claude --resume <id>` under `sh -c` is rejected.
        assert!(!valid_session_id("")); // empty
        assert!(!valid_session_id("x; touch /tmp/pwned"));
        assert!(!valid_session_id("$(id)"));
        assert!(!valid_session_id("a`whoami`"));
        assert!(!valid_session_id("a b")); // whitespace
        assert!(!valid_session_id("a|b"));
        assert!(!valid_session_id("../../etc"));
    }

    #[test]
    fn resolve_windows_reuses_last_good_only_on_probe_failure() {
        let mut last: Vec<String> = vec![];
        let mut misses = 0u8;
        // A successful probe is returned verbatim and remembered as last-good.
        assert_eq!(
            resolve_windows(Ok(vec!["lane-1".into(), "lane-2".into()]), &mut last, &mut misses),
            vec!["lane-1", "lane-2"]
        );
        assert_eq!(last, vec!["lane-1", "lane-2"]);
        // A probe FAILURE reuses last-good instead of collapsing to empty (no spurious drop).
        assert_eq!(
            resolve_windows(Err("tmux spawn failed".into()), &mut last, &mut misses),
            vec!["lane-1", "lane-2"]
        );
        assert_eq!(last, vec!["lane-1", "lane-2"]); // unchanged by failure
    }

    #[test]
    fn reuse_per_path_on_failure_keeps_last_good_per_path() {
        use std::path::PathBuf;
        let (a, b) = (PathBuf::from("/a"), PathBuf::from("/b"));
        let paths = vec![a.clone(), b.clone()];
        let mut lg: HashMap<PathBuf, Vec<i32>> = HashMap::new();
        // Success caches each path's result and returns it verbatim.
        assert_eq!(
            reuse_per_path_on_failure(Ok(vec![vec![1, 2], vec![3]]), &paths, &mut lg),
            vec![vec![1, 2], vec![3]]
        );
        assert_eq!(lg.get(&a), Some(&vec![1, 2]));
        // A scan-task failure reuses the cached per-path results instead of collapsing to empty.
        assert_eq!(
            reuse_per_path_on_failure(Err("scan panicked".into()), &paths, &mut lg),
            vec![vec![1, 2], vec![3]]
        );
        // A path with no cached value falls back to empty (not a panic).
        assert_eq!(
            reuse_per_path_on_failure::<i32>(Err("x".into()), &[PathBuf::from("/c")], &mut lg),
            vec![Vec::<i32>::new()]
        );
    }

    #[test]
    fn resolve_windows_rides_out_a_one_tick_total_vanish() {
        // last-good is non-empty; a single empty probe is treated as a likely tmux server bounce.
        let mut last: Vec<String> = vec!["lane-1".into()];
        let mut misses = 0u8;
        // First empty: reuse last-good (don't drop everyone for a blip).
        assert_eq!(
            resolve_windows(Ok(vec![]), &mut last, &mut misses),
            vec!["lane-1"]
        );
        assert_eq!(misses, 1);
        // Sustained empty (EMPTY_WINDOWS_CONFIRM in a row): accept it — agents really are gone.
        assert_eq!(resolve_windows(Ok(vec![]), &mut last, &mut misses), Vec::<String>::new());
        assert!(last.is_empty());
        // A subsequent successful probe resets the counter.
        resolve_windows(Ok(vec!["lane-9".into()]), &mut last, &mut misses);
        assert_eq!(misses, 0);
    }

    #[test]
    fn placeholder_for_the_newest_unpaired_window() {
        // One existing agent (transcript) + a freshly-spawned second window: surface the new
        // window immediately, mapped to the newest slot, so it isn't invisible until its
        // transcript lands.
        assert_eq!(placeholder_window_index(1, 2), Some(1));
        // Every managed window already transcript-backed → no placeholder.
        assert_eq!(placeholder_window_index(2, 2), None);
        assert_eq!(placeholder_window_index(1, 1), None);
        assert_eq!(placeholder_window_index(0, 0), None);
        // Three windows, one transcript: still a single placeholder (the Fallback invariant),
        // mapped to the newest window.
        assert_eq!(placeholder_window_index(1, 3), Some(2));
    }

    #[test]
    fn program_of_skips_env_assignments() {
        assert_eq!(program_of("claude"), Some("claude"));
        // A work-account command resolves to the claude binary, not the env var.
        assert_eq!(
            program_of("CLAUDE_CONFIG_DIR=/Users/x/.claude-work claude"),
            Some("claude")
        );
        assert_eq!(program_of("FOO=1 BAR=2 aider --model x"), Some("aider"));
        assert_eq!(program_of(""), None);
        assert!(is_env_assignment("CLAUDE_CONFIG_DIR=/x/.claude-work"));
        assert!(!is_env_assignment("claude"));
        assert!(!is_env_assignment("--model=opus")); // a flag, not an env assignment
    }

    #[test]
    fn adopt_picks_command_matching_the_account() {
        let candidates = vec![
            "claude".to_string(),                                         // default account
            "CLAUDE_CONFIG_DIR=/h/.claude-work claude --foo".to_string(), // work account + flag
            "aider".to_string(),                                          // not claude
        ];
        let work = PathBuf::from("/h/.claude-work");
        let want = Some(work.canonicalize().unwrap_or(work));
        // The work-account session resumes with the work command — flag carried over.
        assert_eq!(
            pick_for_account(&candidates, &want),
            Some("CLAUDE_CONFIG_DIR=/h/.claude-work claude --foo".to_string())
        );
        // A default-account session resumes with bare claude.
        assert_eq!(
            pick_for_account(&candidates, &None),
            Some("claude".to_string())
        );
        // Non-claude commands are never chosen (can't --resume).
        assert_eq!(pick_for_account(&["aider".to_string()], &None), None);
        assert_eq!(
            command_account("CLAUDE_CONFIG_DIR=/x claude"),
            Some(PathBuf::from("/x"))
        );
        assert_eq!(command_account("claude"), None);
    }

    #[test]
    fn builtins_are_recognized() {
        // claude-code is always present (the default config dir is always listed).
        assert!(is_builtin("claude-code"));
        assert!(is_builtin("codex"));
        assert!(!is_builtin("claude-yolo"));
    }
}
