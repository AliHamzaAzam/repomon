//! JSON-RPC method dispatch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use repomon_core::agent::{self, shell_quote};
use repomon_core::git::{diff, reader};
use repomon_core::model::{
    AgentChoice, AgentKind, AgentSession, AgentStatus, BrowseEntry, BrowseResult, Commit,
    CreateLaneParams, Lane, RepoId, TimeRange,
};
use repomon_core::protocol::RpcError;
use repomon_core::{Indexer, TmuxRuntime, analytics, session};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{Ctx, ORCHESTRATOR_WINDOW};

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

/// `agent.answer` found the pane in a different state than the client expected (dialog gone or
/// replaced). `error.data.dialog` carries what's actually there now (possibly null) so the
/// client can re-render instead of re-fetching.
const DIALOG_CHANGED: i64 = -32010;

/// Record that input reached a lane window: stamp `input_seen` (quiets the notification
/// engine) and drop the window's sniff-cache entry, so an answered dialog can't be
/// re-advertised by `lane.list` for the rest of its TTL.
async fn mark_input(ctx: &Ctx, lane: repomon_core::model::LaneId, window: &str) {
    ctx.input_seen
        .lock()
        .await
        .insert(lane, std::time::Instant::now());
    ctx.prompt_cache.lock().await.remove(window);
}

