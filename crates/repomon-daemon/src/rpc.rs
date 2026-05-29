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
}
#[derive(Deserialize)]
struct AgentSignal {
    lane_id: repomon_core::model::LaneId,
    key: String,
}
#[derive(Deserialize)]
struct AgentKey {
    lane_id: repomon_core::model::LaneId,
    key: String,
    #[serde(default)]
    literal: bool,
}
#[derive(Deserialize)]
struct AgentCapture {
    lane_id: repomon_core::model::LaneId,
    #[serde(default)]
    lines: Option<u32>,
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
struct ViewportSet {
    lane_ids: Vec<repomon_core::model::LaneId>,
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
        // ---- repos ----
        "repo.list" => to_value(ctx.registry.list().await.map_err(internal)?),
        "repo.add" => {
            let p: RepoAdd = parse(params)?;
            let repo = ctx
                .registry
                .add(std::path::Path::new(&p.path))
                .await
                .map_err(internal)?;
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
        "lane.list" => {
            let mut lanes = ctx.lanes.list().await.map_err(internal)?;
            overlay_agents(ctx, &mut lanes).await;
            to_value(lanes)
        }
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
            let detect = path.clone();
            let session_id = p.session_id.clone();
            let command = tokio::task::spawn_blocking(move || {
                let (config_dir, tail) = match session_id {
                    Some(sid) => {
                        let cfg = agent::claude::config_base_for_session(&detect, &sid).flatten();
                        (cfg, format!("claude --resume {sid}"))
                    }
                    None => {
                        let cfg = agent::claude::summary_for(&detect).and_then(|s| s.config_dir);
                        (cfg, "claude --continue".to_string())
                    }
                };
                match config_dir {
                    Some(dir) => format!("CLAUDE_CONFIG_DIR={} {tail}", dir.display()),
                    None => tail,
                }
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
            Ok(json!({ "lane_id": p.lane_id, "window": window }))
        }
        "agent.capture" => {
            let p: AgentCapture = parse(params)?;
            let tmux = ctx.tmux.clone();
            let (lane, lines) = (p.lane_id, p.lines);
            let content = tokio::task::spawn_blocking(move || tmux.capture(lane, lines))
                .await
                .map_err(internal)?
                .map_err(internal)?;
            Ok(json!({ "content": content }))
        }
        "agent.send_input" => {
            let p: AgentInput = parse(params)?;
            let tmux = ctx.tmux.clone();
            let (lane, text) = (p.lane_id, p.text);
            tokio::task::spawn_blocking(move || tmux.send_text(lane, &text))
                .await
                .map_err(internal)?
                .map_err(internal)?;
            Ok(Value::Null)
        }
        "agent.signal" => {
            let p: AgentSignal = parse(params)?;
            let tmux = ctx.tmux.clone();
            let (lane, key) = (p.lane_id, p.key);
            tokio::task::spawn_blocking(move || tmux.send_key(lane, &key))
                .await
                .map_err(internal)?
                .map_err(internal)?;
            Ok(Value::Null)
        }
        "agent.key" => {
            let p: AgentKey = parse(params)?;
            let tmux = ctx.tmux.clone();
            let (lane, key, literal) = (p.lane_id, p.key, p.literal);
            tokio::task::spawn_blocking(move || {
                if literal {
                    tmux.send_literal(lane, &key)
                } else {
                    tmux.send_key(lane, &key)
                }
            })
            .await
            .map_err(internal)?
            .map_err(internal)?;
            Ok(Value::Null)
        }
        "agent.stop" => {
            let p: LaneId = parse(params)?;
            let tmux = ctx.tmux.clone();
            let lane = p.lane_id;
            let _ = tokio::task::spawn_blocking(move || tmux.kill(lane)).await;
            let _ = ctx.store.set_lane_tmux_window(p.lane_id, None).await;
            ctx.broadcast(
                crate::pubsub::topic::AGENT_STATUS,
                json!({ "lane_id": p.lane_id, "status": "ended" }),
            );
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
            let p: LaneId = parse(params)?;
            let tmux = ctx.tmux.clone();
            let lane = p.lane_id;
            let available = tokio::task::spawn_blocking(move || tmux.has_window(lane))
                .await
                .map_err(internal)?;
            Ok(json!({ "target": ctx.tmux.target(p.lane_id), "available": available }))
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
        "viewport.set" => {
            let p: ViewportSet = parse(params)?;
            *ctx.viewport.lock().await = p.lane_ids;
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

async fn overlay_agents(ctx: &Ctx, lanes: &mut [Lane]) {
    let paths: Vec<std::path::PathBuf> = lanes.iter().map(|l| l.worktree.path.clone()).collect();
    // All recently-active Claude sessions per worktree (one per transcript), so several
    // concurrent agents in one worktree each show up. Falls back to the generic monitor
    // (which also covers aider) when there's nothing recent from Claude.
    let per_lane = tokio::task::spawn_blocking(move || {
        let within = chrono::Duration::hours(SESSION_WINDOW_HOURS);
        paths
            .iter()
            .map(|p| {
                let recent = agent::claude::summaries_for(p, within, MAX_SESSIONS_PER_LANE);
                if recent.is_empty() {
                    agent::summary_for(p).into_iter().collect()
                } else {
                    recent
                }
            })
            .collect::<Vec<Vec<_>>>()
    })
    .await
    .unwrap_or_default();

    let metas = ctx.store.list_lane_meta().await.unwrap_or_default();
    let tmux = ctx.tmux.clone();
    let windows = tokio::task::spawn_blocking(move || tmux.list_windows().unwrap_or_default())
        .await
        .unwrap_or_default();
    // Only count Claude sessions whose process is still alive — a `/exit`ed session leaves a
    // recently-modified transcript behind but is no longer running.
    let live = live_sessions_cached(ctx).await;

    for (lane, mut summaries) in lanes.iter_mut().zip(per_lane) {
        if let Some(live) = &live {
            summaries.retain(|s| s.session_id.as_ref().is_none_or(|id| live.contains(id)));
        }
        let managed = windows.contains(&TmuxRuntime::window_name(lane.id));
        if !summaries.is_empty() {
            for (idx, s) in summaries.into_iter().enumerate() {
                if s.last_activity > lane.last_activity_at {
                    lane.last_activity_at = s.last_activity;
                }
                // repomon manages at most one session per worktree (its single tmux window);
                // assume that's the most-recent one. Every other session is running in another
                // terminal, so it's external and adoptable.
                let mut session = s.into_session(lane.repo.id, lane.worktree.id);
                session.external = !(managed && idx == 0);
                lane.agent_sessions.push(session);
            }
            continue;
        }
        // No parseable transcript: surface a repomon-spawned agent if its window is alive.
        if managed {
            let kind = metas
                .iter()
                .find(|m| m.id == lane.id)
                .and_then(|m| m.agent_kind.clone())
                .map(|k| AgentKind::from_kind_str(&k))
                .unwrap_or(AgentKind::ClaudeCode);
            lane.agent_sessions.push(AgentSession {
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
            });
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

/// Session ids of currently-running `claude` CLI processes — each keeps its transcript file
/// open, so `lsof` reveals which `<session-id>.jsonl` it's writing. `None` means the probe
/// couldn't run (then we don't filter); `Some({})` means no claude is running.
fn live_claude_sessions() -> Option<std::collections::HashSet<String>> {
    use std::process::Command;
    let pgrep = Command::new("pgrep").args(["-x", "claude"]).output().ok()?;
    // pgrep exits 1 when there are no matches — that's a clean "none", not a failure.
    let pids: Vec<&str> = std::str::from_utf8(&pgrep.stdout)
        .ok()?
        .split_whitespace()
        .collect();
    if pids.is_empty() {
        return Some(std::collections::HashSet::new());
    }
    let lsof = Command::new("lsof")
        .arg("-p")
        .arg(pids.join(","))
        .arg("-Fn")
        .output()
        .ok()?;
    let mut ids = std::collections::HashSet::new();
    for line in std::str::from_utf8(&lsof.stdout).unwrap_or("").lines() {
        if let Some(name) = line.strip_prefix('n') {
            if name.ends_with(".jsonl") && name.contains("/projects/") {
                if let Some(stem) = Path::new(name).file_stem().and_then(|s| s.to_str()) {
                    ids.insert(stem.to_string());
                }
            }
        }
    }
    Some(ids)
}

/// Cached [`live_claude_sessions`] with a ~2s TTL, so frequent `lane.list` calls don't hammer
/// `lsof`.
async fn live_sessions_cached(ctx: &Ctx) -> Option<std::collections::HashSet<String>> {
    {
        let cache = ctx.live_sessions.lock().await;
        if let Some((t, set)) = &*cache {
            if t.elapsed() < std::time::Duration::from_secs(2) {
                return Some(set.clone());
            }
        }
    }
    let fresh = tokio::task::spawn_blocking(live_claude_sessions)
        .await
        .ok()
        .flatten();
    if let Some(set) = &fresh {
        *ctx.live_sessions.lock().await = Some((std::time::Instant::now(), set.clone()));
    }
    fresh
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
    fn builtins_are_recognized() {
        // claude-code is always present (the default config dir is always listed).
        assert!(is_builtin("claude-code"));
        assert!(is_builtin("codex"));
        assert!(!is_builtin("claude-yolo"));
    }
}