/// Truncate `s` to at most `max_chars` characters (char-boundary safe), returning the possibly
/// truncated string and whether it was cut. Used to cap `lane.diff`'s patch text server-side.
fn cap_chars(s: &str, max_chars: usize) -> (String, bool) {
    if s.chars().count() <= max_chars {
        return (s.to_string(), false);
    }
    (s.chars().take(max_chars).collect(), true)
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
        "embedded_pty": cfg.embedded_pty,
        "orchestrator_agent": cfg.orchestrator_agent,
        "orchestrator_model": cfg.orchestrator_model,
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
struct AgentPrompt {
    lane_id: repomon_core::model::LaneId,
    #[serde(default)]
    window: Option<String>,
}
#[derive(Deserialize)]
struct AgentWatchBytes {
    lane_id: repomon_core::model::LaneId,
    #[serde(default)]
    window: Option<String>,
    on: bool,
}
#[derive(Deserialize)]
struct AgentAnswer {
    lane_id: repomon_core::model::LaneId,
    /// 0-based index into the dialog's options.
    choice: usize,
    #[serde(default)]
    window: Option<String>,
    /// When set, the answer is sent only if the pane's current dialog still summarizes to
    /// this exact string — the client's stale-view guard.
    #[serde(default)]
    expect_summary: Option<String>,
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
    #[serde(default)]
    embedded_pty: Option<bool>,
    #[serde(default)]
    orchestrator_agent: Option<String>,
    #[serde(default)]
    orchestrator_model: Option<String>,
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
    /// Plain-terminal windows (`term-{lane}-{n}`) visible as Grid tiles, streamed alongside
    /// the lane panes. Non-terminal names are ignored.
    #[serde(default)]
    windows: Vec<String>,
}
#[derive(Deserialize)]
struct LaneMerge {
    lane_id: repomon_core::model::LaneId,
    #[serde(default)]
    into: Option<String>,
}
#[derive(Deserialize)]
struct LaneDiffParams {
    lane_id: repomon_core::model::LaneId,
    #[serde(default)]
    include_patch: bool,
    #[serde(default = "default_max_patch_chars")]
    max_patch_chars: usize,
}
fn default_max_patch_chars() -> usize {
    8000
}
/// Server-side cap: even a caller-supplied `max_patch_chars` can't force an unbounded patch.
const MAX_PATCH_CHARS_CEILING: usize = 20_000;
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
#[derive(Deserialize, Default)]
struct OrchestratorStart {
    /// Override the orchestrator agent — a Claude account (e.g. `claude-work`), a custom agent
    /// name, or `codex`; falls back to `orchestrator_agent` in config, then bare `claude`.
    /// Anything else (no MCP client → can't drive the fleet) is rejected with invalid_params.
    #[serde(default)]
    agent: Option<String>,
    /// Override the model (e.g. `opus`); falls back to `orchestrator_model` in config.
    #[serde(default)]
    model: Option<String>,
    /// How autonomous repomind is (passed to the MCP server as `REPOMON_MCP_AUTONOMY`).
    #[serde(default = "default_autonomy")]
    autonomy: String,
    /// Cap on how many agents repomind may run at once (`REPOMON_MCP_MAX_AGENTS`).
    #[serde(default)]
    max_agents: Option<usize>,
    /// An initial goal to seed the session with.
    #[serde(default)]
    prompt: Option<String>,
}
fn default_autonomy() -> String {
    "autonomous".to_string()
}
#[derive(Deserialize)]
struct OrchestratorInput {
    text: String,
    /// Press Enter after the text (default). `false` just inserts it.
    #[serde(default = "default_true")]
    enter: bool,
}
#[derive(Deserialize)]
struct OrchestratorKey {
    key: String,
    /// Send the key as literal text rather than a tmux key name.
    #[serde(default)]
    literal: bool,
}
#[derive(Deserialize)]
struct OrchestratorWatch {
    /// `true` while a client is viewing the orchestrator pane (gates `stream_orchestrator` so the
    /// daemon only captures the window while someone's watching).
    on: bool,
}
#[derive(Deserialize)]
struct OrchestratorResize {
    cols: u16,
    rows: u16,
}
#[derive(Deserialize)]
struct OrchestratorTranscript {
    /// How many recent transcript items to return.
    #[serde(default = "default_transcript_limit")]
    limit: usize,
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
        "lane.diff" => {
            let p: LaneDiffParams = parse(params)?;
            let max_patch_chars = p.max_patch_chars.min(MAX_PATCH_CHARS_CEILING);
            let lane = ctx.lanes.get(p.lane_id).await.map_err(internal)?;
            let repo_path = lane.repo.path.clone();
            let wt_path = lane.worktree.path.clone();
            let include_patch = p.include_patch;
            let (d, patch) = tokio::task::spawn_blocking(move || -> Result<_, RpcError> {
                // Base branch = the repo MAIN checkout's current branch, not the lane's.
                let repo = reader::open(&repo_path).map_err(internal)?;
                let hi = reader::head_info(&repo).map_err(internal)?;
                let base = hi.branch.ok_or_else(|| {
                    RpcError::internal(format!(
                        "repo's main checkout ({}) has no current branch to diff against \
                         (detached HEAD)",
                        repo_path.display()
                    ))
                })?;
                let d = diff::lane_diff(&wt_path, &base).map_err(internal)?;
                let patch = if include_patch {
                    Some(diff::diff_patch(&wt_path).map_err(internal)?)
                } else {
                    None
                };
                Ok((d, patch))
            })
            .await
            .map_err(internal)??;

            let mut result = json!({
                "base": d.base,
                "merge_base": d.merge_base,
                "commits": d.commits,
                "committed_stat": d.committed_stat,
                "uncommitted_stat": d.uncommitted_stat,
                "untracked": lane.state.dirty.untracked,
            });
            if d.commits_truncated {
                result["commits_truncated"] = json!(true);
            }
            if let Some(patch) = patch {
                let (capped, truncated) = cap_chars(&patch, max_patch_chars);
                result["patch"] = json!(capped);
                if truncated {
                    result["patch_truncated"] = json!(true);
                }
            }
            Ok(result)
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
                        )));
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
                if let Some(b) = p.embedded_pty {
                    cfg.embedded_pty = b;
                }
                // An empty string clears the override (back to bare `claude` / the model default),
                // so the Settings view can cycle to a "default" entry.
                if let Some(a) = p.orchestrator_agent {
                    cfg.orchestrator_agent = (!a.is_empty()).then_some(a);
                }
                if let Some(m) = p.orchestrator_model {
                    cfg.orchestrator_model = (!m.is_empty()).then_some(m);
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
            let win = window.clone();
            tokio::task::spawn_blocking(move || {
                if enter {
                    tmux.send_text_named(&win, &text)
                } else {
                    tmux.send_literal_named(&win, &text)
                }
            })
            .await
            .map_err(internal)?
            .map_err(internal)?;
            mark_input(ctx, lane, &window).await;
            Ok(Value::Null)
        }
        "agent.signal" => {
            let p: AgentSignal = parse(params)?;
            let tmux = ctx.tmux.clone();
            let (lane, key) = (p.lane_id, p.key);
            let window = p.window.unwrap_or_else(|| TmuxRuntime::window_name(lane));
            let win = window.clone();
            tokio::task::spawn_blocking(move || tmux.send_key_named(&win, &key))
                .await
                .map_err(internal)?
                .map_err(internal)?;
            mark_input(ctx, lane, &window).await;
            Ok(Value::Null)
        }
        "agent.key" => {
            let p: AgentKey = parse(params)?;
            let tmux = ctx.tmux.clone();
            let (lane, key, literal) = (p.lane_id, p.key, p.literal);
            let window = p.window.unwrap_or_else(|| TmuxRuntime::window_name(lane));
            let win = window.clone();
            tokio::task::spawn_blocking(move || {
                if literal {
                    tmux.send_literal_named(&win, &key)
                } else {
                    tmux.send_key_named(&win, &key)
                }
            })
            .await
            .map_err(internal)?
            .map_err(internal)?;
            mark_input(ctx, lane, &window).await;
            Ok(Value::Null)
        }
        "agent.watch_bytes" => {
            // The embedded renderer's feed: stream one pane's raw PTY bytes as
            // `event.agent.bytes`. Single-watch semantics — a new `on` replaces the previous
            // watch, `off` just stops it.
            let p: AgentWatchBytes = parse(params)?;
            let window = p
                .window
                .unwrap_or_else(|| TmuxRuntime::window_name(p.lane_id));
            crate::bytes_stream::stop(&ctx.tmux, &ctx.bytes_watch).await;
            if p.on {
                crate::bytes_stream::start(
                    ctx.tmux.clone(),
                    ctx.events.clone(),
                    &ctx.bytes_watch,
                    p.lane_id,
                    window,
                )
                .await
                .map_err(internal)?;
            }
            Ok(Value::Null)
        }
        "agent.prompt" => {
            let p: AgentPrompt = parse(params)?;
            let tmux = ctx.tmux.clone();
            let window = p
                .window
                .unwrap_or_else(|| TmuxRuntime::window_name(p.lane_id));
            let win = window.clone();
            // A fresh capture, not the sniff cache: the popup must show what's on the pane NOW.
            let dialog = tokio::task::spawn_blocking(move || {
                tmux.capture_named(&win, Some(45))
                    .map(|pane| agent::prompt::detect_dialog(&pane))
            })
            .await
            .map_err(internal)?
            .map_err(internal)?;
            ctx.prompt_cache
                .lock()
                .await
                .insert(window, (std::time::Instant::now(), dialog.clone()));
            Ok(json!({ "dialog": dialog }))
        }
        "agent.answer" => {
            let p: AgentAnswer = parse(params)?;
            let tmux = ctx.tmux.clone();
            let window = p
                .window
                .unwrap_or_else(|| TmuxRuntime::window_name(p.lane_id));
            // Re-capture and verify before sending anything: the dialog the client saw may have
            // been answered, replaced, or scrolled away since. Never steer a pane blind.
            let win = window.clone();
            let cap_tmux = tmux.clone();
            let dialog = tokio::task::spawn_blocking(move || {
                cap_tmux
                    .capture_named(&win, Some(45))
                    .map(|pane| agent::prompt::detect_dialog(&pane))
            })
            .await
            .map_err(internal)?
            .map_err(internal)?;
            let Some(dialog) = dialog else {
                // Record the no-dialog result so `lane.list` stops advertising the ghost.
                ctx.prompt_cache
                    .lock()
                    .await
                    .insert(window, (std::time::Instant::now(), None));
                return Err(RpcError {
                    code: DIALOG_CHANGED,
                    message: "no pending dialog".into(),
                    data: Some(json!({ "dialog": Value::Null })),
                });
            };
            if let Some(expect) = &p.expect_summary {
                if *expect != dialog.summary() {
                    ctx.prompt_cache
                        .lock()
                        .await
                        .insert(window, (std::time::Instant::now(), Some(dialog.clone())));
                    return Err(RpcError {
                        code: DIALOG_CHANGED,
                        message: "dialog changed".into(),
                        data: Some(json!({ "dialog": dialog })),
                    });
                }
            }
            if p.choice >= dialog.options.len() {
                return Err(RpcError::invalid_params(format!(
                    "choice {} out of range (dialog has {} options)",
                    p.choice,
                    dialog.options.len()
                )));
            }
            let keys = agent::prompt::dialog_select_keys(&dialog, p.choice);
            let win = window.clone();
            let send_keys = keys.clone();
            tokio::task::spawn_blocking(move || {
                send_keys
                    .iter()
                    .try_for_each(|k| tmux.send_key_named(&win, k))
            })
            .await
            .map_err(internal)?
            .map_err(internal)?;
            mark_input(ctx, p.lane_id, &window).await;
            ctx.invalidate_overlay().await;
            Ok(json!({
                "answered": dialog.options[p.choice].text,
                "sent": keys,
            }))
        }
        "agent.stop" => {
            let p: AgentStop = parse(params)?;
            let lane = p.lane_id;
            let window = p.window.unwrap_or_else(|| TmuxRuntime::window_name(lane));
            // Kill the window and reconcile the window-liveness caches synchronously (the same
            // helper the orphan reaper uses), so an immediately-following `lane.get` can never
            // read this agent back as still live while waiting out `resolve_windows`'s
            // total-vanish debounce. See `reap::kill_and_forget`.
            crate::reap::kill_and_forget(ctx, &window).await;
            let tmux = ctx.tmux.clone();
            let remaining = tokio::task::spawn_blocking(move || {
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
        "terminal.list_all" => {
            // Every lane's open plain terminals — what the Grid tiles. Fleet-wide (unlike
            // `terminal.list`) so one call covers every visible lane.
            let tmux = ctx.tmux.clone();
            let wins = tokio::task::spawn_blocking(move || tmux.list_windows().unwrap_or_default())
                .await
                .map_err(internal)?;
            let mut terms: Vec<Value> = wins
                .into_iter()
                .filter_map(|w| {
                    TmuxRuntime::parse_term_window(&w)
                        .map(|lane| json!({ "lane_id": lane, "id": w }))
                })
                .collect();
            terms.sort_by_key(|t| {
                (
                    t["lane_id"].as_i64().unwrap_or(0),
                    t["id"].as_str().unwrap_or("").to_string(),
                )
            });
            Ok(Value::Array(terms))
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
            let mut p: ViewportSet = parse(params)?;
            *ctx.viewport.lock().await = p.lane_ids;
            *ctx.viewport_focus.lock().await = p.focus_lane.zip(p.focus_window);
            // Only real terminal windows are streamable extras — anything else is dropped so a
            // client can't point the capture loop at arbitrary windows.
            p.windows
                .retain(|w| TmuxRuntime::parse_term_window(w).is_some());
            *ctx.viewport_windows.lock().await = p.windows;
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
            let label = p
                .label
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty());
            ctx.store
                .set_session_label(p.session_id, label)
                .await
                .map_err(internal)?;
            ctx.invalidate_overlay().await;
            ctx.broadcast(
                crate::pubsub::topic::AGENT_STATUS,
                json!({ "renamed": true }),
            );
            Ok(Value::Null)
        }

        // ---- repomind orchestrator (a single daemon-owned `claude` session) ----
        "orchestrator.status" => {
            // A window killed externally would otherwise still read as running; reconcile first.
            reconcile_orchestrator(ctx).await;
            let orch = ctx.orchestrator.lock().await;
            let (attention, headline) = ctx.orchestrator_attention.lock().await.clone();
            Ok(orchestrator_status_value(
                orch.as_ref(),
                &attention,
                headline.as_deref(),
            ))
        }
        // Repomind's conversation as structured TranscriptItems, so a client (the iOS app) can render
        // it as a chat instead of mirroring the raw pane. Pinned to the orchestrator's own
        // `session_id` when known (captured at spawn via `--session-id`); an adopted session (whose
        // id this process never captured) falls back to the newest $HOME transcript with real
        // content across accounts — see `pick_orchestrator_transcript_in`.
        "orchestrator.transcript" => {
            let p: OrchestratorTranscript = parse(params)?;
            reconcile_orchestrator(ctx).await;
            let orch = ctx.orchestrator.lock().await;
            let Some(session) = orch.as_ref() else {
                return Ok(json!([]));
            };
            // A backend without a parseable transcript reads as an empty chat — deliberately NOT
            // the recency-heuristic fallback below, which would misattribute some other live
            // Claude session's transcript as this orchestrator's. Clients render the live pane
            // stream (`event.orchestrator.output`) instead.
            if !session.backend.has_transcript() {
                return Ok(json!([]));
            }
            let session_id = session.session_id.clone();
            drop(orch);
            let items = tokio::task::spawn_blocking(move || {
                pick_orchestrator_transcript(session_id.as_deref())
                    .map(|s| agent::claude::transcript_tail(&s.manifest_path, p.limit))
                    .unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            to_value(items)
        }
        "orchestrator.start" => {
            let p: OrchestratorStart = parse(params)?;
            // Clear a session whose window died externally so a restart actually re-spawns instead
            // of no-op'ing on a corpse.
            reconcile_orchestrator(ctx).await;
            // Hold the session lock across the ENTIRE check → adopt/spawn → record sequence.
            // Releasing it between the is-running check and the record (as this handler once did)
            // let two concurrent starts — a real scenario: the TUI's command-center auto-start and
            // `repomon orchestrate` both fire at startup on separate connections — both observe
            // "not running" and race the spawn: the loser then either failed its own `new-session`
            // outright, spawned a duplicate `orchestrator` window (tmux allows duplicate names),
            // or took the adopt branch on the winner's fresh window and overwrote its
            // just-recorded session id/autonomy. Holding a tokio Mutex across the awaits below is
            // fine — it merely serializes concurrent start/stop/status for the ~tens of ms a tmux
            // spawn takes. Nothing in this region re-locks `ctx.orchestrator` (the audit:
            // `reconcile_orchestrator` runs above, before the guard; config is a separate RwLock;
            // `orchestrator_attention` is only ever taken after — never while holding — it).
            let mut orch = ctx.orchestrator.lock().await;
            // Already tracking a live session: idempotent no-op (don't spawn a second window).
            if orch.is_some() {
                let (attention, headline) = ctx.orchestrator_attention.lock().await.clone();
                return Ok(orchestrator_status_value(
                    orch.as_ref(),
                    &attention,
                    headline.as_deref(),
                ));
            }
            // Resolve agent/model: explicit param wins, then the persisted config default.
            let (cfg_agent, cfg_model, customs) = {
                let cfg = ctx.config.read().await;
                (
                    cfg.orchestrator_agent.clone(),
                    cfg.orchestrator_model.clone(),
                    cfg.agents.clone(),
                )
            };
            let agent = p.agent.or(cfg_agent);
            let model = p.model.or(cfg_model);
            // Resolved once here, recorded on the session for BOTH the adopt and spawn paths, and
            // consulted everywhere a Claude-only capability would otherwise be assumed. Errors out
            // (guard drops, nothing recorded) on an agent that can't run the orchestrator at all.
            let backend = resolve_orchestrator_backend(&agent, &customs)?;
            // A window may survive a daemon restart (tmux outlives us). Adopt it instead of
            // spawning a duplicate `orchestrator` window.
            {
                let tmux = ctx.tmux.clone();
                let exists =
                    tokio::task::spawn_blocking(move || tmux.has_named(ORCHESTRATOR_WINDOW))
                        .await
                        .map_err(internal)?;
                if exists {
                    // Adopting a window from a previous daemon lifetime: we don't know what
                    // autonomy — or session id — it was actually launched with (that lived in the
                    // prior process's memory, not anywhere persisted), so record both as unknown
                    // rather than asserting the caller's (possibly different) requested value or a
                    // freshly-minted id that isn't actually this window's.
                    let session = crate::OrchestratorSession {
                        agent: agent.clone(),
                        model: model.clone(),
                        window: ORCHESTRATOR_WINDOW.to_string(),
                        autonomy: None,
                        session_id: None,
                        backend,
                    };
                    *orch = Some(session);
                    let (attention, headline) = ctx.orchestrator_attention.lock().await.clone();
                    let status =
                        orchestrator_status_value(orch.as_ref(), &attention, headline.as_deref());
                    ctx.broadcast(crate::pubsub::topic::ORCHESTRATOR_STATUS, status.clone());
                    return Ok(status);
                }
            }
            let socket = repomon_core::config::socket_path(&*ctx.config.read().await);
            let base = orchestrator_base_command(&agent, &customs);
            let (command, session_id) = match backend {
                crate::OrchestratorBackend::Claude => {
                    // Build the MCP config file that points the orchestrator's `claude` at
                    // `repomond mcp`. The server's env is authoritative for the socket +
                    // guardrails.
                    let mcp_path =
                        write_orchestrator_mcp_config(&socket, &p.autonomy, p.max_agents)
                            .map_err(internal)?;
                    // Minted fresh for this genuine spawn (never for adopt — see above) so the
                    // transcript picker can pin `orchestrator.transcript`/the end-of-turn check
                    // to this exact session.
                    let session_id = mint_session_id();
                    let command = build_claude_orchestrator_command(
                        &base,
                        &mcp_path,
                        &model,
                        &p.prompt,
                        &session_id,
                    );
                    (command, Some(session_id))
                }
                // Codex takes its MCP registration inline (`-c` overrides — no config file) and
                // has no session pinning; the transcript/end-of-turn paths gate on
                // `backend.has_transcript()` instead of a session id.
                crate::OrchestratorBackend::Codex => (
                    build_codex_orchestrator_command(
                        &base,
                        &socket,
                        &p.autonomy,
                        p.max_agents,
                        &model,
                        &p.prompt,
                    ),
                    None,
                ),
            };
            // cwd = $HOME, so repomind starts from the user's home rather than the daemon's cwd.
            let home = std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/"));
            let tmux = ctx.tmux.clone();
            tokio::task::spawn_blocking(move || {
                tmux.spawn_named(ORCHESTRATOR_WINDOW, &home, &command)
            })
            .await
            .map_err(internal)?
            .map_err(internal)?;
            let session = crate::OrchestratorSession {
                agent,
                model,
                window: ORCHESTRATOR_WINDOW.to_string(),
                autonomy: Some(p.autonomy),
                session_id,
                backend,
            };
            *orch = Some(session);
            let (attention, headline) = ctx.orchestrator_attention.lock().await.clone();
            let status = orchestrator_status_value(orch.as_ref(), &attention, headline.as_deref());
            ctx.broadcast(crate::pubsub::topic::ORCHESTRATOR_STATUS, status.clone());
            Ok(status)
        }
        "orchestrator.stop" => {
            // Take the session lock BEFORE the kill so a stop can't interleave with a concurrent
            // `orchestrator.start` (which holds this lock across its spawn): stop either runs
            // first against nothing, or kills the fully-recorded window — never a window that a
            // mid-flight start is about to record (which would leave an untracked orphan running).
            let mut orch = ctx.orchestrator.lock().await;
            let tmux = ctx.tmux.clone();
            let _ = tokio::task::spawn_blocking(move || tmux.kill_named(ORCHESTRATOR_WINDOW)).await;
            // Unlike `agent.stop` (see `reap::kill_and_forget`), no cache reconciliation is needed
            // after this kill: `prompt_cache` only ever holds lane-window sniffs (`overlay_agents`
            // keys it by lane candidates, which the orchestrator window deliberately isn't), and
            // while `last_good_windows` does carry `orchestrator`, every consumer of the resolved
            // list filters to `lane-*` and orchestrator liveness is always probed directly via
            // `has_named`. Dropping the entry anyway is cheap hygiene, not correctness.
            ctx.last_good_windows
                .lock()
                .await
                .retain(|w| w != ORCHESTRATOR_WINDOW);
            *orch = None;
            *ctx.orchestrator_attention.lock().await = ("none".to_string(), None);
            let status = orchestrator_status_value(None, "none", None);
            ctx.broadcast(crate::pubsub::topic::ORCHESTRATOR_STATUS, status.clone());
            Ok(status)
        }
        "orchestrator.target" => {
            // Clear + broadcast stopped if the window died, so a stale "running" can't linger.
            reconcile_orchestrator(ctx).await;
            let tmux = ctx.tmux.clone();
            // Restore client-follow sizing before the attaching terminal renders it (mirrors
            // `agent.target`).
            let available = tokio::task::spawn_blocking(move || {
                let _ = tmux.follow_client_named(ORCHESTRATOR_WINDOW);
                tmux.has_named(ORCHESTRATOR_WINDOW)
            })
            .await
            .map_err(internal)?;
            let target = format!("{}:={}", ctx.tmux.session(), ORCHESTRATOR_WINDOW);
            Ok(json!({ "target": target, "available": available }))
        }
        "orchestrator.send_input" => {
            let p: OrchestratorInput = parse(params)?;
            // A window killed externally would otherwise still read as running; reconcile first,
            // and refuse to type into a corpse instead of silently no-op'ing at the tmux layer.
            if !reconcile_orchestrator(ctx).await {
                return Err(RpcError::invalid_params(
                    "repomind isn't running — start it from the command-center or 'repomon orchestrate'",
                ));
            }
            let tmux = ctx.tmux.clone();
            let (text, enter) = (p.text, p.enter);
            tokio::task::spawn_blocking(move || {
                if enter {
                    tmux.send_text_named(ORCHESTRATOR_WINDOW, &text)
                } else {
                    tmux.send_literal_named(ORCHESTRATOR_WINDOW, &text)
                }
            })
            .await
            .map_err(internal)?
            .map_err(internal)?;
            // Frame-rate echo while typing: `stream_orchestrator` captures at ~30ms within
            // TYPING_WINDOW of this stamp, the same speedup `input_seen` gives a focused lane.
            *ctx.orchestrator_input_seen.lock().await = Some(std::time::Instant::now());
            Ok(Value::Null)
        }
        "orchestrator.key" => {
            let p: OrchestratorKey = parse(params)?;
            // Same reconcile-first guard as `orchestrator.send_input`: a dead window must not read
            // as a successful keystroke.
            if !reconcile_orchestrator(ctx).await {
                return Err(RpcError::invalid_params(
                    "repomind isn't running — start it from the command-center or 'repomon orchestrate'",
                ));
            }
            let tmux = ctx.tmux.clone();
            let (key, literal) = (p.key, p.literal);
            tokio::task::spawn_blocking(move || {
                if literal {
                    tmux.send_literal_named(ORCHESTRATOR_WINDOW, &key)
                } else {
                    tmux.send_key_named(ORCHESTRATOR_WINDOW, &key)
                }
            })
            .await
            .map_err(internal)?
            .map_err(internal)?;
            *ctx.orchestrator_input_seen.lock().await = Some(std::time::Instant::now());
            Ok(Value::Null)
        }
        // Gate the orchestrator pane stream: the TUI sets this `true` on entering the command-center
        // view and `false` on leaving, so `stream_orchestrator` captures the window only while a
        // client is actually watching.
        "orchestrator.watch" => {
            let p: OrchestratorWatch = parse(params)?;
            *ctx.orchestrator_watched.lock().await = p.on;
            Ok(Value::Null)
        }
        // Size the orchestrator window to the viewer's pane so the streamed capture fills it exactly
        // (no right-edge overflow, and no trailing blank rows from a too-tall window). Mirrors
        // `agent.resize`; `orchestrator.target` restores client-follow before a real attach.
        "orchestrator.resize" => {
            let p: OrchestratorResize = parse(params)?;
            let tmux = ctx.tmux.clone();
            // Clamp to a sane floor so a momentary tiny layout can't shrink the window to nothing.
            let (cols, rows) = (p.cols.max(20), p.rows.max(4));
            tokio::task::spawn_blocking(move || tmux.resize_named(ORCHESTRATOR_WINDOW, cols, rows))
                .await
                .map_err(internal)?
                .map_err(internal)?;
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
/// A transcript written this recently means its session is writing *right now* — proof of
/// liveness independent of the process probe. Such sessions are never truncated, a backstop so an
/// actively-working agent can't vanish even if the probe momentarily misses it.
const RECENTLY_ACTIVE_SECS: i64 = 60;

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
        // `summaries` is newest-first. Keep as many as the worktree has live `claude` processes
        // (or managed windows), so a `/exit`ed session — no live process — is dropped rather than
        // lingering. `fresh` (sessions writing right now) is a backstop that keeps an
        // actively-working agent even if the process probe momentarily misses it.
        let now = chrono::Utc::now();
        let fresh = summaries
            .iter()
            .filter(|s| (now - s.last_activity).num_seconds() < RECENTLY_ACTIVE_SECS)
            .count();
        let keep = sessions_to_keep(summaries.len(), alive, managed_n, fresh);
        summaries.truncate(keep);
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
                lane.agent_sessions.push(window_placeholder_session(
                    lane,
                    kind,
                    lane_windows[w].clone(),
                ));
            }
        } else if managed_n > 0 {
            // No parseable transcript: surface a repomon-spawned agent if its window is alive.
            let kind = lane_meta_kind(&metas, lane.id);
            lane.agent_sessions.push(window_placeholder_session(
                lane,
                kind,
                lane_windows[0].clone(),
            ));
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
                    pending_dialog: None,
                    stale: false,
                    stalled_since: None,
                    ended_turn: false,
                    gate: None,
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

    // dxkit stop-gate verdicts: worktrees running dxkit's loop pack leave an append-only
    // ledger (`.dxkit/loop/ledger.jsonl`); its tail verdict is overlaid onto the lane's real
    // sessions so a fresh `allowed` grants (and a block vetoes) the done-candidate hint.
    // Cached by the ledger's mtime — one cheap stat per lane per overlay, a re-read only when
    // the gate actually ran again. Session matching happens client-side in `attention`.
    {
        let mut cache = ctx.gate_cache.lock().await;
        for lane in lanes.iter_mut() {
            let wt = lane.worktree.path.clone();
            let mtime = std::fs::metadata(wt.join(agent::gate::LEDGER_REL))
                .and_then(|m| m.modified())
                .ok();
            let verdict = match cache.get(&wt) {
                Some((m, v)) if *m == mtime => v.clone(),
                _ => {
                    let v = mtime.and_then(|_| agent::gate::read_gate_verdict(&wt));
                    cache.insert(wt.clone(), (mtime, v.clone()));
                    v
                }
            };
            if let Some(v) = verdict {
                for s in lane.agent_sessions.iter_mut().filter(|s| !s.inferred) {
                    s.gate = Some(v.clone());
                }
            }
        }
        // Bounded by the live lane set: drop worktrees no longer listed.
        let live: std::collections::HashSet<&PathBuf> =
            lanes.iter().map(|l| &l.worktree.path).collect();
        cache.retain(|wt, _| live.contains(wt));
    }

    // Interactive dialogs: a transcript that ends in a tool call reads **Running**, but the
    // pane may be sitting on a permission "Do you want…?" dialog; a turn ending in text reads
    // **Waiting**, but the pane may be showing an option menu (plan approval, a question with
    // choices). Neither is in the JSONL. Sniff the panes of managed sessions: a detected
    // dialog sets `pending_prompt` (clients gate approve/menu controls on it), becomes the
    // notification-ready "why", and flips the status → Waiting. Idle sessions with a live
    // window are sniffed too — a dialog sitting unanswered for more than IDLE_AFTER decays the
    // transcript to Idle, and skipping it here would silently drop its ⏸ — and the same
    // captures feed the stall detector below.
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
                        && matches!(
                            s.status,
                            AgentStatus::Running | AgentStatus::Waiting | AgentStatus::Idle
                        );
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
        let mut prompts: Vec<Option<agent::prompt::PendingDialog>> =
            Vec::with_capacity(candidates.len());
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
            let miss_windows: Vec<String> =
                misses.iter().map(|&i| candidates[i].2.clone()).collect();
            // Each fresh capture yields the parsed dialog AND a content hash — the hash feeds
            // the stall detector's "when did this pane last change?" clock.
            let fresh: Vec<(Option<agent::prompt::PendingDialog>, Option<u64>)> =
                tokio::task::spawn_blocking(move || {
                    miss_windows
                        .iter()
                        .map(|w| match tmux.capture_named(w, Some(45)) {
                            Ok(pane) => {
                                use std::hash::{Hash, Hasher};
                                let mut h = std::collections::hash_map::DefaultHasher::new();
                                pane.hash(&mut h);
                                (agent::prompt::detect_dialog(&pane), Some(h.finish()))
                            }
                            Err(_) => (None, None),
                        })
                        .collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default();
            let now_utc = chrono::Utc::now();
            let mut cache = ctx.prompt_cache.lock().await;
            let mut seen = ctx.pane_seen.lock().await;
            for (&i, (p, hash)) in misses.iter().zip(fresh) {
                let window = &candidates[i].2;
                cache.insert(window.clone(), (std::time::Instant::now(), p.clone()));
                // Stamp the pane's last-change time only when the content actually differs.
                if let Some(h) = hash {
                    match seen.get(window) {
                        Some((prev, _)) if *prev == h => {}
                        _ => {
                            seen.insert(window.clone(), (h, now_utc));
                        }
                    }
                }
                prompts[i] = p;
            }
        }
        // Prune the sniff caches so they can't grow without bound — every window name ever
        // sniffed would otherwise leak an entry. `prompt_cache` also drops results older than
        // the longest sniff TTL (they'd be re-captured anyway); `pane_seen` is pruned by window
        // liveness ONLY — its old timestamps are the stall clock.
        {
            let live: std::collections::HashSet<&str> =
                windows.iter().map(String::as_str).collect();
            let mut cache = ctx.prompt_cache.lock().await;
            cache.retain(|w, (t, _)| live.contains(w.as_str()) && t.elapsed() < SNIFF_TTL);
            let mut seen = ctx.pane_seen.lock().await;
            seen.retain(|w, _| live.contains(w.as_str()));
        }
        let now_utc = chrono::Utc::now();
        let seen = ctx.pane_seen.lock().await;
        for ((li, si, w, _), found) in candidates.into_iter().zip(prompts) {
            let s = &mut lanes[li].agent_sessions[si];
            match found {
                Some(dialog) => {
                    s.status = AgentStatus::Waiting;
                    let summary = dialog.summary();
                    s.last_message = Some(summary.clone());
                    s.pending_prompt = Some(summary);
                    s.pending_dialog = Some(dialog);
                }
                // No dialog: this is where a live-but-frozen agent surfaces as stalled.
                None => {
                    let changed_at = seen.get(&w).map(|&(_, t)| t);
                    if let Some(since) =
                        stall_since(s.status, s.ended_turn, false, changed_at, now_utc)
                    {
                        s.stale = true;
                        s.stalled_since = Some(since);
                    }
                }
            }
        }
    }

    // Diagnostic: attribute any session that vanished since the previous overlay tick, so the
    // intermittent "sessions disappear after idle" report names its own cause in the log.
    diagnose_vanished_sessions(ctx, lanes, live.as_ref()).await;
}

/// How many of a lane's newest-first transcript sessions to keep, given the worktree's live
/// `claude`-process count (`alive`), its managed-window count (`managed_n`), and how many of its
/// sessions are writing right now (`fresh`).
///
/// With the reliable `ps`-based probe, `alive` is trustworthy: a count of 0 means no live agent,
/// so a `/exit`ed session's lingering transcript is dropped rather than shown. `fresh` is a
/// backstop — a session writing right now is kept regardless of the probe — and a probe failure
/// (`None`) doesn't filter at all.
fn sessions_to_keep(total: usize, alive: Option<usize>, managed_n: usize, fresh: usize) -> usize {
    match alive {
        Some(n) => n.max(managed_n).max(fresh).min(total),
        None => total, // probe unavailable: don't filter
    }
}

/// How long a managed agent's pane must sit unchanged — with no dialog up and its turn not
/// ended — before the session reads as stalled.
const STALL_AFTER_MINS: i64 = 5;

/// When a sniffed session counts as stalled, returns the stall's start (the pane's last
/// change). A stall is: workless status (Running mid-tool, or the post-decay Idle) with no
/// dialog on screen, a turn that did NOT end (that would be waiting-for-instructions, not
/// stuck), and a pane frozen for [`STALL_AFTER_MINS`]. `None` = not stalled.
fn stall_since(
    status: AgentStatus,
    ended_turn: bool,
    has_dialog: bool,
    pane_changed_at: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    if has_dialog || ended_turn || !matches!(status, AgentStatus::Running | AgentStatus::Idle) {
        return None;
    }
    pane_changed_at.filter(|&t| now - t >= chrono::Duration::minutes(STALL_AFTER_MINS))
}

/// A stable identity for a surfaced session: its transcript id, else `win:<window>` (a managed
/// placeholder with no transcript yet) or `inferred:<wt>` (a file-activity session).
fn sess_key(s: &repomon_core::model::AgentSession) -> String {
    if let Some(id) = &s.session_id {
        id.clone()
    } else if s.inferred {
        format!("inferred:{}", s.worktree_id.unwrap_or(0))
    } else if let Some(w) = &s.tmux_window {
        format!("win:{w}")
    } else {
        "unknown".to_string()
    }
}

/// Compare this overlay's per-lane sessions to the previous tick's; for each session that
/// vanished, log it at INFO (`target: repomon::overlay`) with a **process-first** attributed
/// reason plus the worktree's live-`claude` count and the lane's remaining session count.
///
/// Process-first (not window-pairing-based) so a multi-agent exit transition — where transcripts
/// re-pair to the surviving windows — doesn't masquerade as a bug. Reasons:
/// - `process-exited` — no live `claude` remains in the worktree: a correct disappearance.
/// - `transcript-aged-out` / `alive-but-dropped` — a `claude` is still alive there but this row
///   dropped: the bug we're hunting. `alive=N sessions=M` disambiguates the multi-agent case
///   (a clean single-agent bug reads `alive>=1 sessions=0`).
/// - `inferred-expired` — a file-activity session aged out (~2 min, by design).
/// - `probe-unavailable` — the pgrep/lsof probe couldn't run this tick.
async fn diagnose_vanished_sessions(
    ctx: &Ctx,
    lanes: &[Lane],
    live: Option<&std::collections::HashMap<std::path::PathBuf, usize>>,
) {
    let current: std::collections::HashMap<
        repomon_core::model::LaneId,
        Vec<crate::OverlaySession>,
    > = lanes
        .iter()
        .map(|lane| {
            let recs = lane
                .agent_sessions
                .iter()
                .map(|s| crate::OverlaySession {
                    key: sess_key(s),
                    external: s.external,
                    inferred: s.inferred,
                    window: s.tmux_window.clone(),
                    manifest: s.manifest_path.clone(),
                    worktree: lane.worktree.path.clone(),
                })
                .collect();
            (lane.id, recs)
        })
        .collect();

    let cutoff = chrono::Utc::now() - chrono::Duration::hours(SESSION_WINDOW_HOURS);
    let mut prev_map = ctx.last_overlay_sessions.lock().await;
    for lane in lanes {
        let cur = &current[&lane.id];
        let Some(prev) = prev_map.get(&lane.id) else {
            continue;
        };
        // The worktree's live `claude` count — the process-first liveness signal.
        let alive = live.and_then(|m| {
            lane.worktree
                .path
                .canonicalize()
                .ok()
                .map(|k| m.get(&k).copied().unwrap_or(0))
        });
        for p in prev {
            if cur.iter().any(|c| c.key == p.key) {
                continue;
            }
            let reason = vanish_reason(p, alive, cutoff);
            tracing::debug!(
                target: "repomon::overlay",
                lane = lane.id,
                session = %p.key,
                external = p.external,
                inferred = p.inferred,
                window = ?p.window,
                alive = ?alive,
                sessions = cur.len(),
                reason,
                "session vanished"
            );
        }
    }
    *prev_map = current;
}

/// Attribute a vanished session from the worktree's live-`claude` count (`alive`) and the
/// transcript age. See [`diagnose_vanished_sessions`] for the reason vocabulary.
fn vanish_reason(
    p: &crate::OverlaySession,
    alive: Option<usize>,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> &'static str {
    if p.inferred {
        return "inferred-expired";
    }
    match alive {
        Some(0) => "process-exited",
        None => "probe-unavailable",
        Some(_) => {
            // A `claude` is alive in this worktree, yet this row dropped. Did its transcript age
            // past the 6h window (the gate hiding a live agent), or drop for another reason?
            let aged = std::fs::metadata(&p.manifest)
                .and_then(|m| m.modified())
                .ok()
                .map(|t| chrono::DateTime::<chrono::Utc>::from(t) < cutoff)
                .unwrap_or(false);
            if aged {
                "transcript-aged-out"
            } else {
                "alive-but-dropped"
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

    pick_for_account(&candidates, &want).unwrap_or_else(|| {
        // Build via `launch_command` so the fallback is immune to the daemon's own
        // CLAUDE_CONFIG_DIR: the default account unsets it (`env -u …`), variants pin their dir.
        let default = agent::claude::default_config_base();
        agent::claude::launch_command(config_dir.as_deref().unwrap_or(&default))
    })
}

/// The account (CLAUDE_CONFIG_DIR, canonicalized) a command targets, or `None` for the default.
///
/// Variant accounts launch with an explicit, shell-quoted `CLAUDE_CONFIG_DIR=…` (see
/// [`agent::claude::launch_command`]); the default account launches as `env -u CLAUDE_CONFIG_DIR
/// claude` (no assignment). So this is `None` when the assignment is absent, parses the value
/// honoring shell quoting (a config dir may contain spaces), and normalizes the *default* base back
/// to `None` so the default account keeps its `None`/`"default"` identity regardless of spelling.
fn command_account(cmd: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(config_dir_arg(cmd)?);
    let dir = dir.canonicalize().unwrap_or(dir);
    let default = agent::claude::default_config_base();
    let default = default.canonicalize().unwrap_or(default);
    (dir != default).then_some(dir)
}

/// The `CLAUDE_CONFIG_DIR=` value from a command's leading env assignment, shell-unquoted, or
/// `None` if absent. Honors the single-quote grouping [`shell_quote`] emits, so a config dir
/// containing spaces (`CLAUDE_CONFIG_DIR='/a b/.claude' claude`) parses as one whole path rather
/// than being split on the inner space.
fn config_dir_arg(cmd: &str) -> Option<String> {
    const KEY: &str = "CLAUDE_CONFIG_DIR=";
    let mut from = 0;
    loop {
        let at = from + cmd[from..].find(KEY)?;
        // Only a real leading assignment (start of command, or right after whitespace).
        if at == 0 || cmd.as_bytes()[at - 1].is_ascii_whitespace() {
            return Some(unquote_shell_word(&cmd[at + KEY.len()..]));
        }
        from = at + KEY.len();
    }
}

/// Read and unquote one shell word from the front of `s`, honoring the single-quote grouping and
/// `'\''` escaping [`shell_quote`] emits; an unquoted word ends at the first whitespace.
fn unquote_shell_word(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => break,
            '\'' => {
                chars.next(); // opening quote
                for c in chars.by_ref() {
                    if c == '\'' {
                        break; // closing quote
                    }
                    out.push(c);
                }
            }
            '\\' => {
                chars.next(); // escape: next char is literal
                if let Some(c) = chars.next() {
                    out.push(c);
                }
            }
            _ => {
                out.push(c);
                chars.next();
            }
        }
    }
    out
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

/// The program a command runs, skipping a leading env prefix so both `CLAUDE_CONFIG_DIR=… claude`
/// and `env -u CLAUDE_CONFIG_DIR claude` (the default account's launch, which *unsets* the var)
/// resolve to `claude`.
fn program_of(command: &str) -> Option<&str> {
    let mut toks = command.split_whitespace().peekable();
    // A leading `env [-i] [-u NAME]… [NAME=val]… program` — skip `env` and its options/
    // assignments (note `-u` takes a NAME argument) so the real program surfaces.
    if toks.peek() == Some(&"env") {
        toks.next();
        while let Some(&t) = toks.peek() {
            if t == "-u" {
                toks.next(); // the flag
                toks.next(); // its NAME argument
            } else if t.starts_with('-') || is_env_assignment(t) {
                toks.next();
            } else {
                break;
            }
        }
        return toks.next();
    }
    // Otherwise skip leading `VAR=val` assignments.
    toks.find(|t| !is_env_assignment(t))
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
        pending_dialog: None,
        stale: false,
        stalled_since: None,
        ended_turn: false,
        gate: None,
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
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
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
#[cfg(not(target_os = "linux"))]
fn live_claude_cwds() -> Option<HashMap<PathBuf, usize>> {
    use std::process::Command;
    // Enumerate `claude` processes via `ps`, matching the executable basename. `pgrep -x claude`
    // proved UNRELIABLE on macOS: it misses live `claude` processes that `ps` lists (their kernel
    // accounting name differs from the exec name), so those worktrees read as alive=0 and had
    // their sessions truncated away — the disappearing-sessions bug. `-ww` disables column
    // truncation so a full-path `comm` isn't clipped before the basename match.
    let ps = Command::new("ps")
        .args(["-axww", "-o", "pid=,comm="])
        .output()
        .ok()?;
    let pids: Vec<String> = std::str::from_utf8(&ps.stdout)
        .ok()?
        .lines()
        .filter_map(|line| {
            let (pid, comm) = line.trim_start().split_once(char::is_whitespace)?;
            let base = comm.trim().rsplit('/').next().unwrap_or("");
            (base == "claude").then(|| pid.to_string())
        })
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

/// Linux variant: scan `/proc` directly — always present, no `ps`/`lsof` dependency, and
/// cheaper than either.
#[cfg(target_os = "linux")]
fn live_claude_cwds() -> Option<HashMap<PathBuf, usize>> {
    live_cwds_by_name("claude")
}

/// Count processes named `name` per working directory by walking `/proc`. A process matches
/// when its `comm` equals `name` (the kernel uses the script basename for `#!` launchers,
/// truncated to 15 bytes — "claude" fits) OR when the basename of its cmdline argv[0] does
/// (covers exec'd wrappers whose comm differs — the Linux analogue of the pgrep-vs-ps lesson
/// above). cwd comes from `/proc/<pid>/cwd`; entries we can't read (other users) are skipped.
#[cfg(target_os = "linux")]
fn live_cwds_by_name(name: &str) -> Option<HashMap<PathBuf, usize>> {
    let mut counts: HashMap<PathBuf, usize> = HashMap::new();
    for entry in std::fs::read_dir("/proc").ok()? {
        let Ok(entry) = entry else { continue };
        let file_name = entry.file_name();
        let Some(pid) = file_name
            .to_str()
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        let base = PathBuf::from("/proc").join(pid);
        let comm_matches = std::fs::read_to_string(base.join("comm"))
            .map(|c| c.trim() == name)
            .unwrap_or(false);
        let argv0_matches = || {
            std::fs::read(base.join("cmdline"))
                .ok()
                .and_then(|raw| {
                    let argv0 = raw.split(|&b| b == 0).next()?.to_vec();
                    let argv0 = String::from_utf8_lossy(&argv0).into_owned();
                    Some(argv0.rsplit('/').next().unwrap_or("") == name)
                })
                .unwrap_or(false)
        };
        if !comm_matches && !argv0_matches() {
            continue;
        }
        let Ok(cwd) = std::fs::read_link(base.join("cwd")) else {
            continue;
        };
        let key = cwd.canonicalize().unwrap_or(cwd);
        *counts.entry(key).or_insert(0) += 1;
    }
    Some(counts)
}

#[cfg(all(test, target_os = "linux"))]
mod live_cwds_tests {
    #[test]
    fn proc_scan_finds_self() {
        let comm = std::fs::read_to_string("/proc/self/comm")
            .unwrap()
            .trim()
            .to_string();
        let cwds = super::live_cwds_by_name(&comm).unwrap();
        let cwd = std::env::current_dir().unwrap();
        let key = cwd.canonicalize().unwrap_or(cwd);
        assert!(
            cwds.get(&key).copied().unwrap_or(0) >= 1,
            "expected {key:?} among {cwds:?}"
        );
    }
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
    let map = match tokio::task::spawn_blocking(live_claude_cwds)
        .await
        .ok()
        .flatten()
    {
        Some(m) => m,
        None => {
            // The probe couldn't run (ps/lsof spawn failed under load, /proc unreadable).
            // Returning None means "don't filter" — callers keep all recent sessions rather than
            // truncating to a bogus low count — but it was silent; log it so a flap is visible.
            tracing::warn!("live claude-process probe failed; not truncating sessions");
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

/// The `{running, agent, model, backend, window, autonomy, session_id, attention, headline}`
/// status JSON for the orchestrator (shared by `orchestrator.status` and the
/// `event.orchestrator.status` broadcast). `agent` is the raw name the session was started with
/// (`claude-work`, a custom, `codex`); `backend` is the normalized seam value
/// (`"claude"`/`"codex"`) clients should switch rendering on — a codex-backed session has no
/// transcript chat view, never reports `end_of_turn`, and always has a null `session_id`.
/// `autonomy` is the level the session was started with, or `null` if it was adopted from a
/// surviving tmux window and is therefore unknown. `session_id` is the `--session-id` UUID it was
/// launched with (see `mint_session_id`) — same "unknown → null" semantics as `autonomy` for an
/// adopted window, always null for codex. `attention` is one of `"none"`, `"permission"`,
/// `"decision"`, `"end_of_turn"` — see
/// [`notify_watch::check_orchestrator_attention`](crate::notify_watch); `headline` is a short
/// "why" (the pending dialog's question, or a tail of repomind's last message) or `null`.
pub(crate) fn orchestrator_status_value(
    orch: Option<&crate::OrchestratorSession>,
    attention: &str,
    headline: Option<&str>,
) -> Value {
    match orch {
        Some(s) => json!({
            "running": true,
            "agent": s.agent,
            "model": s.model,
            "backend": s.backend.as_str(),
            "window": s.window,
            "autonomy": s.autonomy,
            "session_id": s.session_id,
            "attention": attention,
            "headline": headline,
        }),
        None => json!({
            "running": false,
            "agent": Value::Null,
            "model": Value::Null,
            "backend": Value::Null,
            "window": Value::Null,
            "autonomy": Value::Null,
            "session_id": Value::Null,
            "attention": attention,
            "headline": headline,
        }),
    }
}

/// Pick the orchestrator's active transcript out of an already-scanned, newest-first session list:
/// the newest with real message/tool activity (skips the content-less usage-probe sessions),
/// falling back to the newest overall. Pure — split out of [`pick_orchestrator_transcript`] so the
/// selection rule itself is unit-testable without touching the filesystem. Used only as the
/// unknown-session-id fallback (an adopted window) — see [`pick_orchestrator_transcript_in`].
fn pick_orchestrator_transcript_from(
    mut summaries: Vec<agent::TranscriptSummary>,
) -> Option<agent::TranscriptSummary> {
    if summaries.is_empty() {
        return None;
    }
    let idx = summaries
        .iter()
        .position(|s| s.last_message.is_some() || s.tool_call_count > 0)
        .unwrap_or(0);
    Some(summaries.swap_remove(idx))
}

/// The orchestrator's chosen transcript, given its own `session_id` (if known) and its `$HOME`.
/// `Some(id)`: a direct lookup of *that* session's transcript file
/// ([`agent::claude::transcript_for_session`]) — pinned regardless of what else is running on the
/// machine, so another active Claude session can never be misattributed as repomind's, however
/// much more recently it touched its own transcript. `None` (a window adopted from a prior daemon
/// lifetime whose session id this process never captured): falls back to the previous "newest
/// $HOME session with content across accounts" heuristic (see
/// [`pick_orchestrator_transcript_from`]) — the tracked `agent` can be stale after a restart+adopt
/// (it reflects config, not the running window's actual `CLAUDE_CONFIG_DIR`), and the ~empty
/// usage-probe sessions also run in `$HOME`, so neither the account nor plain recency is a
/// reliable selector on its own there. Split out from [`pick_orchestrator_transcript`] so tests can
/// drive it against a fixture `home` without mutating the process-global `HOME` env var.
fn pick_orchestrator_transcript_in(
    home: &Path,
    session_id: Option<&str>,
) -> Option<agent::TranscriptSummary> {
    if let Some(id) = session_id {
        return agent::claude::transcript_for_session(home, id);
    }
    let within = chrono::Duration::hours(SESSION_WINDOW_HOURS);
    let summaries = agent::claude::summaries_for(home, within, MAX_SESSIONS_PER_LANE);
    pick_orchestrator_transcript_from(summaries)
}

/// The orchestrator's chosen transcript for the real `$HOME` — see
/// [`pick_orchestrator_transcript_in`] for the selection rule. Shared by `orchestrator.transcript`
/// (the iOS chat view) and the notify-watch end-of-turn attention check. Blocking (reads/scans
/// `$HOME`) — call from `spawn_blocking`.
pub(crate) fn pick_orchestrator_transcript(
    session_id: Option<&str>,
) -> Option<agent::TranscriptSummary> {
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    pick_orchestrator_transcript_in(&home, session_id)
}

/// Drop a stale orchestrator session: if we think one is running but its tmux window is gone (killed
/// externally, or it `/exit`ed), clear the tracked session and broadcast the stopped status, so
/// `orchestrator.status` reads accurately and `orchestrator.start` re-spawns rather than no-op on a
/// corpse. Returns whether a session is still tracked afterward.
pub(crate) async fn reconcile_orchestrator(ctx: &Ctx) -> bool {
    if ctx.orchestrator.lock().await.is_none() {
        return false;
    }
    let tmux = ctx.tmux.clone();
    // On a probe failure keep the session: don't declare it dead on a transient tmux hiccup.
    let alive = tokio::task::spawn_blocking(move || tmux.has_named(ORCHESTRATOR_WINDOW))
        .await
        .unwrap_or(true);
    if alive {
        return true;
    }
    *ctx.orchestrator.lock().await = None;
    *ctx.orchestrator_attention.lock().await = ("none".to_string(), None);
    ctx.broadcast(
        crate::pubsub::topic::ORCHESTRATOR_STATUS,
        orchestrator_status_value(None, "none", None),
    );
    false
}

/// Which backend an orchestrator agent name runs on. `None` and Claude account variants are
/// Claude; a config custom is Claude too — its command line gets the Claude-shaped flags
/// [`build_claude_orchestrator_command`] appends, exactly as before backends existed (a
/// codex-shaped custom is future work). `codex` is the one non-Claude backend that can actually
/// drive the fleet (it has an MCP client). Anything else is a loud `invalid_params` — `aider`
/// and `cursor-agent` can't speak MCP, and an unknown name has no command — instead of what this
/// path used to do: silently spawn e.g. `aider --mcp-config …`, a broken window the user had to
/// diagnose by hand.
fn resolve_orchestrator_backend(
    agent: &Option<String>,
    customs: &HashMap<String, String>,
) -> Result<crate::OrchestratorBackend, RpcError> {
    use crate::OrchestratorBackend as B;
    let Some(name) = agent else {
        return Ok(B::Claude);
    };
    if customs.contains_key(name) {
        return Ok(B::Claude);
    }
    // `claude` itself and account variants (`claude-work`, …) parse as Other but are Claude; the
    // prefix test matches how the TUI's agent picker has always classified them.
    if name.starts_with("claude") {
        return Ok(B::Claude);
    }
    match AgentKind::from_kind_str(name) {
        AgentKind::Codex => Ok(B::Codex),
        _ => Err(RpcError::invalid_params(format!(
            "agent '{name}' can't run the orchestrator: repomind needs an MCP-capable CLI \
             (a claude account, codex, or a custom agent command)"
        ))),
    }
}

/// Resolve the orchestrator's base launch command from its agent name, mirroring `agent.spawn`: a
/// config custom wins, then an autodetected Claude variant (e.g. `claude-work` →
/// `CLAUDE_CONFIG_DIR=… claude`), else the kind's default binary (`codex` — anything else was
/// already rejected by [`resolve_orchestrator_backend`]). `None` (no agent chosen) is bare
/// `claude`.
fn orchestrator_base_command(agent: &Option<String>, customs: &HashMap<String, String>) -> String {
    match agent {
        Some(name) => {
            if let Some(c) = customs.get(name) {
                c.clone()
            } else if let Some((_, cmd)) = agent::claude::agent_variants()
                .into_iter()
                .find(|(n, _)| n == name)
            {
                cmd
            } else {
                AgentKind::from_kind_str(name).command().to_string()
            }
        }
        None => "claude".to_string(),
    }
}

/// Build the full `claude` invocation for the orchestrator, shell-quoted for `sh -c` (tmux runs
/// the window command through a shell). `--mcp-config` *adds* the repomon fleet server; the user's
/// own basic-memory (mnemind) server still loads from their Claude config, so we don't redeclare
/// it. The fleet + memory tools are pre-approved so routine orchestration doesn't prompt.
/// `session_id` pins the launched session's id (`--session-id <uuid>`, verified against `claude
/// --help` to exist) so the transcript picker can find *this* session's transcript directly
/// instead of guessing by recency — see [`pick_orchestrator_transcript_in`]. The Codex
/// counterpart is [`build_codex_orchestrator_command`].
fn build_claude_orchestrator_command(
    base: &str,
    mcp_config_path: &Path,
    model: &Option<String>,
    prompt: &Option<String>,
    session_id: &str,
) -> String {
    let mut command = base.to_string();
    command.push_str(" --mcp-config ");
    command.push_str(&shell_quote(&mcp_config_path.to_string_lossy()));
    command.push_str(" --append-system-prompt ");
    command.push_str(&shell_quote(repomon_mcp::PERSONA));
    command.push_str(" --allowedTools mcp__repomon,mcp__basic-memory");
    command.push_str(" --session-id ");
    command.push_str(&shell_quote(session_id));
    if let Some(model) = model {
        command.push_str(" --model ");
        command.push_str(&shell_quote(model));
    }
    if let Some(prompt) = prompt.as_deref().filter(|p| !p.is_empty()) {
        command.push(' ');
        command.push_str(&shell_quote(prompt));
    }
    command
}

/// Build the full `codex` invocation for the orchestrator, shell-quoted for `sh -c` (tmux runs
/// the window command through a shell). ALL Codex-CLI flag knowledge lives here (plus its unit
/// test), so a codex release changing a flag is a one-function fix. Verified against codex-cli
/// 0.142.3 (`codex --help`); where it diverges from the Claude arm:
/// - No `--mcp-config` file: the repomon fleet server is registered inline via `-c key=value`
///   dotted TOML overrides (`mcp_servers.repomon.*`; the value portion is parsed as TOML). The
///   user's own `~/.codex/config.toml` servers (e.g. basic-memory) still load — not redeclared,
///   mirroring the Claude arm's treatment.
/// - No `--append-system-prompt`: the repomind persona is prepended to the initial positional
///   prompt instead. Weaker than a real system prompt (visible in the chat, can fade over a very
///   long session) — if codex stabilizes an instructions-file override, swap it in here.
/// - No `--session-id` and no `--allowedTools`: codex can't pin its session file (and its
///   on-disk format is unstable anyway — the caller records `session_id: None`, and the
///   transcript/end-of-turn paths gate on `OrchestratorBackend::has_transcript`); tool
///   pre-approval is expressed through the approval policy below instead of a per-tool list.
/// - `autonomy` maps onto codex's approval/sandbox flags so routine MCP-driven orchestration
///   never stalls on an interactive approval. The REAL guardrail is `REPOMON_MCP_AUTONOMY`,
///   enforced server-side by `repomon_mcp::policy` from the env this hands the MCP server.
fn build_codex_orchestrator_command(
    base: &str,
    socket: &Path,
    autonomy: &str,
    max_agents: Option<usize>,
    model: &Option<String>,
    prompt: &Option<String>,
) -> String {
    let repomond = repomon_core::service::repomond_path();
    let mut command = base.to_string();
    // Interpolated straight into TOML basic strings: the paths this carries (the repomond binary,
    // the daemon socket) never contain quotes/backslashes on the platforms repomon ships for.
    let mut env = format!(
        "REPOMON_MCP_SOCKET = \"{}\", REPOMON_MCP_AUTONOMY = \"{autonomy}\"",
        socket.to_string_lossy(),
    );
    if let Some(n) = max_agents {
        env.push_str(&format!(", REPOMON_MCP_MAX_AGENTS = \"{n}\""));
    }
    for over in [
        format!(
            "mcp_servers.repomon.command=\"{}\"",
            repomond.to_string_lossy()
        ),
        "mcp_servers.repomon.args=[\"mcp\"]".to_string(),
        format!("mcp_servers.repomon.env={{ {env} }}"),
    ] {
        command.push_str(" -c ");
        command.push_str(&shell_quote(&over));
    }
    command.push_str(match autonomy {
        // Never stall on approvals; the sandbox still bounds what shell commands can touch.
        "autonomous" => " -a never -s workspace-write",
        // Codex decides when to ask; its dialogs surface through the pane attention sniff.
        "supervised" => " -a on-request",
        "read-only" => " -s read-only",
        // An unknown level gets the middle road rather than full autonomy.
        _ => " -a on-request",
    });
    if let Some(model) = model {
        command.push_str(" -m ");
        command.push_str(&shell_quote(model));
    }
    let goal = match prompt.as_deref().filter(|p| !p.is_empty()) {
        Some(p) => format!("{}\n\n{p}", repomon_mcp::PERSONA),
        None => repomon_mcp::PERSONA.to_string(),
    };
    command.push(' ');
    command.push_str(&shell_quote(&goal));
    command
}

/// Mint a fresh v4-shaped UUID for `--session-id`, without pulling in the `uuid` crate (no crate
/// in this workspace depends on it — see `Cargo.lock`). Mirrors the entropy pattern
/// `repomon_mcp::policy`'s `mint_confirm`/`random_token` use for its confirmation tokens: this
/// doesn't need to be cryptographically random, only fresh and correctly shaped — `claude
/// --session-id` merely needs a valid, presumably-unused UUID to key the orchestrator's own
/// transcript by.
fn mint_session_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    // Two independent hasher draws give the 128 bits a UUID needs; each seeds from a fresh
    // per-process `RandomState` plus the nanos/counter so two calls in the same nanosecond still
    // diverge.
    let mut h1 = RandomState::new().build_hasher();
    h1.write_u64(nanos ^ counter);
    let a = h1.finish();
    let mut h2 = RandomState::new().build_hasher();
    h2.write_u64(counter.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ nanos.rotate_left(17));
    let b = h2.finish();

    // Force the version (4) and variant (RFC 4122, `10xx`) nibbles so this always parses as a
    // well-formed UUID even though it isn't cryptographically random.
    let time_low = (a >> 32) as u32;
    let time_mid = (a >> 16) as u16;
    let time_hi_and_version = ((a as u16) & 0x0FFF) | 0x4000;
    let clock_seq = (((b >> 48) as u16) & 0x3FFF) | 0x8000;
    let node = b & 0xFFFF_FFFF_FFFF;
    format!("{time_low:08x}-{time_mid:04x}-{time_hi_and_version:04x}-{clock_seq:04x}-{node:012x}")
}

/// Write the orchestrator's `--mcp-config` file (registering the `repomon` stdio server pointed at
/// `repomond mcp` on `socket`), returning its path. The server's env carries the socket + autonomy
/// guardrails. Mirrors the logic that previously lived in `repomon orchestrate`.
fn write_orchestrator_mcp_config(
    socket: &Path,
    autonomy: &str,
    max_agents: Option<usize>,
) -> std::io::Result<PathBuf> {
    let repomond = repomon_core::service::repomond_path();
    let mut env = serde_json::Map::new();
    env.insert("REPOMON_MCP_SOCKET".into(), json!(socket.to_string_lossy()));
    env.insert("REPOMON_MCP_AUTONOMY".into(), json!(autonomy));
    if let Some(n) = max_agents {
        env.insert("REPOMON_MCP_MAX_AGENTS".into(), json!(n.to_string()));
    }
    let mcp_config = json!({
        "mcpServers": {
            "repomon": {
                "command": repomond.to_string_lossy(),
                "args": ["mcp"],
                "env": Value::Object(env),
            }
        }
    });
    let cfg_dir = repomon_core::config::config_dir();
    std::fs::create_dir_all(&cfg_dir)?;
    let path = cfg_dir.join("repomind-mcp.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&mcp_config).unwrap_or_default(),
    )?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_visibility_rules() {
        // total, alive, managed_n, fresh
        // A live agent (alive>=1) is kept; with several stale transcripts, only the live one(s).
        assert_eq!(sessions_to_keep(5, Some(1), 0, 0), 1);
        // No live process and nothing writing -> a /exit'ed session is dropped (the user's ask).
        assert_eq!(sessions_to_keep(5, Some(0), 0, 0), 0);
        // A session writing right now is kept even if the probe momentarily reads 0 (backstop).
        assert_eq!(sessions_to_keep(5, Some(0), 0, 1), 1);
        // A managed lane keeps its window count.
        assert_eq!(sessions_to_keep(3, Some(0), 1, 0), 1);
        // keep = max(alive, managed_n, fresh), capped at the number that exist.
        assert_eq!(sessions_to_keep(5, Some(2), 1, 3), 3);
        assert_eq!(sessions_to_keep(1, Some(5), 0, 0), 1);
        // A probe failure doesn't filter.
        assert_eq!(sessions_to_keep(2, None, 0, 0), 2);
        assert_eq!(sessions_to_keep(0, Some(0), 0, 0), 0);
    }

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
            resolve_windows(
                Ok(vec!["lane-1".into(), "lane-2".into()]),
                &mut last,
                &mut misses
            ),
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
        assert_eq!(
            resolve_windows(Ok(vec![]), &mut last, &mut misses),
            Vec::<String>::new()
        );
        assert!(last.is_empty());
        // A subsequent successful probe resets the counter.
        resolve_windows(Ok(vec!["lane-9".into()]), &mut last, &mut misses);
        assert_eq!(misses, 0);
    }

    #[test]
    fn resolve_windows_accepts_empty_immediately_once_last_good_is_reconciled() {
        // This is the effect `reap::kill_and_forget` buys `agent.stop`: proactively dropping the
        // just-killed window from `last_good` (rather than waiting for the reaper/next probe to
        // notice on its own) means the very next genuinely-empty probe isn't mistaken for the
        // total-vanish-debounce case in `resolve_windows_rides_out_a_one_tick_total_vanish`
        // above — it's accepted at once, so a stopped agent's window can't be read back as still
        // live for even one extra tick.
        let mut last: Vec<String> = vec!["lane-1".into()];
        let mut misses = 0u8;
        last.retain(|w| w != "lane-1"); // what `kill_and_forget` does synchronously on kill
        assert_eq!(
            resolve_windows(Ok(vec![]), &mut last, &mut misses),
            Vec::<String>::new()
        );
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
        // The default account's launch UNSETS the var via `env -u` — still resolves to claude.
        assert_eq!(
            program_of("env -u CLAUDE_CONFIG_DIR claude"),
            Some("claude")
        );
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
    fn command_account_normalizes_pinned_default_and_strips_quotes() {
        // The default account launches with `env -u CLAUDE_CONFIG_DIR claude` (no `CLAUDE_CONFIG_DIR=`
        // prefix), so it reads back as the *default* account (None).
        assert_eq!(command_account("env -u CLAUDE_CONFIG_DIR claude"), None);
        // A hand-written pin to the default base also normalizes to the default account (defensive).
        let default = agent::claude::default_config_base();
        assert_eq!(
            command_account(&format!("CLAUDE_CONFIG_DIR={} claude", default.display())),
            None
        );
        // shell_quote wraps the path in single quotes; the parse must see through them — both for
        // a variant account...
        assert_eq!(
            command_account("CLAUDE_CONFIG_DIR='/h/.claude-work' claude"),
            Some(PathBuf::from("/h/.claude-work"))
        );
        // ...and for a quoted default base (still the default account).
        assert_eq!(
            command_account(&format!("CLAUDE_CONFIG_DIR='{}' claude", default.display())),
            None
        );
        // A shell-quoted config dir containing spaces must parse as one whole path, not split on
        // the inner space (shell_quote wraps it, so command_account must honor the quoting).
        assert_eq!(
            command_account("CLAUDE_CONFIG_DIR='/h/with a space/.claude-work' claude"),
            Some(PathBuf::from("/h/with a space/.claude-work"))
        );
    }

    #[test]
    fn builtins_are_recognized() {
        // claude-code is always present (the default config dir is always listed).
        assert!(is_builtin("claude-code"));
        assert!(is_builtin("codex"));
        assert!(!is_builtin("claude-yolo"));
    }

    #[test]
    fn orchestrator_base_resolves_agent() {
        let mut customs = HashMap::new();
        customs.insert(
            "claude-yolo".to_string(),
            "claude --dangerously-skip-permissions".to_string(),
        );
        // No agent chosen -> bare claude (the default backend).
        assert_eq!(orchestrator_base_command(&None, &customs), "claude");
        // A custom agent resolves to its configured command (flags carried over).
        assert_eq!(
            orchestrator_base_command(&Some("claude-yolo".into()), &customs),
            "claude --dangerously-skip-permissions"
        );
        // A kind name resolves to its default binary (mirrors agent.spawn); for the orchestrator
        // only `codex` reaches here — anything non-MCP-capable is rejected upstream by
        // `resolve_orchestrator_backend`.
        assert_eq!(
            orchestrator_base_command(&Some("codex".into()), &customs),
            "codex"
        );
    }

    #[test]
    fn orchestrator_backend_resolution() {
        use crate::OrchestratorBackend as B;
        let mut customs = HashMap::new();
        customs.insert("my-yolo".to_string(), "claude --yolo".to_string());
        // Default and every claude-ish name → Claude.
        assert_eq!(
            resolve_orchestrator_backend(&None, &customs).unwrap(),
            B::Claude
        );
        for name in ["claude", "claude-code", "claude-work"] {
            assert_eq!(
                resolve_orchestrator_backend(&Some(name.into()), &customs).unwrap(),
                B::Claude,
                "{name}"
            );
        }
        // A config custom is Claude-flag-shaped regardless of its name.
        assert_eq!(
            resolve_orchestrator_backend(&Some("my-yolo".into()), &customs).unwrap(),
            B::Claude
        );
        // Codex is the one non-Claude backend.
        assert_eq!(
            resolve_orchestrator_backend(&Some("codex".into()), &customs).unwrap(),
            B::Codex
        );
        // MCP-less agents and unknown names are loud errors, not broken spawns.
        for name in ["aider", "cursor", "gemini"] {
            let err = resolve_orchestrator_backend(&Some(name.into()), &customs).unwrap_err();
            assert!(
                err.message.contains(name) && err.message.contains("orchestrator"),
                "{name}: {}",
                err.message
            );
        }
    }

    #[test]
    fn codex_orchestrator_command_wires_mcp_and_persona() {
        let socket = PathBuf::from("/tmp/repomon-test.sock");
        // Autonomous: MCP registration inline, approvals off, sandboxed, persona as the prompt.
        let cmd =
            build_codex_orchestrator_command("codex", &socket, "autonomous", Some(4), &None, &None);
        assert!(cmd.starts_with("codex "), "{cmd}");
        // The three -c overrides registering the fleet MCP server.
        assert!(cmd.contains("mcp_servers.repomon.command="), "{cmd}");
        assert!(cmd.contains("mcp_servers.repomon.args=[\"mcp\"]"), "{cmd}");
        assert!(
            cmd.contains("REPOMON_MCP_SOCKET = \"/tmp/repomon-test.sock\""),
            "{cmd}"
        );
        assert!(
            cmd.contains("REPOMON_MCP_AUTONOMY = \"autonomous\""),
            "{cmd}"
        );
        assert!(cmd.contains("REPOMON_MCP_MAX_AGENTS = \"4\""), "{cmd}");
        assert!(cmd.contains(" -a never -s workspace-write"), "{cmd}");
        // The persona rides in as the initial prompt (codex has no --append-system-prompt).
        assert!(cmd.contains("repomind"), "{cmd}");
        // None of the Claude-only flags may leak into a codex invocation.
        for claude_flag in [
            "--mcp-config",
            "--append-system-prompt",
            "--allowedTools",
            "--session-id",
            "--model",
        ] {
            assert!(!cmd.contains(claude_flag), "{claude_flag} leaked: {cmd}");
        }

        // Supervised + model + prompt: on-request approvals, -m, prompt appended to the persona.
        let cmd = build_codex_orchestrator_command(
            "codex",
            &socket,
            "supervised",
            None,
            &Some("gpt-5.2-codex".into()),
            &Some("what needs me?".into()),
        );
        assert!(cmd.contains(" -a on-request"), "{cmd}");
        assert!(!cmd.contains("REPOMON_MCP_MAX_AGENTS"), "{cmd}");
        assert!(cmd.contains(" -m 'gpt-5.2-codex'"), "{cmd}");
        assert!(cmd.contains("what needs me?"), "{cmd}");

        // Read-only maps to codex's read-only sandbox.
        let cmd =
            build_codex_orchestrator_command("codex", &socket, "read-only", None, &None, &None);
        assert!(cmd.contains(" -s read-only"), "{cmd}");
    }

    #[test]
    fn orchestrator_command_wires_mcp_persona_and_tools() {
        let path = PathBuf::from("/tmp/repomind-mcp.json");
        let sid = "11111111-1111-4111-8111-111111111111";
        // No model, no prompt: the core wiring is always present.
        let cmd = build_claude_orchestrator_command("claude", &path, &None, &None, sid);
        assert!(cmd.starts_with("claude --mcp-config "));
        assert!(cmd.contains("/tmp/repomind-mcp.json"));
        assert!(cmd.contains("--append-system-prompt"));
        assert!(cmd.contains("--allowedTools mcp__repomon,mcp__basic-memory"));
        // The persona is appended (a recognizable line from it survives the quoting).
        assert!(cmd.contains("repomind"));
        // The session id is always pinned.
        assert!(cmd.contains(&format!("--session-id '{sid}'")));
        // No model flag when none is requested.
        assert!(!cmd.contains("--model"));

        // A model + a prompt are appended (shell-quoted).
        let cmd = build_claude_orchestrator_command(
            "CLAUDE_CONFIG_DIR=/h/.claude-work claude",
            &path,
            &Some("opus".into()),
            &Some("what needs me?".into()),
            sid,
        );
        assert!(cmd.starts_with("CLAUDE_CONFIG_DIR=/h/.claude-work claude "));
        assert!(cmd.contains("--model 'opus'"));
        assert!(cmd.contains("'what needs me?'"));
        assert!(cmd.contains(&format!("--session-id '{sid}'")));

        // An empty prompt is dropped (not quoted as an empty arg).
        let cmd =
            build_claude_orchestrator_command("claude", &path, &None, &Some(String::new()), sid);
        assert!(!cmd.trim_end().ends_with("''"));
    }

    #[test]
    fn mint_session_id_is_a_well_formed_v4_uuid() {
        // `claude --session-id` rejects anything that isn't a valid UUID (verified live against
        // `claude --help`, which documents the flag) — so the minted id must always parse as one:
        // 8-4-4-4-12 hex groups, version nibble `4`, variant nibble in `8..=b`. Two draws must
        // also differ (a repeated id would collide with a still-live session's transcript file).
        let a = mint_session_id();
        let b = mint_session_id();
        assert_ne!(a, b, "two mints must not collide");
        for id in [&a, &b] {
            let parts: Vec<&str> = id.split('-').collect();
            assert_eq!(parts.len(), 5, "not 5 hyphen groups: {id}");
            assert_eq!(
                [
                    parts[0].len(),
                    parts[1].len(),
                    parts[2].len(),
                    parts[3].len(),
                    parts[4].len()
                ],
                [8, 4, 4, 4, 12],
                "wrong group lengths: {id}"
            );
            assert!(
                parts
                    .iter()
                    .all(|p| p.chars().all(|c| c.is_ascii_hexdigit())),
                "non-hex digit: {id}"
            );
            assert_eq!(
                parts[2].chars().next(),
                Some('4'),
                "version nibble must be 4: {id}"
            );
            assert!(
                matches!(parts[3].chars().next(), Some('8' | '9' | 'a' | 'b')),
                "variant nibble must be 8..=b: {id}"
            );
        }
    }

    #[test]
    fn orchestrator_status_shapes() {
        // Running session reports its fields, plus the attention/headline passed in.
        let s = crate::OrchestratorSession {
            agent: Some("claude-work".into()),
            model: Some("opus".into()),
            window: "orchestrator".into(),
            autonomy: Some("autonomous".into()),
            session_id: Some("11111111-1111-4111-8111-111111111111".into()),
            backend: crate::OrchestratorBackend::Claude,
        };
        let v = orchestrator_status_value(Some(&s), "decision", Some("Which auth method?"));
        assert_eq!(v["running"], json!(true));
        assert_eq!(v["agent"], json!("claude-work"));
        assert_eq!(v["model"], json!("opus"));
        assert_eq!(v["backend"], json!("claude"));
        assert_eq!(v["window"], json!("orchestrator"));
        assert_eq!(v["autonomy"], json!("autonomous"));
        assert_eq!(
            v["session_id"],
            json!("11111111-1111-4111-8111-111111111111")
        );
        assert_eq!(v["attention"], json!("decision"));
        assert_eq!(v["headline"], json!("Which auth method?"));
        // An adopted session's autonomy AND session id are both unknown; a codex-backed session
        // reports its backend so clients switch off the (empty) transcript chat view.
        let adopted = crate::OrchestratorSession {
            agent: Some("codex".into()),
            model: None,
            window: "orchestrator".into(),
            autonomy: None,
            session_id: None,
            backend: crate::OrchestratorBackend::Codex,
        };
        let v = orchestrator_status_value(Some(&adopted), "none", None);
        assert_eq!(v["autonomy"], Value::Null);
        assert_eq!(v["session_id"], Value::Null);
        assert_eq!(v["backend"], json!("codex"));
        // No session: running=false with null fields; attention/headline still pass through.
        let v = orchestrator_status_value(None, "none", None);
        assert_eq!(v["running"], json!(false));
        assert_eq!(v["agent"], Value::Null);
        assert_eq!(v["backend"], Value::Null);
        assert_eq!(v["autonomy"], Value::Null);
        assert_eq!(v["session_id"], Value::Null);
        assert_eq!(v["attention"], json!("none"));
        assert_eq!(v["headline"], Value::Null);
    }

    #[test]
    fn stall_needs_frozen_pane_and_an_unfinished_turn() {
        use repomon_core::model::AgentStatus::*;
        let now = chrono::Utc::now();
        let old = now - chrono::Duration::minutes(6);
        let fresh = now - chrono::Duration::minutes(1);

        // Frozen mid-work — Running (transcript ends in a tool call) or the post-10-min Idle
        // decay of the same shape: stalled, anchored on the pane's last change.
        assert_eq!(
            stall_since(Running, false, false, Some(old), now),
            Some(old)
        );
        assert_eq!(stall_since(Idle, false, false, Some(old), now), Some(old));
        // The pane is still moving: not stalled.
        assert_eq!(stall_since(Running, false, false, Some(fresh), now), None);
        // A dialog is up (waiting on you): never a stall.
        assert_eq!(stall_since(Running, false, true, Some(old), now), None);
        // The turn ended (waiting for instructions, however long ago): never a stall.
        assert_eq!(stall_since(Idle, true, false, Some(old), now), None);
        assert_eq!(stall_since(Waiting, true, false, Some(old), now), None);
        // Rate-limited is timer-owned, not stuck.
        assert_eq!(stall_since(RateLimited, false, false, Some(old), now), None);
        // No pane observation yet: can't call it.
        assert_eq!(stall_since(Running, false, false, None, now), None);
    }

    #[test]
    fn picks_newest_transcript_with_content_else_newest_overall() {
        fn stub(last_message: Option<&str>, tool_calls: u32) -> agent::TranscriptSummary {
            agent::TranscriptSummary {
                kind: repomon_core::model::AgentKind::ClaudeCode,
                manifest_path: PathBuf::from("/tmp/x.jsonl"),
                cwd: None,
                last_activity: chrono::Utc::now(),
                tool_call_count: tool_calls,
                status: repomon_core::model::AgentStatus::Idle,
                title: None,
                last_message: last_message.map(str::to_string),
                config_dir: None,
                session_id: None,
                ended_turn: false,
            }
        }
        // The newest (first) entry is a content-less usage-probe session; skip it for the next
        // one that actually has a message.
        let picked =
            pick_orchestrator_transcript_from(vec![stub(None, 0), stub(Some("hi"), 0)]).unwrap();
        assert_eq!(picked.last_message.as_deref(), Some("hi"));
        // A tool call with no message still counts as "real content".
        let picked = pick_orchestrator_transcript_from(vec![stub(None, 0), stub(None, 3)]).unwrap();
        assert_eq!(picked.tool_call_count, 3);
        // Nothing has content: fall back to the newest (first) overall.
        let picked = pick_orchestrator_transcript_from(vec![stub(None, 0), stub(None, 0)]).unwrap();
        assert!(picked.last_message.is_none());
        // No sessions at all: None.
        assert!(pick_orchestrator_transcript_from(vec![]).is_none());
    }

    #[test]
    fn pick_orchestrator_transcript_in_pins_to_session_id_else_falls_back_to_newest() {
        // Reproduces the live-verified misattribution: an "unrelated" Claude session (some other
        // active session on the machine) touches its transcript AFTER the orchestrator's own,
        // making it the newest — a recency-only picker would return the wrong one. `Some(id)` must
        // still pick the orchestrator's own (older) transcript by id; only `None` (an adopted
        // window with no known id) falls back to the old newest-wins heuristic.
        let root = tempfile::tempdir().unwrap();
        let home = PathBuf::from("/Users/fixture-home");
        let dir = root.path().join(agent::claude::encode_project_dir(&home));
        std::fs::create_dir_all(&dir).unwrap();
        let line = |text: &str| {
            format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{text}"}}]}}}}"#
            )
        };

        // The orchestrator's own session, written first (older mtime).
        std::fs::write(
            dir.join("orchestrator-session-id.jsonl"),
            line("repomind's own turn"),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        // An unrelated Claude session, written after — newer mtime, so a pure recency scan would
        // wrongly prefer it.
        std::fs::write(
            dir.join("unrelated-session-id.jsonl"),
            line("some other session's turn"),
        )
        .unwrap();

        // SAFETY: single-threaded test; nothing else reads the environment here.
        unsafe { std::env::set_var("REPOMON_CLAUDE_PROJECTS", root.path()) };
        let pinned = pick_orchestrator_transcript_in(&home, Some("orchestrator-session-id"));
        let fallback = pick_orchestrator_transcript_in(&home, None);
        // SAFETY: single-threaded test; nothing else reads the environment here.
        unsafe { std::env::remove_var("REPOMON_CLAUDE_PROJECTS") };

        assert_eq!(
            pinned
                .expect("orchestrator's own transcript is found by id")
                .session_id
                .as_deref(),
            Some("orchestrator-session-id"),
            "Some(id) must pick the orchestrator's own transcript, not the newer unrelated one"
        );
        assert_eq!(
            fallback
                .expect("newest-overall fallback still finds a transcript")
                .session_id
                .as_deref(),
            Some("unrelated-session-id"),
            "None (adopted, unknown id) must keep the existing newest-wins behavior"
        );
    }
}
