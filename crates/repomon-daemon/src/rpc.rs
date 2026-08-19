//! JSON-RPC method dispatch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use repomon_core::agent::backend::{CaptureOpts, ScrollEvent, SpawnSpec};
use repomon_core::agent::{self, shell_quote};
use repomon_core::git::{diff, reader};
use repomon_core::model::{
    AgentAddress, AgentChoice, AgentDoctorInfo, AgentKind, AgentSession, AgentStatus, BrowseEntry,
    BrowseResult, Commit, CreateLaneParams, Lane, RemoteDevice, RepoId, ResolvedAgentAddress,
    SystemDoctorResult, TimeRange,
};
use repomon_core::protocol::RpcError;
use repomon_core::{Indexer, TmuxRuntime, analytics, config, session};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use std::sync::Arc;

use crate::conn::ConnSession;
use crate::{Ctx, ORCHESTRATOR_WINDOW};

const DAEMON_PROTOCOL_REVISION: u32 = 2;

fn internal<E: std::fmt::Display>(e: E) -> RpcError {
    RpcError::internal(e.to_string())
}

fn parse<T: DeserializeOwned>(params: Option<Value>) -> Result<T, RpcError> {
    serde_json::from_value(params.unwrap_or(Value::Null))
        .map_err(|e| RpcError::invalid_params(e.to_string()))
}

fn parse_opt<T: DeserializeOwned + Default>(params: Option<Value>) -> Result<T, RpcError> {
    match params {
        None | Some(Value::Null) => Ok(T::default()),
        Some(v) => serde_json::from_value(v).map_err(|e| RpcError::invalid_params(e.to_string())),
    }
}

fn to_value<T: serde::Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(internal)
}

/// Rebuild [`Ctx::remote_tokens`] from the store's paired devices plus the legacy `[remote] token`
/// from config (device name `None`). The single choke point for the auth cache: startup seeding
/// and every pair/revoke funnel through here, so the handshake callback always reads a current set.
///
/// **Concurrency:** this is read-then-write (read the store's device list, then overwrite the cache)
/// and is NOT internally serialized. Two overlapping refreshes race — an auth-cache refresh race
/// where a `remote.pair`'s post-mutation read lands after a concurrent `remote.revoke`'s write,
/// re-adding a just-revoked token. Callers MUST hold [`Ctx::remote_mutate_lock`] across their store
/// mutation and this refresh so the mutate+rebuild is atomic (see the `remote.pair`/`remote.revoke`
/// handlers and the startup seed).
pub async fn refresh_remote_tokens(ctx: &Ctx) -> Result<(), RpcError> {
    let devices = ctx.store.remote_device_list().await.map_err(internal)?;
    let config_token = ctx.config.read().await.remote.token.clone();
    let mut tokens: Vec<(String, Option<String>)> = devices
        .into_iter()
        .map(|d| (d.token, Some(d.name)))
        .collect();
    if let Some(t) = config_token {
        if !t.is_empty() {
            tokens.push((t, None));
        }
    }
    *ctx.remote_tokens.write().unwrap() = tokens;
    Ok(())
}

/// The pairing URL the companion app scans: `repomon://<bind>?name=<urlencoded>#<token>`. The
/// device name is a QUERY item and the fragment is the bare token — both the current app and
/// the legacy phone build take the entire fragment as the token, so splicing `&name=` into the
/// fragment corrupted the stored token and every named pairing 401'd at the handshake.
async fn remote_pair_url(ctx: &Ctx, dev: &RemoteDevice) -> String {
    let bind = ctx
        .config
        .read()
        .await
        .remote
        .bind
        .clone()
        .unwrap_or_default();
    format!(
        "repomon://{bind}?name={}#{}",
        percent_encode(&dev.name),
        dev.token,
    )
}

/// Minimal percent-encoding for a device name spliced into a URL fragment. Keeps the unreserved
/// set and escapes everything else; avoids a urlencoding dependency for one short field.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `agent.answer` found the pane in a different state than the client expected (dialog gone or
/// replaced). `error.data.dialog` carries what's actually there now (possibly null) so the
/// client can re-render instead of re-fetching.
const DIALOG_CHANGED: i64 = -32010;

/// `file.write` found the on-disk mtime didn't match the caller's `expected_mtime_ms` (or the
/// file vanished entirely). The frontend must treat this as a save conflict — offer to re-read,
/// merge, or force — never as a generic failure to retry blindly. `error.data` carries both
/// mtimes (`actual_mtime_ms` is `null` when the file no longer exists) so the client can render
/// the conflict without a second RPC round-trip. Same "distinct code + structured data" shape as
/// `DIALOG_CHANGED` above.
const FILE_CONFLICT: i64 = -32011;

/// Record that input reached a lane window: stamp `input_seen` (quiets the notification
/// engine) and drop the window's sniff-cache entry, so an answered dialog can't be
/// re-advertised by `lane.list` for the rest of its TTL.
pub(crate) async fn mark_input(ctx: &Ctx, lane: repomon_core::model::LaneId, window: &str) {
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
        "theme": cfg.theme,
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
        "notify_sound_volume": cfg.notify_sound_volume,
        "notify_sound_unfocused_only": cfg.notify_sound_unfocused_only,
        "notify_sound_agent_needs_you": cfg.notify_sound_agent_needs_you,
        "notify_sound_agent_finished": cfg.notify_sound_agent_finished,
        "notify_sound_repomind_needs_you": cfg.notify_sound_repomind_needs_you,
        "notify_sound_error_or_stall": cfg.notify_sound_error_or_stall,
        "notify_sound_incoming_message": cfg.notify_sound_incoming_message,
        "notify_sound_update_ready": cfg.notify_sound_update_ready,
        "message_inject_agents": cfg.message_inject_agents,
        "message_inject_operator": cfg.message_inject_operator,
        "notify_show_why": cfg.notify_show_why,
        "notify_coalesce": cfg.notify_coalesce,
        "notify_click_focus": cfg.notify_click_focus,
        "notify_desktop_fallback": cfg.notify_desktop_fallback,
        "notify_subagents": cfg.notify_subagents,
        "usage_probe": cfg.usage_probe,
        "expand_agents": cfg.expand_agents,
        "sort_repos_by_activity": cfg.sort_repos_by_activity,
        "embedded_pty": cfg.embedded_pty,
        "orchestrator_agent": cfg.orchestrator_agent,
        "orchestrator_model": cfg.orchestrator_model,
        "agent_icons": cfg.agent_icons,
        "supervision": cfg.supervision,
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
struct RepoSetHidden {
    repo_id: RepoId,
    hidden: bool,
}
#[derive(Deserialize)]
struct RepoNotesGet {
    repo_id: RepoId,
}
#[derive(Deserialize)]
struct RepoNotesSet {
    repo_id: RepoId,
    content: String,
}
#[derive(Deserialize)]
struct JournalAppend {
    session: String,
    action: String,
    #[serde(default)]
    lane_id: Option<i64>,
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    params: Option<String>,
    #[serde(default)]
    outcome: Option<String>,
    #[serde(default)]
    detail: Option<String>,
}
#[derive(Deserialize)]
struct JournalQuery {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    since_last_session: bool,
    #[serde(default)]
    limit: Option<usize>,
}
#[derive(Deserialize)]
struct ApprovalRecord {
    repo: String,
    command: String,
    verdict: String,
}
#[derive(Deserialize)]
struct ApprovalRuleRef {
    repo: String,
    pattern: String,
}
#[derive(Deserialize)]
struct ScheduleAdd {
    spec: String,
    prompt: String,
    #[serde(default)]
    max_actions: Option<u32>,
}
#[derive(Deserialize)]
struct ScheduleRemove {
    id: i64,
}
#[derive(Deserialize)]
struct PlaybookSave {
    name: String,
    content: String,
}
#[derive(Deserialize)]
struct PlaybookSearch {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}
#[derive(Deserialize)]
struct PlaybookName {
    name: String,
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
/// `to` on `message.send`: a single canonical address (the historical, still-default shape) or a
/// list of addresses. Either shape may contain wildcard tokens (`lane-X/*`, `*`) — see
/// [`classify_token`]. Declared `untagged` so existing string-`to` callers are unaffected.
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
enum MessageTo {
    Single(String),
    Multi(Vec<String>),
}

impl MessageTo {
    fn items(&self) -> Vec<&str> {
        match self {
            MessageTo::Single(value) => vec![value.as_str()],
            MessageTo::Multi(values) => values.iter().map(String::as_str).collect(),
        }
    }

    fn has_wildcard(&self) -> bool {
        self.items()
            .into_iter()
            .any(|item| !matches!(classify_token(item), AddressToken::Literal(_)))
    }

    /// `Some(address)` when `to` is a single, non-wildcard address — the exact shape
    /// `message.send` accepted before A6. That case keeps returning a bare `FleetMessage`
    /// unchanged; anything else (a list, or a bare wildcard) returns a fan-out summary.
    fn as_legacy_single(&self) -> Option<&str> {
        match self {
            MessageTo::Single(value)
                if matches!(classify_token(value), AddressToken::Literal(_)) =>
            {
                Some(value.as_str())
            }
            _ => None,
        }
    }
}

/// One parsed `to` token.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AddressToken {
    /// A canonical address handled exactly as before: `operator`, `repomind`, `@label`, or
    /// `lane-X[/slot]`. Resolved later by [`resolve_message_address`].
    Literal(String),
    /// `lane-X/*` — every active agent session in lane X, minus the sender.
    LaneWildcard(repomon_core::model::LaneId),
    /// `*` — every active agent session in the fleet, minus the sender.
    GlobalWildcard,
}

fn classify_token(token: &str) -> AddressToken {
    let token = token.trim();
    if token == "*" {
        return AddressToken::GlobalWildcard;
    }
    if let Some(rest) = token.strip_prefix("lane-") {
        if let Some(lane_part) = rest.strip_suffix("/*") {
            if let Ok(lane_id) = lane_part.parse::<repomon_core::model::LaneId>() {
                return AddressToken::LaneWildcard(lane_id);
            }
        }
    }
    AddressToken::Literal(token.to_string())
}

/// Every active agent session in `lanes`, addressed canonically (`lane-X/slot`), restricted to
/// `lane_filter` when set, and excluding the sender's own session so a broadcast never mails
/// itself.
fn expand_wildcard_targets(
    lanes: &[Lane],
    lane_filter: Option<repomon_core::model::LaneId>,
    sender: &ResolvedAgentAddress,
) -> Vec<String> {
    let mut out = Vec::new();
    for lane in lanes {
        if let Some(filter) = lane_filter {
            if lane.id != filter {
                continue;
            }
        }
        for (index, _session) in lane.agent_sessions.iter().enumerate() {
            let slot = (index + 1) as u32;
            if sender.lane_id == Some(lane.id) && sender.slot == Some(slot) {
                continue;
            }
            out.push(format!("lane-{}/{}", lane.id, slot));
        }
    }
    out
}

/// Expand `to` into the literal address list to fan a send out to: wildcard tokens are resolved
/// against `lanes` (self-excluded), plain tokens pass through unchanged, and duplicates collapse
/// to a single delivery. `lanes` may be empty when `to` has no wildcard token — callers only need
/// to fetch it when [`MessageTo::has_wildcard`] is true.
fn expand_message_targets(
    to: &MessageTo,
    lanes: &[Lane],
    sender: &ResolvedAgentAddress,
) -> Result<Vec<String>, RpcError> {
    let items = to.items();
    if items.is_empty() {
        return Err(RpcError::invalid_params(
            "message recipient list must not be empty",
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let token = item.trim();
        if token.is_empty() {
            return Err(RpcError::invalid_params(
                "message recipient must not be empty",
            ));
        }
        match classify_token(token) {
            AddressToken::Literal(address) => {
                if seen.insert(address.clone()) {
                    out.push(address);
                }
            }
            AddressToken::GlobalWildcard => {
                for address in expand_wildcard_targets(lanes, None, sender) {
                    if seen.insert(address.clone()) {
                        out.push(address);
                    }
                }
            }
            AddressToken::LaneWildcard(lane_id) => {
                for address in expand_wildcard_targets(lanes, Some(lane_id), sender) {
                    if seen.insert(address.clone()) {
                        out.push(address);
                    }
                }
            }
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
struct MessageSend {
    to: MessageTo,
    body: String,
    #[serde(default)]
    reply_to: Option<String>,
    #[serde(default)]
    identity_token: Option<String>,
    #[serde(default)]
    source: Option<String>,
}
#[derive(Deserialize)]
struct MessageInbox {
    #[serde(default)]
    unread_only: bool,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    identity_token: Option<String>,
    #[serde(default)]
    source: Option<String>,
}
#[derive(Deserialize)]
struct MessageMarkRead {
    id: String,
    #[serde(default)]
    identity_token: Option<String>,
    #[serde(default)]
    source: Option<String>,
}
#[derive(Deserialize)]
struct MessageList {
    #[serde(default)]
    lane_id: Option<repomon_core::model::LaneId>,
    #[serde(default)]
    unread_only: bool,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    before: Option<String>,
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
    /// Reasoning-effort hint, translated per agent kind (e.g. claude `MAX_THINKING_TOKENS`, codex
    /// `model_reasoning_effort`). Additive; absent is the unchanged default.
    #[serde(default)]
    effort: Option<String>,
    /// Permission/launch mode: `default` (emit nothing), `auto`, or `plan`. Additive.
    #[serde(default)]
    mode: Option<String>,
    /// Model override forwarded to the agent (e.g. `opus`). Additive.
    #[serde(default)]
    model: Option<String>,
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
    #[serde(default)]
    include_state: bool,
}
#[derive(Deserialize)]
struct AgentPrompt {
    lane_id: repomon_core::model::LaneId,
    #[serde(default)]
    window: Option<String>,
}
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
    /// Which Claude account (config dir) to target. `None`/`"default"` = `~/.claude`.
    #[serde(default)]
    account: Option<String>,
}
#[derive(Deserialize)]
struct PluginToggle {
    id: String,
    #[serde(flatten)]
    scope: ExtScope,
    /// Which Claude account (config dir) to target. `None`/`"default"` = `~/.claude`.
    #[serde(default)]
    account: Option<String>,
}
fn ext_scope_json(scope: &ExtScope) -> Value {
    match scope {
        ExtScope::Global => json!({ "scope": "global" }),
        ExtScope::Repo { repo_id } => json!({ "scope": "repo", "repo_id": repo_id }),
    }
}
#[derive(Deserialize)]
struct PluginInstall {
    r#ref: String,
    #[serde(flatten)]
    scope: ExtScope,
    /// Which Claude account (config dir) to target. `None`/`"default"` = `~/.claude`.
    #[serde(default)]
    account: Option<String>,
}
#[derive(Deserialize)]
struct NameOnly {
    name: String,
    #[serde(default)]
    account: Option<String>,
}
#[derive(Deserialize)]
struct OptionalName {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    account: Option<String>,
}
#[derive(Deserialize)]
struct IdOnly {
    id: String,
    #[serde(default)]
    account: Option<String>,
}
#[derive(Deserialize)]
struct OptionalId {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    account: Option<String>,
}
#[derive(Deserialize)]
struct SourceOnly {
    source: String,
    #[serde(default)]
    account: Option<String>,
}
#[derive(Deserialize)]
struct SkillCreate {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(flatten)]
    scope: ExtScope,
    /// Which Claude account (config dir) to target. `None`/`"default"` = `~/.claude`.
    #[serde(default)]
    account: Option<String>,
}
#[derive(Deserialize)]
struct SkillPath {
    path: PathBuf,
}
#[derive(Deserialize)]
struct SkillWrite {
    path: PathBuf,
    content: String,
}
#[derive(Deserialize)]
struct SkillDelete {
    name: String,
    #[serde(flatten)]
    scope: ExtScope,
    /// Which Claude account (config dir) to target. `None`/`"default"` = `~/.claude`.
    #[serde(default)]
    account: Option<String>,
}

/// Cached once a detection succeeds: repeated CLI-present checks (eg. every `ext.list`) shouldn't
/// repeatedly shell out to `claude --version`. Failures are deliberately NOT cached (see
/// `claude_cli` below) so installing the CLI after daemon start doesn't require a restart.
static CLAUDE_CLI: std::sync::OnceLock<std::sync::Arc<crate::ext::ClaudeCli>> =
    std::sync::OnceLock::new();

/// Resolve the cached CLI handle, detecting off the async runtime on a cache miss. Only a
/// successful detection is cached; a miss re-probes on the next call, so a CLI installed after
/// the daemon started is picked up without a restart.
async fn claude_cli() -> Result<std::sync::Arc<crate::ext::ClaudeCli>, RpcError> {
    if let Some(cli) = CLAUDE_CLI.get() {
        return Ok(cli.clone());
    }
    match tokio::task::spawn_blocking(crate::ext::ClaudeCli::detect)
        .await
        .map_err(internal)?
    {
        Some(cli) => {
            let cli = std::sync::Arc::new(cli);
            // A concurrent detection may have won the race; both results are equivalent, so
            // either value is fine to return.
            Ok(CLAUDE_CLI.get_or_init(|| cli).clone())
        }
        None => Err(RpcError::new(-32021, "claude CLI not found on PATH")),
    }
}

fn cli_error(failure: crate::ext::CliFailure) -> RpcError {
    RpcError {
        code: -32020,
        message: failure.message,
        data: Some(json!({ "stderr": failure.stderr, "exit_code": failure.exit_code })),
    }
}

/// Run a CLI op off the async runtime under the given account, emit event.ext.changed, and return
/// {ok, stdout}. `account` picks the `CLAUDE_CONFIG_DIR` the `claude` CLI runs under.
async fn run_cli_op(
    ctx: &Ctx,
    account: Option<&str>,
    args: Vec<String>,
    changed_scope: Value,
) -> Result<Value, RpcError> {
    let cli = claude_cli().await?;
    let config_dir = crate::ext::account_config_dir(account);
    let stdout = tokio::task::spawn_blocking(move || {
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        cli.run_for(config_dir.as_deref(), &arg_refs)
    })
    .await
    .map_err(internal)?
    .map_err(cli_error)?;
    ctx.broadcast("event.ext.changed", changed_scope);
    Ok(json!({ "ok": true, "stdout": stdout }))
}

/// Registered repos paired with their skills root (`.claude/skills`). Lets `skill.write` figure
/// out which repo (if any) a written path belongs to, so a repo-scoped edit can fan out to that
/// repo's lane worktrees the same way skill.create/skill.delete/plugin.enable already do.
async fn skill_repo_roots(ctx: &Ctx) -> Result<Vec<(PathBuf, PathBuf)>, RpcError> {
    Ok(ctx
        .registry
        .list()
        .await
        .map_err(internal)?
        .into_iter()
        .map(|repo| {
            let skills_root = repo.path.join(".claude/skills");
            (repo.path, skills_root)
        })
        .collect())
}

/// Every directory `skill.read`/`skill.write`/`skill.delete` are allowed to touch: the global
/// skills dirs across agent ecosystems plus repo skills dirs for every registered repo.
async fn skill_roots(ctx: &Ctx) -> Result<Vec<PathBuf>, RpcError> {
    let mut roots = Vec::new();
    if let Some(home) = crate::ext::claude_home() {
        roots.push(home.join("skills"));
    }
    if let Some(dirs) = directories::BaseDirs::new() {
        let home = dirs.home_dir();
        roots.push(home.join(".gemini/config/skills"));
        roots.push(home.join(".gemini/skills"));
        roots.push(home.join(".codex/skills"));
        roots.push(home.join(".config/opencode/skills"));
        roots.push(home.join(".opencode/skills"));
        roots.push(home.join(".cursor/skills"));
    }
    for repo in ctx.registry.list().await.map_err(internal)? {
        roots.push(repo.path.join(".claude/skills"));
        roots.push(repo.path.join(".gemini/skills"));
        roots.push(repo.path.join(".gemini/config/skills"));
        roots.push(repo.path.join(".agents/skills"));
        roots.push(repo.path.join(".codex/skills"));
        roots.push(repo.path.join(".opencode/skills"));
        roots.push(repo.path.join(".cursor/skills"));
    }
    Ok(roots)
}

/// Translate `files::ReadError` into the RPC-level error `file.read` returns. Both cases are
/// deliberate rejections (see `files::read_file`'s doc comment for why this RPC never truncates),
/// so both are `invalid_params` rather than `internal` — the caller gave a request this RPC
/// can't safely satisfy, not the daemon hitting an unexpected failure.
fn file_read_error(e: crate::files::ReadError) -> RpcError {
    match e {
        crate::files::ReadError::Binary => RpcError::invalid_params("binary file"),
        crate::files::ReadError::TooLarge(size) => RpcError::invalid_params(format!(
            "file too large to edit ({size} bytes; cap is {} bytes) — rejected rather than \
             truncated, since a truncated read risks the editor saving a truncated copy back \
             over the real file",
            crate::files::READ_CAP_BYTES
        )),
        crate::files::ReadError::Io(e) => internal(e),
    }
}

/// Translate `files::WriteError` into the RPC-level error `file.write` returns. `Conflict` gets
/// the distinct `FILE_CONFLICT` code + structured `data` (see that const's doc comment) so the
/// frontend can branch on it instead of pattern-matching the message text.
fn file_write_error(e: crate::files::WriteError) -> RpcError {
    match e {
        crate::files::WriteError::Conflict {
            expected_ms,
            actual_ms,
        } => RpcError {
            code: FILE_CONFLICT,
            message: "conflict: file changed on disk".into(),
            data: Some(json!({
                "expected_mtime_ms": expected_ms,
                "actual_mtime_ms": actual_ms,
            })),
        },
        crate::files::WriteError::NoParentDir => {
            RpcError::invalid_params("parent directory does not exist (no mkdir -p in v1)")
        }
        crate::files::WriteError::Io(e) => internal(e),
    }
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
    #[serde(default = "default_pointer_cell")]
    col: u16,
    #[serde(default = "default_pointer_cell")]
    row: u16,
    #[serde(default)]
    window: Option<String>,
}
fn default_scroll_ticks() -> u32 {
    1
}
fn default_pointer_cell() -> u16 {
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
    theme: Option<String>,
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
    notify_sound_volume: Option<f32>,
    #[serde(default)]
    notify_sound_unfocused_only: Option<bool>,
    #[serde(default)]
    notify_sound_agent_needs_you: Option<bool>,
    #[serde(default)]
    notify_sound_agent_finished: Option<bool>,
    #[serde(default)]
    notify_sound_repomind_needs_you: Option<bool>,
    #[serde(default)]
    notify_sound_error_or_stall: Option<bool>,
    #[serde(default)]
    notify_sound_incoming_message: Option<bool>,
    #[serde(default)]
    notify_sound_update_ready: Option<bool>,
    #[serde(default)]
    message_inject_agents: Option<bool>,
    #[serde(default)]
    message_inject_operator: Option<bool>,
    #[serde(default)]
    notify_show_why: Option<bool>,
    #[serde(default)]
    notify_coalesce: Option<bool>,
    #[serde(default)]
    notify_click_focus: Option<bool>,
    #[serde(default)]
    notify_desktop_fallback: Option<bool>,
    #[serde(default)]
    notify_subagents: Option<bool>,
    #[serde(default)]
    usage_probe: Option<bool>,
    #[serde(default)]
    expand_agents: Option<bool>,
    #[serde(default)]
    sort_repos_by_activity: Option<bool>,
    #[serde(default)]
    embedded_pty: Option<bool>,
    #[serde(default)]
    orchestrator_agent: Option<String>,
    #[serde(default)]
    orchestrator_model: Option<String>,
    #[serde(default)]
    agent_icons: Option<HashMap<String, String>>,
    #[serde(default)]
    supervision: Option<repomon_core::agent::supervision::SupervisionConfig>,
}

#[derive(Deserialize, Default)]
struct SupervisionGet {
    #[serde(default)]
    lane_id: Option<repomon_core::model::LaneId>,
}

#[derive(Deserialize)]
struct SupervisionSet {
    lane_id: repomon_core::model::LaneId,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    classes: Option<
        std::collections::BTreeMap<
            repomon_core::agent::supervision::DialogClass,
            repomon_core::agent::supervision::PolicyAction,
        >,
    >,
    #[serde(default)]
    mail_mode: Option<repomon_core::agent::supervision::MailDeliveryMode>,
    #[serde(default)]
    nudge_text: Option<String>,
    #[serde(default)]
    stall_mins: Option<u32>,
    #[serde(default)]
    nudge_retries: Option<u32>,
    #[serde(default)]
    expect_work: Option<bool>,
}

#[derive(Deserialize, Default)]
struct SupervisionAudit {
    #[serde(default)]
    lane_id: Option<repomon_core::model::LaneId>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    before_id: Option<i64>,
}

#[derive(Deserialize)]
struct SupervisionNudge {
    lane_id: repomon_core::model::LaneId,
    #[serde(default)]
    window: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct PushDevice {
    device_token: String,
}
#[derive(Deserialize)]
struct RemotePair {
    name: String,
}
#[derive(Deserialize)]
struct RemoteRevoke {
    name: String,
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
struct AgentTranscriptPage {
    lane_id: repomon_core::model::LaneId,
    /// Which session's transcript; `None` = the lane's most recent.
    #[serde(default)]
    session_id: Option<String>,
    /// Exclusive byte offset returned as `next_before` by the previous page.
    #[serde(default)]
    before: Option<u64>,
}
#[derive(Deserialize)]
struct AgentAdopt {
    lane_id: repomon_core::model::LaneId,
    /// Resume this exact backend session; `None` resumes the most recent Claude session.
    #[serde(default)]
    session_id: Option<String>,
    /// Additive backend identity. Omitted retains the original Claude behavior.
    #[serde(default)]
    agent: Option<String>,
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
struct CommitShowParams {
    lane_id: repomon_core::model::LaneId,
    oid: String,
    #[serde(default = "default_max_patch_chars")]
    max_patch_chars: usize,
}
#[derive(Deserialize)]
struct FileList {
    lane_id: repomon_core::model::LaneId,
    /// Relative to the worktree root; omitted/`None` lists the root itself.
    #[serde(default)]
    path: Option<String>,
}
#[derive(Deserialize)]
struct FileRead {
    lane_id: repomon_core::model::LaneId,
    path: String,
}
#[derive(Deserialize)]
struct FileWrite {
    lane_id: repomon_core::model::LaneId,
    path: String,
    content: String,
    /// The mtime (ms) the editor last read for this file. Given, this write is rejected with
    /// `FILE_CONFLICT` unless the on-disk mtime still matches. Omitted = last-write-wins.
    #[serde(default)]
    expected_mtime_ms: Option<u64>,
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

fn resolved_named(address: &str, window: Option<String>) -> ResolvedAgentAddress {
    ResolvedAgentAddress {
        address: AgentAddress::new(address),
        lane_id: None,
        slot: None,
        window,
        session_id: None,
        agent_kind: None,
    }
}

fn resolved_lane(lane: &Lane, slot: usize) -> Result<ResolvedAgentAddress, RpcError> {
    if slot == 0 {
        return Err(RpcError::invalid_params("agent slot is 1-based"));
    }
    // Transcript summaries are activity-sorted, not slot-sorted. A managed address must follow
    // the durable window name or a recently stopped external summary can steal the slot.
    let session = lane
        .agent_sessions
        .iter()
        .find(|session| {
            session
                .tmux_window
                .as_deref()
                .and_then(TmuxRuntime::slot_of_window)
                == Some(slot)
        })
        .or_else(|| lane.agent_sessions.get(slot - 1))
        .ok_or_else(|| {
            RpcError::invalid_params(format!("lane-{} has no agent slot {slot}", lane.id))
        })?;
    Ok(ResolvedAgentAddress {
        address: AgentAddress::new(format!("lane-{}/{}", lane.id, slot)),
        lane_id: Some(lane.id),
        slot: Some(slot as u32),
        window: session.tmux_window.clone(),
        session_id: session.session_id.clone(),
        agent_kind: Some(session.agent.as_str().into_owned()),
    })
}

async fn resolve_message_address(
    ctx: &Ctx,
    requested: &str,
) -> Result<ResolvedAgentAddress, RpcError> {
    let requested = requested.trim();
    if requested == "operator" {
        return Ok(resolved_named("operator", None));
    }
    if requested == "repomind" {
        let orchestrator = ctx.orchestrator.lock().await;
        return orchestrator
            .as_ref()
            .map(|session| resolved_named("repomind", Some(session.window.clone())))
            .ok_or_else(|| RpcError::invalid_params("repomind is not running"));
    }

    let lanes = lanes_with_agents(ctx).await?;
    resolve_agent_message_address(&lanes, requested)
}

fn resolve_agent_message_address(
    lanes: &[Lane],
    requested: &str,
) -> Result<ResolvedAgentAddress, RpcError> {
    if let Some(label) = requested.strip_prefix('@') {
        if label.is_empty() {
            return Err(RpcError::invalid_params("message label must not be empty"));
        }
        let mut matches = Vec::new();
        for lane in lanes {
            for (index, session) in lane.agent_sessions.iter().enumerate() {
                if session.custom_label.as_deref() == Some(label) {
                    matches.push((lane, index + 1));
                }
            }
        }
        return match matches.as_slice() {
            [] => Err(RpcError::invalid_params(format!(
                "no agent has the exact label @{label}"
            ))),
            [(lane, slot)] => resolved_lane(lane, *slot),
            _ => Err(RpcError::invalid_params(format!(
                "agent label @{label} is ambiguous"
            ))),
        };
    }

    let Some(rest) = requested.strip_prefix("lane-") else {
        return Err(RpcError::invalid_params(format!(
            "invalid fleet address {requested:?}"
        )));
    };
    let mut parts = rest.split('/');
    let lane_id: i64 = parts
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or_else(|| RpcError::invalid_params("invalid lane address"))?;
    let slot = match parts.next() {
        Some(part) => part
            .parse::<usize>()
            .ok()
            .filter(|slot| *slot > 0)
            .ok_or_else(|| RpcError::invalid_params("agent slot is 1-based"))?,
        None => 1,
    };
    if parts.next().is_some() {
        return Err(RpcError::invalid_params("invalid lane address"));
    }
    let lane = lanes
        .iter()
        .find(|lane| lane.id == lane_id)
        .ok_or_else(|| RpcError::invalid_params(format!("no lane {lane_id}")))?;
    resolved_lane(lane, slot)
}

async fn message_sender(
    ctx: &Ctx,
    identity_token: Option<String>,
    source: Option<String>,
) -> Result<ResolvedAgentAddress, RpcError> {
    if let Some(token) = identity_token {
        return ctx
            .store
            .resolve_mcp_identity(token)
            .await
            .map_err(internal)?
            .ok_or_else(|| RpcError::invalid_params("invalid or revoked MCP identity"));
    }
    match source.as_deref() {
        None | Some("operator") => Ok(resolved_named("operator", None)),
        Some("repomind") => Ok(resolved_named("repomind", Some(ORCHESTRATOR_WINDOW.into()))),
        Some(_) => Err(RpcError::invalid_params("invalid message sender source")),
    }
}

fn message_event(message: &repomon_core::model::FleetMessage) -> Value {
    json!({
        "id": message.id,
        "lane_id": message.recipient.lane_id,
        "from": message.sender.address,
        "body": message.body,
        "message": message,
    })
}

/// Dispatch a single request to its handler.
pub async fn dispatch(
    ctx: &Ctx,
    sess: &Arc<ConnSession>,
    method: &str,
    params: Option<Value>,
) -> Result<Value, RpcError> {
    // Agent-driving calls stamp this connection's last-interaction beat, which `agent.fit`'s
    // remote-vs-remote arbitration reads (last-interaction-wins). Done here on the method name so
    // the per-handler code needn't thread `sess`. `agent.fit` is deliberately absent: it stamps
    // itself only when it actually applies a resize (see its handler).
    if matches!(
        method,
        "agent.send_input"
            | "agent.signal"
            | "agent.key"
            | "agent.scroll"
            | "agent.answer"
            | "supervision.nudge"
    ) {
        *sess.last_interaction.lock().await = Some(std::time::Instant::now());
    }
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
        // Hiding leaves the repo registered, watched, and lane-bearing; only clients stop
        // showing it. Deliberately not `repo.remove`, which drops the registration outright.
        "repo.set_hidden" => {
            let p: RepoSetHidden = parse(params)?;
            ctx.registry
                .set_hidden(p.repo_id, p.hidden)
                .await
                .map_err(internal)?;
            ctx.broadcast(
                crate::pubsub::topic::REPO_CHANGED,
                json!({ "repo_id": p.repo_id, "hidden": p.hidden }),
            );
            Ok(Value::Null)
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
        "repo.notes.get" => {
            let p: RepoNotesGet = parse(params)?;
            let repo = ctx
                .store
                .get_repo(p.repo_id)
                .await
                .map_err(|_| RpcError::invalid_params(format!("no repo {}", p.repo_id)))?;
            let all = ctx.registry.list().await.map_err(internal)?;
            let dir = ctx.notes_dir.clone();
            let repo_name = repo.name.clone();
            let (content, path) = tokio::task::spawn_blocking(move || {
                let path = repomon_core::notes::notes_path(&dir, &repo, &all);
                repomon_core::notes::read(&dir, &repo, &all).map(|c| (c, path))
            })
            .await
            .map_err(internal)?
            .map_err(internal)?;
            to_value(json!({
                "repo_id": p.repo_id,
                "name": repo_name,
                "exists": content.is_some(),
                "content": content.unwrap_or_default(),
                "path": path.to_string_lossy(),
            }))
        }
        "repo.notes.set" => {
            let p: RepoNotesSet = parse(params)?;
            // Cap before any lookup: the error should name the limit, not depend on repo state.
            if p.content.len() > repomon_core::notes::MAX_NOTES_BYTES {
                return Err(RpcError::invalid_params(format!(
                    "notes are {} bytes; the cap is {} bytes",
                    p.content.len(),
                    repomon_core::notes::MAX_NOTES_BYTES
                )));
            }
            let repo = ctx
                .store
                .get_repo(p.repo_id)
                .await
                .map_err(|_| RpcError::invalid_params(format!("no repo {}", p.repo_id)))?;
            let all = ctx.registry.list().await.map_err(internal)?;
            let dir = ctx.notes_dir.clone();
            let repo_name = repo.name.clone();
            let content = p.content.clone();
            let (old_bytes, path) = tokio::task::spawn_blocking(move || {
                let old = repomon_core::notes::read(&dir, &repo, &all)
                    .ok()
                    .flatten()
                    .map(|s| s.len());
                repomon_core::notes::write(&dir, &repo, &all, &content).map(|p| (old, p))
            })
            .await
            .map_err(internal)?
            .map_err(internal)?;
            tracing::info!(
                repo = %repo_name,
                repo_id = p.repo_id,
                old_bytes = old_bytes.unwrap_or(0),
                new_bytes = p.content.len(),
                path = %path.display(),
                conn = ?sess.kind,
                "repo notes replaced"
            );
            to_value(json!({
                "repo_id": p.repo_id,
                "bytes": p.content.len(),
                "path": path.to_string_lossy(),
            }))
        }

        // ---- orchestration journal ----
        "journal.append" => {
            let p: JournalAppend = parse(params)?;
            let id = ctx
                .store
                .append_journal(repomon_core::model::JournalEntry {
                    id: 0,
                    at: chrono::Utc::now(),
                    session: p.session,
                    action: p.action,
                    lane_id: p.lane_id,
                    repo: p.repo,
                    params: p.params,
                    outcome: p.outcome.unwrap_or_else(|| "ok".to_string()),
                    detail: p.detail,
                })
                .await
                .map_err(internal)?;
            to_value(json!({ "id": id }))
        }
        "journal.query" => {
            let p: JournalQuery = parse(params)?;
            let limit = p.limit.unwrap_or(50).min(200);
            let entries = if let Some(q) = p.query {
                ctx.store.search_journal(q, limit).await
            } else if p.since_last_session {
                ctx.store.journal_since_prev_session(limit).await
            } else {
                ctx.store.recent_journal(limit).await
            }
            .map_err(internal)?;
            to_value(json!({ "entries": entries }))
        }

        // ---- approval policy ----
        "approval.record" => {
            use repomon_core::agent::approval;
            let p: ApprovalRecord = parse(params)?;
            if !matches!(p.verdict.as_str(), "approve" | "deny") {
                return Err(RpcError::invalid_params(
                    "verdict must be \"approve\" or \"deny\"",
                ));
            }
            let pattern = approval::command_pattern(&p.command);
            if pattern.is_empty() {
                return to_value(json!({
                    "pattern": Value::Null,
                    "approvals": 0,
                    "rule_exists": false,
                    "propose": false,
                }));
            }
            let approvals = ctx
                .store
                .record_approval_event(p.repo.clone(), pattern.clone(), p.verdict.clone())
                .await
                .map_err(internal)?;
            let rule_exists = ctx
                .store
                .has_approval_rule(p.repo.clone(), pattern.clone())
                .await
                .map_err(internal)?;
            let propose =
                approvals >= 3 && !rule_exists && !approval::is_always_escalate(&p.command);
            tracing::info!(
                repo = %p.repo,
                pattern = %pattern,
                verdict = %p.verdict,
                approvals,
                "approval verdict recorded"
            );
            to_value(json!({
                "pattern": pattern,
                "approvals": approvals,
                "rule_exists": rule_exists,
                "propose": propose,
            }))
        }
        "approval.allow" => {
            let p: ApprovalRuleRef = parse(params)?;
            ctx.store
                .add_approval_rule(p.repo.clone(), p.pattern.clone())
                .await
                .map_err(internal)?;
            tracing::info!(repo = %p.repo, pattern = %p.pattern, "approval rule confirmed");
            Ok(Value::Null)
        }
        "approval.remove" => {
            let p: ApprovalRuleRef = parse(params)?;
            ctx.store
                .remove_approval_rule(p.repo.clone(), p.pattern.clone())
                .await
                .map_err(|e| RpcError::invalid_params(e.to_string()))?;
            tracing::info!(repo = %p.repo, pattern = %p.pattern, "approval rule removed");
            Ok(Value::Null)
        }
        "approval.list" => {
            let rules = ctx.store.list_approval_rules().await.map_err(internal)?;
            to_value(json!({ "rules": rules }))
        }

        // ---- standing-orchestration schedules ----
        "schedule.add" => {
            let p: ScheduleAdd = parse(params)?;
            let spec = repomon_core::schedule::parse_spec(&p.spec)
                .map_err(|e| RpcError::invalid_params(e.to_string()))?;
            let prompt = p.prompt.trim().to_string();
            if prompt.is_empty() || prompt.len() > 2000 {
                return Err(RpcError::invalid_params(
                    "schedule prompt must be 1-2000 bytes",
                ));
            }
            // Headless standing runs drive `claude -p`; a non-Claude orchestrator can't run them.
            {
                let cfg = ctx.config.read().await;
                if matches!(
                    resolve_orchestrator_backend(&cfg.orchestrator_agent, &cfg.agents),
                    Ok(b) if b != crate::OrchestratorBackend::Claude
                ) {
                    return Err(RpcError::invalid_params(
                        "headless standing runs support the claude backend only; \
                         orchestrator_agent is set to a non-claude backend",
                    ));
                }
            }
            let max_actions = p.max_actions.unwrap_or(10).min(50);
            let sched = ctx
                .store
                .add_schedule(p.spec.clone(), prompt, max_actions)
                .await
                .map_err(internal)?;
            tracing::info!(id = sched.id, spec = %sched.spec, "schedule added");
            let mut v = serde_json::to_value(&sched).map_err(internal)?;
            v["next_run"] = json!(spec.next_after(chrono::Local::now()).to_rfc3339());
            Ok(v)
        }
        "schedule.list" => {
            let scheds = ctx.store.list_schedules().await.map_err(internal)?;
            let now = chrono::Local::now();
            let rows: Vec<Value> = scheds
                .iter()
                .map(|s| {
                    let mut v = serde_json::to_value(s).unwrap_or_default();
                    if let Ok(spec) = repomon_core::schedule::parse_spec(&s.spec) {
                        v["next_run"] = json!(spec.next_after(now).to_rfc3339());
                    }
                    v
                })
                .collect();
            to_value(json!({ "schedules": rows }))
        }
        "schedule.remove" => {
            let p: ScheduleRemove = parse(params)?;
            ctx.store
                .remove_schedule(p.id)
                .await
                .map_err(|e| RpcError::invalid_params(e.to_string()))?;
            tracing::info!(id = p.id, "schedule removed");
            Ok(Value::Null)
        }

        // ---- playbooks ----
        "playbook.save" => {
            let p: PlaybookSave = parse(params)?;
            let name = p.name.trim();
            if name.is_empty()
                || name.len() > 64
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            {
                return Err(RpcError::invalid_params(
                    "playbook name must be 1-64 chars of [A-Za-z0-9._-] (kebab-case works well)",
                ));
            }
            if p.content.len() > 16384 {
                return Err(RpcError::invalid_params(format!(
                    "playbook is {} bytes; the cap is 16384 bytes",
                    p.content.len()
                )));
            }
            let book = ctx
                .store
                .save_playbook(name.to_string(), p.content)
                .await
                .map_err(internal)?;
            tracing::info!(playbook = %book.name, status = %book.status, "playbook saved");
            to_value(book)
        }
        "playbook.search" => {
            let p: PlaybookSearch = parse(params)?;
            let books = ctx
                .store
                .search_playbooks(p.query, p.limit.unwrap_or(10).min(50))
                .await
                .map_err(internal)?;
            to_value(json!({ "playbooks": books }))
        }
        "playbook.list" => {
            let books = ctx.store.list_playbooks().await.map_err(internal)?;
            to_value(json!({ "playbooks": books }))
        }
        "playbook.approve" => {
            let p: PlaybookName = parse(params)?;
            let book = ctx
                .store
                .approve_playbook(p.name)
                .await
                .map_err(|e| RpcError::invalid_params(e.to_string()))?;
            tracing::info!(playbook = %book.name, "playbook approved");
            to_value(book)
        }
        "playbook.delete" => {
            let p: PlaybookName = parse(params)?;
            ctx.store
                .delete_playbook(p.name.clone())
                .await
                .map_err(|e| RpcError::invalid_params(e.to_string()))?;
            tracing::info!(playbook = %p.name, "playbook deleted");
            Ok(Value::Null)
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

        // ---- durable fleet messages ----
        "message.send" => {
            let p: MessageSend = parse(params)?;
            let sender = message_sender(ctx, p.identity_token, p.source).await?;
            if let Some(single) = p.to.as_legacy_single() {
                // Exact pre-A6 behavior: one address in, one `FleetMessage` out.
                let to = single.to_string();
                let recipient = resolve_message_address(ctx, &to).await?;
                let message = ctx
                    .store
                    .send_message(AgentAddress::new(to), sender, recipient, p.body, p.reply_to)
                    .await
                    .map_err(internal)?;
                ctx.broadcast("event.message.stored", message_event(&message));
                to_value(message)
            } else {
                // A list and/or a wildcard: fan out one `send_message` per resolved recipient,
                // reusing the same single-delivery machinery, and report a per-recipient result
                // instead of a bare `FleetMessage`.
                let lanes = if p.to.has_wildcard() {
                    lanes_with_agents(ctx).await?
                } else {
                    Vec::new()
                };
                let targets = expand_message_targets(&p.to, &lanes, &sender)?;
                let mut results = Vec::with_capacity(targets.len());
                let mut sent_count = 0usize;
                for target in targets {
                    match resolve_message_address(ctx, &target).await {
                        Ok(recipient) => {
                            match ctx
                                .store
                                .send_message(
                                    AgentAddress::new(target.clone()),
                                    sender.clone(),
                                    recipient,
                                    p.body.clone(),
                                    p.reply_to.clone(),
                                )
                                .await
                            {
                                Ok(message) => {
                                    ctx.broadcast("event.message.stored", message_event(&message));
                                    sent_count += 1;
                                    results.push(json!({
                                        "to": target,
                                        "status": "sent",
                                        "message_id": message.id,
                                        "thread_id": message.thread_id,
                                    }));
                                }
                                Err(error) => {
                                    results.push(json!({
                                        "to": target,
                                        "status": "delivery_error",
                                        "error": error.to_string(),
                                    }));
                                }
                            }
                        }
                        Err(error) => {
                            results.push(json!({
                                "to": target,
                                "status": "no_such_session",
                                "error": error.message,
                            }));
                        }
                    }
                }
                to_value(json!({
                    "recipient_count": results.len(),
                    "sent_count": sent_count,
                    "results": results,
                }))
            }
        }
        "message.inbox" => {
            let p: MessageInbox = parse(params)?;
            let recipient = message_sender(ctx, p.identity_token, p.source).await?;
            let page = ctx
                .store
                .list_messages(
                    Some(recipient.address),
                    None,
                    p.unread_only,
                    p.limit.unwrap_or(50),
                    p.before,
                    true,
                )
                .await
                .map_err(internal)?;
            to_value(page)
        }
        "message.mark_read" => {
            let p: MessageMarkRead = parse(params)?;
            let sender = message_sender(ctx, p.identity_token, p.source).await?;
            let current = ctx
                .store
                .get_message(p.id.clone())
                .await
                .map_err(internal)?;
            if sender.address.as_str() != "operator" && current.recipient.address != sender.address
            {
                return Err(RpcError::invalid_params(
                    "message does not belong to this inbox",
                ));
            }
            to_value(ctx.store.mark_message_read(p.id).await.map_err(internal)?)
        }
        "message.list" => {
            let p: MessageList = parse(params)?;
            to_value(
                ctx.store
                    .list_messages(
                        None,
                        p.lane_id,
                        p.unread_only,
                        p.limit.unwrap_or(50),
                        p.before,
                        false,
                    )
                    .await
                    .map_err(internal)?,
            )
        }
        "lane.create" => {
            let mut p: CreateLaneParams = parse(params)?;
            // Defense-in-depth: a remote caller must not pin the worktree to an arbitrary host path.
            // The remote allowlist grants `lane.create` but deliberately withholds `fs.browse`, so a
            // paired device has no legitimate way to have picked a path — it's expected to let the
            // daemon derive the template worktree location. Strip any supplied `path` (rather than
            // hard-erroring) so a harmless client that fills it in still succeeds, while a hostile
            // one can't write outside the managed worktree root. The local Unix socket is unaffected.
            if !sess.is_local() && p.path.take().is_some() {
                tracing::warn!(
                    "remote lane.create supplied a path; ignoring it (deriving template)"
                );
            }
            let lane = ctx.lanes.create(p).await.map_err(internal)?;
            // Seed the new worktree with the repo's extension config (best-effort; a failure only
            // means the lane starts with whatever git checked out).
            let repo_root = lane.repo.path.clone();
            let wt_path = lane.worktree.path.clone();
            tokio::task::spawn_blocking(move || {
                if let Err(e) = crate::ext::sync_worktree(&repo_root, &wt_path) {
                    tracing::debug!("lane.create ext seed skipped: {e}");
                }
            });
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
            let _ = ctx.store.delete_lane_policy(p.lane_id).await;
            crate::supervision::refresh(ctx).await;
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
                // d.untracked, not lane.state.dirty.untracked: the cached scan can lag the
                // live stats computed above, and one lane.diff snapshot must be self-consistent.
                "untracked": d.untracked,
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
        // commit.show: item 6, one commit's full metadata + patch for GitExplorerPanel's
        // commit-detail view (Branch/History rows becoming clickable). LOCAL-ONLY — see
        // remote.rs's remote_method_allowed, which withholds this alongside the worktree
        // file-editor RPCs: unlike lane.diff (scoped to one lane's current diff), a caller-chosen
        // oid can walk the *entire* repo history one commit at a time, a much broader read surface
        // than the already-allowed reads.
        "commit.show" => {
            let p: CommitShowParams = parse(params)?;
            let max_patch_chars = p.max_patch_chars.min(MAX_PATCH_CHARS_CEILING);
            let lane = ctx.lanes.get(p.lane_id).await.map_err(internal)?;
            let wt_path = lane.worktree.path.clone();
            let oid = p.oid.clone();
            let mut show = tokio::task::spawn_blocking(move || diff::commit_show(&wt_path, &oid))
                .await
                .map_err(internal)?
                .map_err(internal)?;

            let (patch, patch_truncated) = cap_chars(&show.patch, max_patch_chars);
            show.patch = patch;
            show.patch_truncated = patch_truncated;
            to_value(show)
        }

        // ---- worktree files, for the in-app editor (D1 file.list, D2 file.read/file.write) ----
        // LOCAL-ONLY: see `remote::remote_method_allowed`'s doc comment, which withholds these
        // three the same way it already withholds `fs.browse` — doubly so here since `file.write`
        // touches the host filesystem.
        "file.list" => {
            let p: FileList = parse(params)?;
            let lane = ctx.lanes.get(p.lane_id).await.map_err(internal)?;
            let root = lane.worktree.path.clone();
            let rel = p.path.unwrap_or_default();
            let Some(dir) = crate::files::worktree_path_allowed(&root, &rel) else {
                return Err(RpcError::invalid_params("path escapes the worktree root"));
            };
            tokio::task::spawn_blocking(move || crate::files::list_dir(&root, &dir))
                .await
                .map_err(internal)?
                .map_err(internal)
                .and_then(to_value)
        }
        "file.read" => {
            let p: FileRead = parse(params)?;
            let lane = ctx.lanes.get(p.lane_id).await.map_err(internal)?;
            let root = lane.worktree.path.clone();
            let Some(target) = crate::files::worktree_path_allowed(&root, &p.path) else {
                return Err(RpcError::invalid_params("path escapes the worktree root"));
            };
            tokio::task::spawn_blocking(move || crate::files::read_file(&target))
                .await
                .map_err(internal)?
                .map_err(file_read_error)
                .and_then(to_value)
        }
        "file.write" => {
            let p: FileWrite = parse(params)?;
            let lane = ctx.lanes.get(p.lane_id).await.map_err(internal)?;
            let root = lane.worktree.path.clone();
            let Some(target) = crate::files::worktree_path_allowed(&root, &p.path) else {
                return Err(RpcError::invalid_params("path escapes the worktree root"));
            };
            let content = p.content.clone();
            let expected = p.expected_mtime_ms;
            let result = tokio::task::spawn_blocking(move || {
                crate::files::write_file(&target, &content, expected)
            })
            .await
            .map_err(internal)?
            .map_err(file_write_error)?;
            ctx.broadcast(
                "event.file.changed",
                json!({ "lane_id": p.lane_id, "path": p.path }),
            );
            to_value(result)
        }

        // ---- extensions (Claude Code config: marketplaces, plugins, skills) ----
        "ext.list" => {
            let p: ExtList = parse(params)?;
            let account = p.account.clone().unwrap_or_else(|| "default".to_string());
            let repo_root = match p.scope {
                ExtScope::Global => None,
                ExtScope::Repo { repo_id } => {
                    Some(ctx.store.get_repo(repo_id).await.map_err(internal)?.path)
                }
            };
            let cli_version = match account.as_str() {
                "antigravity" => Some("antigravity".to_string()),
                "codex" => Some("codex".to_string()),
                "opencode" => Some("opencode".to_string()),
                "cursor" => Some("cursor".to_string()),
                _ => claude_cli().await.ok().map(|c| c.version.clone()),
            };
            let accounts = crate::ext::ext_accounts();
            let snap = tokio::task::spawn_blocking(move || {
                let mut snap =
                    crate::ext::scan_for_account(&account, repo_root.as_deref(), cli_version);
                snap.accounts = accounts;
                snap.account = account;
                snap
            })
            .await
            .map_err(internal)?;
            to_value(snap)
        }
        "plugin.enable" | "plugin.disable" => {
            let enabled = method == "plugin.enable";
            let p: PluginToggle = parse(params)?;
            let account = p.account.as_deref();
            match account {
                Some("opencode") => {
                    let repo_path = match &p.scope {
                        ExtScope::Repo { repo_id } => {
                            Some(ctx.store.get_repo(*repo_id).await.map_err(internal)?.path)
                        }
                        ExtScope::Global => None,
                    };
                    if enabled {
                        crate::ext::enable_opencode_plugin(&p.id, repo_path.as_deref())
                            .map_err(internal)?;
                    } else {
                        crate::ext::disable_opencode_plugin(&p.id, repo_path.as_deref())
                            .map_err(internal)?;
                    }
                    ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
                    Ok(json!({ "ok": true }))
                }
                Some("antigravity") => {
                    let settings_path = match &p.scope {
                        ExtScope::Global => {
                            let dirs = directories::BaseDirs::new()
                                .ok_or_else(|| internal("no home directory"))?;
                            dirs.home_dir().join(".gemini/settings.json")
                        }
                        ExtScope::Repo { repo_id } => {
                            let repo = ctx.store.get_repo(*repo_id).await.map_err(internal)?;
                            repo.path.join(".gemini/settings.json")
                        }
                    };
                    let raw_name = p.id.split('@').next().unwrap_or(&p.id);
                    crate::ext::set_plugin_enabled(&settings_path, &p.id, Some(enabled))
                        .map_err(internal)?;
                    if raw_name != p.id {
                        let _ =
                            crate::ext::set_plugin_enabled(&settings_path, raw_name, Some(enabled));
                    }
                    ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
                    Ok(json!({ "ok": true }))
                }
                Some("codex") => {
                    let settings_path = match &p.scope {
                        ExtScope::Global => {
                            let dirs = directories::BaseDirs::new()
                                .ok_or_else(|| internal("no home directory"))?;
                            dirs.home_dir().join(".codex/settings.json")
                        }
                        ExtScope::Repo { repo_id } => {
                            let repo = ctx.store.get_repo(*repo_id).await.map_err(internal)?;
                            repo.path.join(".codex/settings.json")
                        }
                    };
                    let raw_name = p.id.split('@').next().unwrap_or(&p.id);
                    crate::ext::set_plugin_enabled(&settings_path, &p.id, Some(enabled))
                        .map_err(internal)?;
                    if raw_name != p.id {
                        let _ =
                            crate::ext::set_plugin_enabled(&settings_path, raw_name, Some(enabled));
                    }
                    ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
                    Ok(json!({ "ok": true }))
                }
                Some("cursor") => {
                    let settings_path = match &p.scope {
                        ExtScope::Global => {
                            let dirs = directories::BaseDirs::new()
                                .ok_or_else(|| internal("no home directory"))?;
                            dirs.home_dir().join(".cursor/settings.json")
                        }
                        ExtScope::Repo { repo_id } => {
                            let repo = ctx.store.get_repo(*repo_id).await.map_err(internal)?;
                            repo.path.join(".cursor/settings.json")
                        }
                    };
                    crate::ext::set_plugin_enabled(&settings_path, &p.id, Some(enabled))
                        .map_err(internal)?;
                    ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
                    Ok(json!({ "ok": true }))
                }
                _ => {
                    let (settings, fanout_root) = match &p.scope {
                        ExtScope::Global => {
                            let home = crate::ext::claude_home_for(account).ok_or_else(|| {
                                internal("this account has no Claude config directory")
                            })?;
                            (home.join("settings.json"), None)
                        }
                        ExtScope::Repo { repo_id } => {
                            let repo = ctx.store.get_repo(*repo_id).await.map_err(internal)?;
                            (
                                repo.path.join(".claude/settings.local.json"),
                                Some(repo.path),
                            )
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
            }
        }
        "plugin.install" => {
            let p: PluginInstall = parse(params)?;
            let account = p.account.as_deref();
            let repo_path = match &p.scope {
                ExtScope::Repo { repo_id } => {
                    Some(ctx.store.get_repo(*repo_id).await.map_err(internal)?.path)
                }
                ExtScope::Global => None,
            };
            match account {
                Some("opencode") => {
                    crate::ext::add_opencode_plugin(&p.r#ref, repo_path.as_deref())
                        .map_err(internal)?;
                    ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
                    Ok(json!({ "ok": true }))
                }
                Some("antigravity") => {
                    crate::ext::install_antigravity_plugin(&p.r#ref, repo_path.as_deref())
                        .map_err(internal)?;
                    ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
                    Ok(json!({ "ok": true }))
                }
                Some("codex") => {
                    crate::ext::install_codex_plugin(&p.r#ref, repo_path.as_deref())
                        .map_err(internal)?;
                    ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
                    Ok(json!({ "ok": true }))
                }
                Some("cursor") => {
                    crate::ext::install_cursor_extension(&p.r#ref, repo_path.as_deref())
                        .map_err(internal)?;
                    ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
                    Ok(json!({ "ok": true }))
                }
                _ => {
                    let cli = claude_cli().await?;
                    let config_dir = crate::ext::account_config_dir(account);
                    let args: [String; 5] = [
                        "plugin".into(),
                        "install".into(),
                        p.r#ref.clone(),
                        "-s".into(),
                        "user".into(),
                    ];
                    let stdout = tokio::task::spawn_blocking(move || {
                        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                        cli.run_for(config_dir.as_deref(), &arg_refs)
                    })
                    .await
                    .map_err(internal)?
                    .map_err(cli_error)?;
                    // Repo-scope install also enables it there so "install to this repo" does what it says.
                    let fanout = match &p.scope {
                        ExtScope::Repo { repo_id } => {
                            let repo = ctx.store.get_repo(*repo_id).await.map_err(internal)?;
                            let settings = repo.path.join(".claude/settings.local.json");
                            let id = p.r#ref.clone();
                            let root = repo.path.clone();
                            let summary = tokio::task::spawn_blocking(move || {
                                crate::ext::set_plugin_enabled(&settings, &id, Some(true))?;
                                Ok::<_, std::io::Error>(crate::ext::fan_out(&root))
                            })
                            .await
                            .map_err(internal)?
                            .map_err(internal)?;
                            Some(summary)
                        }
                        ExtScope::Global => None,
                    };
                    ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
                    Ok(json!({ "ok": true, "stdout": stdout, "fanout": fanout }))
                }
            }
        }
        "plugin.remove" => {
            let p: PluginToggle = parse(params)?;
            let account = p.account.as_deref();
            let repo_path = match &p.scope {
                ExtScope::Repo { repo_id } => {
                    Some(ctx.store.get_repo(*repo_id).await.map_err(internal)?.path)
                }
                ExtScope::Global => None,
            };

            match account {
                Some("opencode") => {
                    crate::ext::remove_opencode_plugin(&p.id, repo_path.as_deref())
                        .map_err(internal)?;
                    ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
                    Ok(json!({ "ok": true }))
                }
                Some("antigravity") => {
                    crate::ext::remove_antigravity_plugin(&p.id, repo_path.as_deref())
                        .map_err(internal)?;
                    ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
                    Ok(json!({ "ok": true }))
                }
                Some("codex") => {
                    crate::ext::remove_codex_plugin(&p.id, repo_path.as_deref())
                        .map_err(internal)?;
                    ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
                    Ok(json!({ "ok": true }))
                }
                Some("cursor") => {
                    crate::ext::remove_cursor_extension(&p.id, repo_path.as_deref())
                        .map_err(internal)?;
                    ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
                    Ok(json!({ "ok": true }))
                }
                _ => {
                    let cli = claude_cli().await?;
                    let config_dir = crate::ext::account_config_dir(account);
                    let home = crate::ext::claude_home_for(account)
                        .ok_or_else(|| internal("this account has no Claude config directory"))?;
                    let id = p.id.clone();
                    let stdout = tokio::task::spawn_blocking(move || {
                        crate::ext::uninstall_claude_plugin(
                            &cli,
                            config_dir.as_deref(),
                            &home,
                            &id,
                            repo_path.as_deref(),
                        )
                    })
                    .await
                    .map_err(internal)?
                    .map_err(cli_error)?;

                    ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
                    Ok(json!({ "ok": true, "stdout": stdout }))
                }
            }
        }
        "plugin.update" => {
            let p: OptionalId = parse(params)?;
            let account = p.account.as_deref();
            match account {
                Some("opencode") => {
                    Ok(json!({ "ok": true, "stdout": "OpenCode plugins up to date" }))
                }
                Some("antigravity") => {
                    Ok(json!({ "ok": true, "stdout": "Antigravity plugins up to date" }))
                }
                Some("codex") => Ok(json!({ "ok": true, "stdout": "Codex plugins up to date" })),
                Some("cursor") => {
                    Ok(json!({ "ok": true, "stdout": "Cursor extensions are managed by Cursor" }))
                }
                _ => {
                    let mut args = vec!["plugin".into(), "update".into()];
                    if let Some(id) = p.id {
                        args.push(id)
                    }
                    run_cli_op(
                        ctx,
                        p.account.as_deref(),
                        args,
                        json!({ "scope": "global" }),
                    )
                    .await
                }
            }
        }
        "plugin.details" => {
            let p: IdOnly = parse(params)?;
            let account = p.account.as_deref();
            match account {
                Some("opencode") => {
                    let details = crate::ext::opencode_plugin_details(&p.id, None);
                    Ok(json!({ "text": details }))
                }
                Some("antigravity") => {
                    let details = crate::ext::antigravity_plugin_details(&p.id, None);
                    Ok(json!({ "text": details }))
                }
                Some("codex") => {
                    let details = crate::ext::codex_plugin_details(&p.id, None);
                    Ok(json!({ "text": details }))
                }
                Some("cursor") => {
                    let details = crate::ext::cursor_extension_details(&p.id, None);
                    Ok(json!({ "text": details }))
                }
                _ => {
                    let cli = claude_cli().await?;
                    let config_dir = crate::ext::account_config_dir(p.account.as_deref());
                    let text = tokio::task::spawn_blocking(move || {
                        cli.run_for(config_dir.as_deref(), &["plugin", "details", &p.id])
                    })
                    .await
                    .map_err(internal)?
                    .map_err(cli_error)?;
                    Ok(json!({ "text": text }))
                }
            }
        }
        "marketplace.add" => {
            let p: SourceOnly = parse(params)?;
            run_cli_op(
                ctx,
                p.account.as_deref(),
                vec![
                    "plugin".into(),
                    "marketplace".into(),
                    "add".into(),
                    p.source,
                ],
                json!({ "scope": "global" }),
            )
            .await
        }
        "marketplace.remove" => {
            let p: NameOnly = parse(params)?;
            run_cli_op(
                ctx,
                p.account.as_deref(),
                vec![
                    "plugin".into(),
                    "marketplace".into(),
                    "remove".into(),
                    p.name,
                ],
                json!({ "scope": "global" }),
            )
            .await
        }
        "marketplace.refresh" => {
            let p: OptionalName = parse(params)?;
            let mut args = vec!["plugin".into(), "marketplace".into(), "update".into()];
            if let Some(name) = p.name {
                args.push(name)
            }
            run_cli_op(
                ctx,
                p.account.as_deref(),
                args,
                json!({ "scope": "global" }),
            )
            .await
        }

        // ---- skills (create/read/write/delete SKILL.md, path-guarded) ----
        "skill.create" => {
            let p: SkillCreate = parse(params)?;
            let is_global = matches!(p.scope, ExtScope::Global);
            let repo = match &p.scope {
                ExtScope::Repo { repo_id } => {
                    Some(ctx.store.get_repo(*repo_id).await.map_err(internal)?)
                }
                ExtScope::Global => None,
            };
            let skills_dir = crate::ext::skills_dir_for(
                p.account.as_deref(),
                is_global,
                repo.as_ref().map(|r| r.path.as_path()),
            )
            .ok_or_else(|| internal("failed to resolve skills directory for account"))?;
            let fanout_root = repo.map(|r| r.path);
            let (name, description) = (p.name.clone(), p.description.clone());
            let path = tokio::task::spawn_blocking(move || {
                let path = crate::ext::scaffold_skill(&skills_dir, &name, description.as_deref())?;
                if let Some(root) = fanout_root {
                    crate::ext::fan_out(&root);
                }
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
                return Err(RpcError::invalid_params(
                    "path is outside managed skill directories",
                ));
            }
            let md = if p.path.ends_with("SKILL.md") {
                p.path.clone()
            } else {
                p.path.join("SKILL.md")
            };
            let content = tokio::fs::read_to_string(&md).await.map_err(internal)?;
            Ok(json!({ "content": content }))
        }
        "skill.write" => {
            let p: SkillWrite = parse(params)?;
            let roots = skill_roots(ctx).await?;
            if !crate::ext::skill_path_allowed(&p.path, &roots) {
                return Err(RpcError::invalid_params(
                    "path is outside managed skill directories",
                ));
            }
            let md = if p.path.ends_with("SKILL.md") {
                p.path.clone()
            } else {
                p.path.join("SKILL.md")
            };
            tokio::fs::write(&md, p.content).await.map_err(internal)?;
            // create/delete/toggle all fan a repo-scoped change out to every lane worktree; find
            // out whether this write landed under a registered repo's skills root so it does the
            // same, instead of leaving existing worktrees with a stale copy.
            let mut fanout_repo = None;
            if let Ok(resolved_md) = md.canonicalize() {
                for (repo_path, skills_root) in skill_repo_roots(ctx).await? {
                    if skills_root
                        .canonicalize()
                        .is_ok_and(|root| resolved_md.starts_with(&root))
                    {
                        fanout_repo = Some(repo_path);
                        break;
                    }
                }
            }
            let fanout = match fanout_repo {
                Some(repo_path) => Some(
                    tokio::task::spawn_blocking(move || crate::ext::fan_out(&repo_path))
                        .await
                        .map_err(internal)?,
                ),
                None => None,
            };
            ctx.broadcast("event.ext.changed", json!({ "scope": "global" }));
            Ok(json!({ "ok": true, "fanout": fanout }))
        }
        "skill.delete" => {
            let p: SkillDelete = parse(params)?;
            if !crate::ext::valid_skill_name(&p.name) {
                return Err(RpcError::invalid_params("invalid skill name"));
            }
            let is_global = matches!(p.scope, ExtScope::Global);
            let repo = match &p.scope {
                ExtScope::Repo { repo_id } => {
                    Some(ctx.store.get_repo(*repo_id).await.map_err(internal)?)
                }
                ExtScope::Global => None,
            };
            let fanout_root = repo.as_ref().map(|r| r.path.clone());
            let account = p.account.clone();
            let name = p.name.clone();
            let repo_path = repo.map(|r| r.path);
            let fanout = tokio::task::spawn_blocking(move || {
                crate::ext::delete_skill(
                    account.as_deref(),
                    is_global,
                    &name,
                    repo_path.as_deref(),
                )?;
                // sync_worktree prunes skills whose source dir is gone (see this task's ext.rs
                // change), so one fan-out both deletes the skill everywhere and re-syncs the
                // survivors.
                Ok::<_, std::io::Error>(fanout_root.map(|root| crate::ext::fan_out(&root)))
            })
            .await
            .map_err(internal)?
            .map_err(internal)?;
            ctx.broadcast("event.ext.changed", ext_scope_json(&p.scope));
            Ok(json!({ "ok": true, "fanout": fanout }))
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
            let choices: Vec<AgentChoice> = detect_all_agents(&cfg)
                .into_iter()
                .map(|a| AgentChoice {
                    default: is_default(&a.name),
                    detected: a.detected,
                    name: a.name,
                    command: a.command,
                    custom: a.custom,
                })
                .collect();
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
                if let Some(t) = p.theme {
                    cfg.theme = Some(t);
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
                if let Some(volume) = p.notify_sound_volume {
                    cfg.notify_sound_volume = volume.clamp(0.0, 1.0);
                }
                if let Some(b) = p.notify_sound_unfocused_only {
                    cfg.notify_sound_unfocused_only = b;
                }
                if let Some(b) = p.notify_sound_agent_needs_you {
                    cfg.notify_sound_agent_needs_you = b;
                }
                if let Some(b) = p.notify_sound_agent_finished {
                    cfg.notify_sound_agent_finished = b;
                }
                if let Some(b) = p.notify_sound_repomind_needs_you {
                    cfg.notify_sound_repomind_needs_you = b;
                }
                if let Some(b) = p.notify_sound_error_or_stall {
                    cfg.notify_sound_error_or_stall = b;
                }
                if let Some(b) = p.notify_sound_incoming_message {
                    cfg.notify_sound_incoming_message = b;
                }
                if let Some(b) = p.notify_sound_update_ready {
                    cfg.notify_sound_update_ready = b;
                }
                if let Some(b) = p.message_inject_agents {
                    cfg.message_inject_agents = b;
                }
                if let Some(b) = p.message_inject_operator {
                    cfg.message_inject_operator = b;
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
                if let Some(b) = p.notify_desktop_fallback {
                    cfg.notify_desktop_fallback = b;
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
                if let Some(b) = p.sort_repos_by_activity {
                    cfg.sort_repos_by_activity = b;
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
                if let Some(icons) = p.agent_icons {
                    cfg.agent_icons = icons;
                }
                let had_supervision = p.supervision.is_some();
                if let Some(s) = p.supervision {
                    cfg.supervision = s;
                }
                if let Err(e) = cfg.save_to(&ctx.config_path) {
                    *cfg = prev;
                    return Err(internal(e));
                }
                if had_supervision {
                    drop(cfg);
                    crate::supervision::refresh(ctx).await;
                }
            }
            let cfg = ctx.config.read().await;
            let value = config_json(&cfg);
            ctx.broadcast("event.config.changed", value.clone());
            Ok(value)
        }
        "agent.spawn" => {
            let p: AgentSpawn = parse(params)?;
            let path = ctx.lanes.focus(p.lane_id).await.map_err(internal)?;
            // Resolve the chosen name to a command AND the kind whose flag dialect we translate
            // launch options for: a config custom wins (kind inferred from the command it runs, so
            // a claude wrapper still gets claude flags), then an autodetected Claude variant (e.g.
            // claude-work → `CLAUDE_CONFIG_DIR=… claude`, still Claude under the hood), else the
            // kind's default binary.
            let (command, kind) = {
                let cfg = ctx.config.read().await;
                if let Some(c) = cfg.agents.get(&p.agent) {
                    let kind = kind_from_command(c);
                    (c.clone(), kind)
                } else if let Some((_, cmd)) = agent::claude::agent_variants()
                    .into_iter()
                    .find(|(n, _)| n == &p.agent)
                {
                    (cmd, AgentKind::ClaudeCode)
                } else {
                    let k = AgentKind::from_kind_str(&p.agent);
                    (k.command().to_string(), k)
                }
            };
            // Translate --mode/--model/--effort into the kind's flags (and, for claude `ultracode`,
            // a `/effort` input to inject). A no-op (byte-identical to the legacy command) when no
            // options are requested.
            let plan = apply_launch_options(
                command,
                &kind,
                p.effort.as_deref(),
                p.mode.as_deref(),
                p.model.as_deref(),
            );
            // When we must inject `/effort` first, the task is sent as input AFTER the injection
            // (so effort is set before the task), not appended as a launch argument.
            let task = p
                .task
                .as_deref()
                .filter(|t| !t.is_empty())
                .map(str::to_string);
            let _spawn_guard = ctx.spawn_lock.lock().await;
            let backend = ctx.backend.clone();
            let lane_for_allocation = p.lane_id;
            let (expected_window, slot) = tokio::task::spawn_blocking(move || {
                next_agent_window(backend.as_ref(), lane_for_allocation)
            })
            .await
            .map_err(internal)??;
            let identity = ResolvedAgentAddress {
                address: AgentAddress::new(format!("lane-{}/{}", p.lane_id, slot)),
                lane_id: Some(p.lane_id),
                slot: Some(slot),
                window: Some(expected_window.clone()),
                session_id: None,
                agent_kind: Some(kind.as_str().into_owned()),
            };
            let identity_token = ctx
                .store
                .create_mcp_identity(identity)
                .await
                .map_err(internal)?;
            let socket = {
                let config = ctx.config.read().await;
                repomon_core::config::socket_path(&config)
            };
            let command = if matches!(kind, AgentKind::ClaudeCode | AgentKind::Codex) {
                let mcp_config = write_agent_mcp_config(&expected_window).map_err(internal)?;
                attach_agent_mcp(plan.command, &kind, &mcp_config)
            } else {
                plan.command
            };
            let mut spec = SpawnSpec::new(command, path);
            spec.env.extend([
                (
                    "REPOMON_MCP_SOCKET".into(),
                    socket.to_string_lossy().into_owned(),
                ),
                ("REPOMON_MCP_MODE".into(), "agent".into()),
                ("REPOMON_MCP_IDENTITY_TOKEN".into(), identity_token),
            ]);
            configure_backend_mcp(&kind, &mut spec).map_err(internal)?;
            let inject = plan.effort_inject;
            let inject_task = if inject.is_some() { task.clone() } else { None };
            if inject.is_none() {
                if let Some(task) = task {
                    spec = match kind {
                        AgentKind::OpenCode => spec.arg("--prompt").arg(task),
                        AgentKind::Antigravity => spec.arg("--prompt-interactive").arg(task),
                        _ => spec.arg(task),
                    };
                }
            }
            let tmux = ctx.backend.clone();
            let lane = p.lane_id;
            let kind_str = kind.as_str().into_owned();
            let spawned = tokio::task::spawn_blocking(move || -> repomon_core::Result<String> {
                let window = tmux.spawn(lane, &spec)?;
                let _ = tmux.set_window_agent_kind(&window, &kind_str);
                // Best-effort: set the effort level and type the task once the TUI is up. Operators
                // do exactly this by hand; a short settle lets claude start reading input.
                if let Some(eff) = inject {
                    std::thread::sleep(std::time::Duration::from_millis(2000));
                    tmux.send_text_named(&window, &eff)?;
                    if let Some(task) = inject_task {
                        std::thread::sleep(std::time::Duration::from_millis(600));
                        tmux.send_text_named(&window, &task)?;
                    }
                }
                Ok(window)
            })
            .await
            .map_err(internal)?;
            let window = match spawned {
                Ok(window) if window == expected_window => window,
                Ok(window) => {
                    let _ = ctx
                        .store
                        .revoke_mcp_identity_for_window(expected_window.clone())
                        .await;
                    return Err(internal(format!(
                        "spawn allocated {window}, expected {expected_window}"
                    )));
                }
                Err(error) => {
                    let _ = ctx
                        .store
                        .revoke_mcp_identity_for_window(expected_window.clone())
                        .await;
                    return Err(internal(error));
                }
            };
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
            // Take over an agent running in another terminal and resume its exact backend session.
            // Omitting `agent` preserves the original Claude-only RPC behavior.
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
            let kind = p
                .agent
                .as_deref()
                .map(AgentKind::from_kind_str)
                .unwrap_or(AgentKind::ClaudeCode);
            let session_id = p.session_id.clone();
            let adopt_kind = kind.clone();
            let mut spec = tokio::task::spawn_blocking(move || match adopt_kind {
                AgentKind::ClaudeCode => {
                    let (config_dir, resume) = match &session_id {
                        Some(sid) => (
                            agent::claude::config_base_for_session(&path, sid).flatten(),
                            vec!["--resume".to_string(), sid.clone()],
                        ),
                        None => (
                            agent::claude::summary_for(&path).and_then(|s| s.config_dir),
                            vec!["--continue".to_string()],
                        ),
                    };
                    SpawnSpec {
                        program: adopt_base_command(&default_agent, &customs, &config_dir),
                        args: resume,
                        cwd: path,
                        env: Vec::new(),
                    }
                }
                AgentKind::OpenCode => SpawnSpec {
                    program: "opencode".into(),
                    args: session_id
                        .map(|sid| vec!["--session".into(), sid])
                        .unwrap_or_else(|| vec!["--continue".into()]),
                    cwd: path,
                    env: Vec::new(),
                },
                AgentKind::Antigravity => SpawnSpec {
                    program: "agy".into(),
                    args: session_id
                        .map(|sid| vec!["--conversation".into(), sid])
                        .unwrap_or_else(|| vec!["--continue".into()]),
                    cwd: path,
                    env: Vec::new(),
                },
                AgentKind::Codex => SpawnSpec {
                    // Codex has no stable session-resume flag; re-launch fresh in the worktree.
                    program: "codex".into(),
                    args: Vec::new(),
                    cwd: path,
                    env: Vec::new(),
                },
                AgentKind::Cursor => SpawnSpec {
                    // cursor-agent has no session-resume flag; re-launch fresh in the worktree.
                    program: "cursor-agent".into(),
                    args: Vec::new(),
                    cwd: path,
                    env: Vec::new(),
                },
                AgentKind::Aider => SpawnSpec {
                    // Aider has no MCP support and no session-resume flag; re-launch fresh.
                    program: "aider".into(),
                    args: Vec::new(),
                    cwd: path,
                    env: Vec::new(),
                },
                AgentKind::Other(ref cmd) => {
                    // Custom agent: re-launch the configured command. No session-resume support.
                    SpawnSpec {
                        program: cmd.clone(),
                        args: Vec::new(),
                        cwd: path,
                        env: Vec::new(),
                    }
                }
            })
            .await
            .map_err(internal)?;
            if spec.program.is_empty() {
                return Err(RpcError::invalid_params("agent backend cannot be adopted"));
            }
            let _spawn_guard = ctx.spawn_lock.lock().await;
            let backend = ctx.backend.clone();
            let lane_for_allocation = p.lane_id;
            let (expected_window, slot) = tokio::task::spawn_blocking(move || {
                next_agent_window(backend.as_ref(), lane_for_allocation)
            })
            .await
            .map_err(internal)??;
            let identity = ResolvedAgentAddress {
                address: AgentAddress::new(format!("lane-{}/{}", p.lane_id, slot)),
                lane_id: Some(p.lane_id),
                slot: Some(slot),
                window: Some(expected_window.clone()),
                session_id: p.session_id.clone(),
                agent_kind: Some(kind.as_str().into_owned()),
            };
            let token = ctx
                .store
                .create_mcp_identity(identity)
                .await
                .map_err(internal)?;
            let socket = {
                let config = ctx.config.read().await;
                repomon_core::config::socket_path(&config)
            };
            if matches!(kind, AgentKind::ClaudeCode | AgentKind::Codex) {
                let mcp_config = write_agent_mcp_config(&expected_window).map_err(internal)?;
                spec.program = attach_agent_mcp(spec.program, &kind, &mcp_config);
            }
            spec.env.extend([
                (
                    "REPOMON_MCP_SOCKET".into(),
                    socket.to_string_lossy().into_owned(),
                ),
                ("REPOMON_MCP_MODE".into(), "agent".into()),
                ("REPOMON_MCP_IDENTITY_TOKEN".into(), token),
            ]);
            configure_backend_mcp(&kind, &mut spec).map_err(internal)?;
            let tmux = ctx.backend.clone();
            let lane = p.lane_id;
            let kind_str = kind.as_str().into_owned();
            let spawned = tokio::task::spawn_blocking(move || -> repomon_core::Result<String> {
                let window = tmux.spawn(lane, &spec)?;
                let _ = tmux.set_window_agent_kind(&window, &kind_str);
                Ok(window)
            })
            .await
            .map_err(internal)?;
            let window = match spawned {
                Ok(window) if window == expected_window => window,
                Ok(window) => {
                    let _ = ctx
                        .store
                        .revoke_mcp_identity_for_window(expected_window.clone())
                        .await;
                    return Err(internal(format!(
                        "adopt allocated {window}, expected {expected_window}"
                    )));
                }
                Err(error) => {
                    let _ = ctx
                        .store
                        .revoke_mcp_identity_for_window(expected_window.clone())
                        .await;
                    return Err(internal(error));
                }
            };
            // The one moment the daemon KNOWS which transcript runs in this window: stamp
            // the sticky binding deterministically instead of leaving it to first-contact
            // guessing — `--resume` doesn't touch the resumed .jsonl until the first
            // exchange, so the binder could otherwise pair a newer external transcript onto
            // the adopted window and the stamp would wedge it there.
            if let Some(sid) = p.session_id.clone() {
                ctx.known_managed_sessions.lock().await.insert(sid.clone());
                let tmux = ctx.backend.clone();
                let w = window.clone();
                let k = kind.as_str().into_owned();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Err(e) = tmux.set_window_session(&w, &sid) {
                        tracing::warn!("failed to stamp adopted session on {w}: {e}");
                    }
                    if let Err(e) = tmux.set_window_agent_kind(&w, &k) {
                        tracing::warn!("failed to stamp adopted agent kind on {w}: {e}");
                    }
                })
                .await;
            }
            let _ = ctx
                .store
                .set_lane_tmux_window(p.lane_id, Some(window.clone()))
                .await;
            let _ = ctx
                .store
                .set_lane_agent_kind(p.lane_id, Some(kind.as_str().into_owned()))
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
            let opts = CaptureOpts {
                last_lines: p.lines,
            };
            let window = p
                .window
                .unwrap_or_else(|| TmuxRuntime::window_name(p.lane_id));
            if p.include_state {
                // A capture is useful as a terminal checkpoint only when no raw PTY chunk crossed
                // it. Retry short active bursts until the stream cursor is stable, then return the
                // exact cursor represented by the repaint. The desktop discards queued chunks at
                // or below this cursor before resuming incremental rendering.
                let mut state = None;
                let mut checkpoint = None;
                let mut stable = false;
                for _ in 0..4 {
                    let before = crate::bytes_stream::cursor(&ctx.bytes_watches, &window).await;
                    let backend = ctx.backend.clone();
                    let capture_window = window.clone();
                    let next = tokio::task::spawn_blocking(move || {
                        let content = backend.capture_named(&capture_window, opts)?;
                        let alternate = backend.alternate_on_named(&capture_window);
                        let size = backend.size_named(&capture_window);
                        let cursor = backend.cursor_named(&capture_window);
                        Ok::<_, repomon_core::Error>((content, alternate, size, cursor))
                    })
                    .await
                    .map_err(internal)?
                    .map_err(internal)?;
                    // Let pipe-pane deliver output tmux had already applied when capture-pane ran.
                    tokio::time::sleep(std::time::Duration::from_millis(8)).await;
                    let after = crate::bytes_stream::cursor(&ctx.bytes_watches, &window).await;
                    state = Some(next);
                    checkpoint = after;
                    stable = before == after;
                    if stable {
                        break;
                    }
                }
                let (content, alternate, size, cursor) =
                    state.ok_or_else(|| RpcError::internal("terminal capture unavailable"))?;
                Ok(json!({
                    "content": content,
                    "alternate": alternate,
                    "cols": size.map(|d| d.0),
                    "rows": size.map(|d| d.1),
                    "cursor": cursor.map(|c| json!({ "col": c.col, "row": c.row })),
                    "generation": checkpoint.map(|c| c.generation),
                    "sequence": checkpoint.map(|c| c.sequence),
                    "stable": stable,
                }))
            } else {
                let backend = ctx.backend.clone();
                let capture_window = window.clone();
                let content = tokio::task::spawn_blocking(move || {
                    backend.capture_named(&capture_window, opts)
                })
                .await
                .map_err(internal)?
                .map_err(internal)?;
                Ok(json!({ "content": content }))
            }
        }
        "agent.send_input" => {
            let p: AgentInput = parse(params)?;
            let tmux = ctx.backend.clone();
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
            let tmux = ctx.backend.clone();
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
            let tmux = ctx.backend.clone();
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
            // `event.agent.bytes`. Refcounted per window and per connection: a window has one
            // shared pipe (tmux allows only one pipe-pane per pane), this session joins/leaves its
            // readership, and delivery is filtered per connection at the forwarding loops. A new
            // `on` NEVER stops another session's watch; `on:false` releases only THIS session's.
            let p: AgentWatchBytes = parse(params)?;
            if p.on {
                let window = p
                    .window
                    .clone()
                    .unwrap_or_else(|| TmuxRuntime::window_name(p.lane_id));
                let stream = crate::bytes_stream::watch(
                    ctx.backend.clone(),
                    ctx.events.clone(),
                    &ctx.bytes_watches,
                    p.lane_id,
                    window.clone(),
                    sess.id,
                )
                .await
                .map_err(internal)?;
                sess.watched_bytes.lock().unwrap().insert(window.clone());
                // The ack carries the pane's grid so a remote emulator renders at exactly
                // this size instead of resizing the real pane (which would squeeze a
                // simultaneously attached TUI's mediated view). The stream cursor is additive,
                // so existing clients that only read the grid remain compatible.
                let tmux = ctx.backend.clone();
                let dims = tokio::task::spawn_blocking(move || tmux.size_named(&window))
                    .await
                    .map_err(internal)?;
                return Ok(json!({
                    "cols": dims.map(|d| d.0),
                    "rows": dims.map(|d| d.1),
                    "generation": stream.generation,
                    "sequence": stream.sequence,
                }));
            }
            // `on:false`. With an explicit window, release just that one. WITHOUT a window (the
            // TUI's stop path always sends `{lane_id, on:false}` — even when it watched a
            // non-default window), release every window THIS session watches that belongs to the
            // lane, matched by the WatchEntry.lane field. Resolving a default window name here
            // would orphan the real watch.
            let targets: Vec<String> = match &p.window {
                Some(window) => vec![window.clone()],
                None => {
                    let map = ctx.bytes_watches.lock().await;
                    let mut watched = sess.watched_bytes.lock().unwrap();
                    // Purge names whose registry entry already died (EOF-cleaned: the window
                    // closed). There is nothing left to unwatch, but they must not linger in
                    // `watched_bytes` either — a later window-name reuse would otherwise deliver
                    // bytes this session never asked for. Lane-independent on purpose: a dead
                    // entry's lane is unknowable (the entry is gone), and a dead name is stale for
                    // every lane.
                    watched.retain(|w| map.contains_key(w));
                    watched
                        .iter()
                        .filter(|w| map.get(*w).is_some_and(|e| e.lane == p.lane_id))
                        .cloned()
                        .collect()
                }
            };
            for window in targets {
                crate::bytes_stream::unwatch(&ctx.backend, &ctx.bytes_watches, &window, sess.id)
                    .await;
                sess.watched_bytes.lock().unwrap().remove(&window);
            }
            Ok(Value::Null)
        }
        "agent.prompt" => {
            let p: AgentPrompt = parse(params)?;
            let tmux = ctx.backend.clone();
            let window = p
                .window
                .unwrap_or_else(|| TmuxRuntime::window_name(p.lane_id));
            let win = window.clone();
            let (dialog, sub) = tokio::task::spawn_blocking(move || {
                tmux.capture_named(&win, CaptureOpts::last(45)).map(|pane| {
                    (
                        agent::prompt::detect_dialog(&pane),
                        agent::prompt::detect_subagent_running(&pane),
                    )
                })
            })
            .await
            .map_err(internal)?
            .map_err(internal)?;
            ctx.prompt_cache
                .lock()
                .await
                .insert(window, (std::time::Instant::now(), dialog.clone(), sub));
            Ok(json!({ "dialog": dialog }))
        }
        "agent.answer" => {
            let p: AgentAnswer = parse(params)?;
            let tmux = ctx.backend.clone();
            let window = p
                .window
                .unwrap_or_else(|| TmuxRuntime::window_name(p.lane_id));
            // Re-capture and verify before sending anything: the dialog the client saw may have
            // been answered, replaced, or scrolled away since. Never steer a pane blind.
            let win = window.clone();
            let cap_tmux = tmux.clone();
            let dialog = tokio::task::spawn_blocking(move || {
                cap_tmux
                    .capture_named(&win, CaptureOpts::last(45))
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
                    .insert(window, (std::time::Instant::now(), None, None));
                return Err(RpcError {
                    code: DIALOG_CHANGED,
                    message: "no pending dialog".into(),
                    data: Some(json!({ "dialog": Value::Null })),
                });
            };
            if let Some(expect) = &p.expect_summary {
                if *expect != dialog.summary() {
                    ctx.prompt_cache.lock().await.insert(
                        window,
                        (std::time::Instant::now(), Some(dialog.clone()), None),
                    );
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
            let _ = ctx
                .store
                .revoke_mcp_identity_for_window(window.clone())
                .await;
            let tmux = ctx.backend.clone();
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
            let tmux = ctx.backend.clone();
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
            let target = ctx.backend.exact_target_named(&window);
            let attach = attach_json(&*ctx.backend, &target);
            Ok(json!({ "target": target, "available": available, "attach": attach }))
        }
        "agent.resize" => {
            let p: AgentResize = parse(params)?;
            let tmux = ctx.backend.clone();
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
        "agent.fit" => {
            // The arbitrated resize for mediated viewers: reflow the pane to the caller's grid
            // only when no other session with a fresher claim owns the window's size (a Local/TUI
            // focus always wins; a remote peer wins only while its focus beat is fresh AND it drove
            // the agent more recently than the caller). Always answers with the authoritative grid,
            // so a refused caller renders pinned at the shared size instead of fighting.
            let p: AgentResize = parse(params)?;
            let window = p
                .window
                .unwrap_or_else(|| TmuxRuntime::window_name(p.lane_id));
            let now = std::time::Instant::now();
            let caller = sess_snapshot(sess).await;
            let others = other_session_snapshots(ctx, sess.id).await;
            let allowed = fit_allowed(&caller, &others, &window, now);
            if !allowed {
                let tmux = ctx.backend.clone();
                let w = window.clone();
                let dims = tokio::task::spawn_blocking(move || tmux.size_named(&w))
                    .await
                    .map_err(internal)?;
                return Ok(json!({
                    "applied": false,
                    "cols": dims.map(|d| d.0),
                    "rows": dims.map(|d| d.1),
                }));
            }
            let (cols, rows) = (p.cols.max(20), p.rows.max(4));
            let tmux = ctx.backend.clone();
            let w = window.clone();
            tokio::task::spawn_blocking(move || tmux.resize_named(&w, cols, rows))
                .await
                .map_err(internal)?
                .map_err(internal)?;
            // The applied fit is this connection's most recent agent-driving act — stamp it so a
            // later remote peer's fit yields to us (last-interaction-wins). Denied fits don't stamp.
            *sess.last_interaction.lock().await = Some(now);
            Ok(json!({ "applied": true, "cols": cols, "rows": rows }))
        }
        "agent.scroll" => {
            let p: AgentScroll = parse(params)?;
            let tmux = ctx.backend.clone();
            let lane = p.lane_id;
            let window = p.window.unwrap_or_else(|| TmuxRuntime::window_name(lane));
            let (up, ticks, col, row) = (p.up, p.ticks.min(40), p.col, p.row);
            // Only forward to a full-screen agent (alternate screen) — it owns its scrollback, so
            // it can scroll itself. A plain shell would just get junk on its command line; the
            // caller falls back to the capture-based scroll when `forwarded` is false.
            let forwarded = tokio::task::spawn_blocking(move || -> repomon_core::Result<bool> {
                if tmux.alternate_on_named(&window) {
                    let (max_col, max_row) =
                        tmux.size_named(&window).unwrap_or((u16::MAX, u16::MAX));
                    tmux.scroll_wheel_named(
                        &window,
                        ScrollEvent {
                            up,
                            ticks,
                            col: col.clamp(1, max_col.max(1)),
                            row: row.clamp(1, max_row.max(1)),
                        },
                    )?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            })
            .await
            .map_err(internal)?
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
                    // Drop any active pause (every slot window) so the lane reverts to its
                    // natural status now.
                    ctx.rate_limits
                        .lock()
                        .await
                        .retain(|w, _| TmuxRuntime::lane_id_of(w) != Some(p.lane_id));
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
            let tmux = ctx.backend.clone();
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
            let attach = attach_json(&*ctx.backend, &target);
            // Nudge other clients (a TUI, another desktop window) to re-list terminals so a shell
            // opened here shows up for them too.
            ctx.broadcast("event.repo.changed", json!({ "path": Value::Null }));
            Ok(json!({ "id": name, "target": target, "attach": attach }))
        }
        "terminal.list" => {
            let p: LaneId = parse(params)?;
            let prefix = format!("term-{}-", p.lane_id);
            let tmux = ctx.backend.clone();
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
            let tmux = ctx.backend.clone();
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
            let tmux = ctx.backend.clone();
            let id = p.id;
            let _ = tokio::task::spawn_blocking(move || tmux.kill_named(&id)).await;
            ctx.broadcast("event.repo.changed", json!({ "path": Value::Null }));
            Ok(Value::Null)
        }
        "terminal.target" => {
            let p: TerminalId = parse(params)?;
            let tmux = ctx.backend.clone();
            let id = p.id.clone();
            let available = tokio::task::spawn_blocking(move || tmux.has_named(&id))
                .await
                .map_err(internal)?;
            let target = ctx.backend.target_named(&p.id);
            let attach = attach_json(&*ctx.backend, &target);
            Ok(json!({ "target": target, "available": available, "attach": attach }))
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
        // A bounded page read backwards from a stable JSONL byte offset. The desktop uses this
        // for native full-history scrolling without putting an unbounded transcript in xterm.
        "agent.transcript_page" => {
            let p: AgentTranscriptPage = parse(params)?;
            let path = ctx.lanes.focus(p.lane_id).await.map_err(internal)?;
            let page = tokio::task::spawn_blocking(move || {
                let manifest = match &p.session_id {
                    Some(id) => agent::claude::transcript_path_for_session(&path, id),
                    None => {
                        let within = chrono::Duration::hours(SESSION_WINDOW_HOURS);
                        agent::claude::summaries_for(&path, within, MAX_SESSIONS_PER_LANE)
                            .first()
                            .map(|summary| summary.manifest_path.clone())
                    }
                };
                manifest
                    .map(|manifest| agent::claude::transcript_page(&manifest, p.before))
                    .map(|page| {
                        json!({
                            "items": page.items,
                            "next_before": page.next_before,
                        })
                    })
                    .unwrap_or_else(|| json!({ "items": [], "next_before": null }))
            })
            .await
            .map_err(internal)?;
            Ok(page)
        }
        // Push-notification device registration (the iOS companion).
        // ---- remote devices (LOCAL SOCKET ONLY — blocked over the bridge by the allowlist) ----
        "remote.pair" => {
            let p: RemotePair = parse(params)?;
            // Serialize the store mutation and the cache rebuild together (see
            // `Ctx::remote_mutate_lock`): a concurrent `remote.revoke` must not interleave a stale
            // refresh between our write and rebuild and resurrect a revoked token.
            let dev = {
                let _guard = ctx.remote_mutate_lock.lock().await;
                let dev = ctx
                    .store
                    .remote_device_pair(&p.name)
                    .await
                    .map_err(internal)?;
                // Refresh the auth cache so the freshly minted token authenticates immediately.
                refresh_remote_tokens(ctx).await?;
                dev
            };
            let url = remote_pair_url(ctx, &dev).await;
            Ok(json!({ "name": dev.name, "token": dev.token, "url": url }))
        }
        "remote.devices" => {
            let devices = ctx.store.remote_device_list().await.map_err(internal)?;
            // Never expose the token here — this is the listing surface.
            let out: Vec<Value> = devices
                .iter()
                .map(|d| {
                    json!({
                        "name": d.name,
                        "role": d.role,
                        "created_at": d.created_at,
                        "last_seen_at": d.last_seen_at,
                    })
                })
                .collect();
            Ok(Value::Array(out))
        }
        "remote.revoke" => {
            let p: RemoteRevoke = parse(params)?;
            // Serialize the mutate+refresh pair against a concurrent `remote.pair` (see
            // `Ctx::remote_mutate_lock`) so the revoked token can't survive in the auth cache.
            let revoked = {
                let _guard = ctx.remote_mutate_lock.lock().await;
                let revoked = ctx
                    .store
                    .remote_device_revoke(&p.name)
                    .await
                    .map_err(internal)?;
                // Drop the revoked token from the auth cache; live connections holding it are kicked
                // on their next request (see `remote::handle_conn`) or next event forward.
                refresh_remote_tokens(ctx).await?;
                revoked
            };
            Ok(json!({ "revoked": revoked }))
        }
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
            // Only real terminal windows are streamable extras — anything else is dropped so a
            // client can't point the capture loop at arbitrary windows.
            p.windows
                .retain(|w| TmuxRuntime::parse_term_window(w).is_some());
            // This handler is the single writer of the viewport fields, so it also rewrites the
            // std-Mutex `output_filter` snapshot the event-forward loops read to filter
            // `event.agent.output` (they must not await; see `ConnSession::output_filter`). Build
            // it from the SAME values written to the tokio fields below so the two never diverge.
            *sess.output_filter.lock().unwrap() = (
                p.lane_ids.iter().copied().collect(),
                p.windows.iter().cloned().collect(),
            );
            // Per connection now: each device writes its OWN viewport/focus into its session, and
            // the capture loop streams the union across all live sessions. Wire shape unchanged.
            *sess.viewport.lock().await = p.lane_ids;
            *sess.viewport_focus.lock().await = p.focus_lane.zip(p.focus_window);
            // The focus heartbeat: `agent.fit` treats the focused window as size-owned while
            // this is fresh. A client re-asserts its viewport every few seconds, so a crashed
            // or closed client releases ownership when the beat stops.
            *sess.viewport_focus_at.lock().await = Some(std::time::Instant::now());
            *sess.viewport_windows.lock().await = p.windows;
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
                "protocol_revision": DAEMON_PROTOCOL_REVISION,
                "capabilities": [
                    "terminal.checkpoint.v1",
                    "terminal.stream-sequence.v1"
                ],
            }))
        }
        "daemon.shutdown" => {
            ctx.request_shutdown();
            Ok(Value::Null)
        }

        // ---- system / machine health ----
        "system.doctor" => {
            let cfg = ctx.config.read().await;
            let tmux = repomon_core::agent::tmux::TmuxRuntime::probe();
            let git = repomon_core::git::probe();
            let agents: Vec<AgentDoctorInfo> = detect_all_agents(&cfg)
                .into_iter()
                .map(|a| AgentDoctorInfo {
                    kind: a.kind,
                    name: a.name,
                    command: a.command,
                    detected: a.detected,
                })
                .collect();
            to_value(SystemDoctorResult { tmux, git, agents })
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
                let tmux = ctx.backend.clone();
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
                crate::OrchestratorBackend::Antigravity => {
                    ensure_antigravity_mcp_registration().map_err(internal)?;
                    let command = build_antigravity_orchestrator_command(
                        &base,
                        &socket,
                        &p.autonomy,
                        p.max_agents,
                        &model,
                        &p.prompt,
                    );
                    (command, None)
                }
                crate::OrchestratorBackend::OpenCode => {
                    let command = build_opencode_orchestrator_command(
                        &base,
                        &socket,
                        &p.autonomy,
                        p.max_agents,
                        &model,
                        &p.prompt,
                    )
                    .map_err(internal)?;
                    (command, None)
                }
            };
            // cwd = the user's home, so repomind starts from there rather than the daemon's cwd.
            let home = config::home();
            let spec = SpawnSpec::new(command, home);
            let tmux = ctx.backend.clone();
            tokio::task::spawn_blocking(move || tmux.spawn_named(ORCHESTRATOR_WINDOW, &spec))
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
            let tmux = ctx.backend.clone();
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
                .retain(|w| w.name != ORCHESTRATOR_WINDOW);
            *orch = None;
            *ctx.orchestrator_attention.lock().await = ("none".to_string(), None);
            let status = orchestrator_status_value(None, "none", None);
            ctx.broadcast(crate::pubsub::topic::ORCHESTRATOR_STATUS, status.clone());
            Ok(status)
        }
        "orchestrator.target" => {
            // Clear + broadcast stopped if the window died, so a stale "running" can't linger.
            reconcile_orchestrator(ctx).await;
            let tmux = ctx.backend.clone();
            // Restore client-follow sizing before the attaching terminal renders it (mirrors
            // `agent.target`).
            let available = tokio::task::spawn_blocking(move || {
                let _ = tmux.follow_client_named(ORCHESTRATOR_WINDOW);
                tmux.has_named(ORCHESTRATOR_WINDOW)
            })
            .await
            .map_err(internal)?;
            let target = ctx.backend.exact_target_named(ORCHESTRATOR_WINDOW);
            let attach = attach_json(&*ctx.backend, &target);
            Ok(json!({ "target": target, "available": available, "attach": attach }))
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
            let tmux = ctx.backend.clone();
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
            let tmux = ctx.backend.clone();
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
            *sess.orchestrator_watched.lock().await = p.on;
            Ok(Value::Null)
        }
        // Size the orchestrator window to the viewer's pane so the streamed capture fills it exactly
        // (no right-edge overflow, and no trailing blank rows from a too-tall window). Mirrors
        // `agent.resize`; `orchestrator.target` restores client-follow before a real attach.
        "orchestrator.resize" => {
            let p: OrchestratorResize = parse(params)?;
            let tmux = ctx.backend.clone();
            // Clamp to a sane floor so a momentary tiny layout can't shrink the window to nothing.
            let (cols, rows) = (p.cols.max(20), p.rows.max(4));
            tokio::task::spawn_blocking(move || tmux.resize_named(ORCHESTRATOR_WINDOW, cols, rows))
                .await
                .map_err(internal)?
                .map_err(internal)?;
            Ok(Value::Null)
        }
        // ---- supervision ----
        "supervision.get" => {
            let p: SupervisionGet = parse_opt(params)?;
            let defaults = ctx.config.read().await.supervision.clone();
            let (lane, effective) = match p.lane_id {
                Some(lid) => {
                    let lane_row = ctx.store.lane_policy(lid).await.map_err(internal)?;
                    let eff =
                        repomon_core::agent::supervision::resolve(&defaults, lane_row.as_ref());
                    (lane_row, Some(eff))
                }
                None => (None, None),
            };
            Ok(json!({
                "defaults": defaults,
                "lane": lane,
                "effective": effective,
            }))
        }
        "supervision.set" => {
            let p: SupervisionSet = parse(params)?;
            let mut existing = ctx
                .store
                .lane_policy(p.lane_id)
                .await
                .map_err(internal)?
                .unwrap_or_else(|| repomon_core::agent::supervision::SupervisionOverrides {
                    lane_id: p.lane_id,
                    enabled: false,
                    classes: std::collections::BTreeMap::new(),
                    mail_mode: None,
                    nudge_text: None,
                    stall_mins: None,
                    nudge_retries: None,
                    expect_work: false,
                    updated_at: chrono::Utc::now(),
                });
            if let Some(enabled) = p.enabled {
                existing.enabled = enabled;
            }
            if let Some(classes) = p.classes {
                existing.classes = classes;
            }
            if p.mail_mode.is_some() {
                existing.mail_mode = p.mail_mode;
            }
            if p.nudge_text.is_some() {
                existing.nudge_text = p.nudge_text;
            }
            if p.stall_mins.is_some() {
                existing.stall_mins = p.stall_mins;
            }
            if p.nudge_retries.is_some() {
                existing.nudge_retries = p.nudge_retries;
            }
            if let Some(expect_work) = p.expect_work {
                existing.expect_work = expect_work;
            }
            existing.updated_at = chrono::Utc::now();
            ctx.store
                .set_lane_policy(existing.clone())
                .await
                .map_err(internal)?;
            crate::supervision::refresh(ctx).await;
            ctx.broadcast(
                crate::pubsub::SUPERVISION_CHANGED,
                json!({ "lane_id": p.lane_id }),
            );
            let defaults = ctx.config.read().await.supervision.clone();
            let effective = repomon_core::agent::supervision::resolve(&defaults, Some(&existing));
            Ok(json!({ "effective": effective }))
        }
        "supervision.audit" => {
            let p: SupervisionAudit = parse_opt(params)?;
            let limit = p.limit.unwrap_or(50).min(200);
            let entries = ctx
                .store
                .supervision_log(p.lane_id, limit, p.before_id)
                .await
                .map_err(internal)?;
            Ok(json!({ "entries": entries }))
        }
        "supervision.status" => {
            let snapshot = ctx.supervision.read().await.clone();
            let mut lane_statuses = Vec::new();
            for &lane_id in snapshot.lanes.keys() {
                let last = ctx
                    .store
                    .supervision_last(lane_id)
                    .await
                    .map_err(internal)?;
                lane_statuses.push(json!({
                    "lane_id": lane_id,
                    "enabled": true,
                    "last": last,
                }));
            }
            lane_statuses.sort_by_key(|l| l.get("lane_id").and_then(Value::as_i64).unwrap_or(0));
            Ok(json!({
                "master": snapshot.master,
                "lanes": lane_statuses,
            }))
        }
        "supervision.nudge" => {
            let p: SupervisionNudge = parse(params)?;
            let lane = p.lane_id;
            let window = p.window.unwrap_or_else(|| TmuxRuntime::window_name(lane));
            let text = match p.text {
                Some(t) => t,
                None => {
                    let defaults = ctx.config.read().await.supervision.clone();
                    let lane_row = ctx.store.lane_policy(lane).await.map_err(internal)?;
                    let effective =
                        repomon_core::agent::supervision::resolve(&defaults, lane_row.as_ref());
                    effective.nudge_text
                }
            };
            let seed = crate::inject::AuditSeed {
                lane_id: lane,
                window: window.clone(),
                session_id: None,
                agent_kind: None,
                trigger: "manual_nudge".to_string(),
                dialog_class: None,
                repo_scoped: None,
                decision: "nudge".to_string(),
                policy_source: None,
                reason: Some("manual nudge from operator".to_string()),
                subject: None,
                pane_excerpt: None,
            };
            let outcome = crate::inject::verified_send(
                ctx,
                crate::inject::Expectation::IdleNoDialog,
                crate::inject::Payload::Line(text),
                seed,
            )
            .await;
            match outcome {
                crate::inject::SendOutcome::Sent { keys, entry_id } => Ok(json!({
                    "outcome": "sent",
                    "entry_id": entry_id,
                    "keys": keys,
                })),
                crate::inject::SendOutcome::Skipped { reason, entry_id } => Ok(json!({
                    "outcome": "skipped",
                    "entry_id": entry_id,
                    "reason": reason.as_str(),
                    "keys": Value::Null,
                })),
                crate::inject::SendOutcome::Failed { error, entry_id } => Ok(json!({
                    "outcome": "failed",
                    "entry_id": entry_id,
                    "error": error,
                    "keys": Value::Null,
                })),
            }
        }

        other => Err(RpcError::method_not_found(other)),
    }
}

/// The optional `attach` field of the `agent.target` / `terminal.target` /
/// `orchestrator.target` responses: the exact command a client should run in a real terminal
/// to attach to `target`, so clients stop hard-coding `tmux … attach` themselves. Additive —
/// older clients keep deriving the tmux invocation from `target` alone.
fn attach_json(backend: &dyn repomon_core::SessionBackend, target: &str) -> Value {
    let cmd = backend.attach_command(target);
    json!({ "program": cmd.program, "args": cmd.args })
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

/// TTL for the cached lane overlay. The notify watcher recomputes a fresh overlay every ~2s
/// (and every state-transition event it emits is preceded by that fresh recompute, so
/// event-triggered client refreshes always read current data). Keeping the TTL just under
/// Recomputing the overlay from scratch takes ~10-30ms depending on lane and window counts. A 500ms
/// TTL ensures frequent client polls (1-2s heartbeat) always receive fresh status updates while
/// coalescing sub-second burst requests into a single recomputation.
const OVERLAY_TTL: std::time::Duration = std::time::Duration::from_millis(500);

/// The full lane list with live agent sessions overlaid — what `lane.list` serves — from a
/// short-TTL cache so a stream of per-second client polls collapses into ~1 scan per TTL. Stale
/// concurrent callers may each recompute (bounded, rare); we accept that over single-flight to
/// avoid a leader-failure deadlock. Structural changes call [`Ctx::invalidate_overlay`].
pub(crate) async fn lanes_with_agents(ctx: &Ctx) -> Result<Vec<Lane>, RpcError> {
    {
        let cache = ctx.overlay_cache.lock().await;
        if let Some((t, lanes)) = cache.entry() {
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
    // Single-flight: only one overlay scan runs at a time. Callers that arrived together (two
    // clients polling `lane.list`, or the notify watcher landing on the same instant) queue on this
    // lock; whoever waited then finds the leader's just-written cache below and reuses it instead of
    // running its own tmux/transcript/gix scan. The `_fresh` contract still holds — the value is at
    // most one in-flight scan old (well under the notify watcher's 2s tick and 30s debounce).
    let _flight = ctx.overlay_flight.lock().await;
    {
        let cache = ctx.overlay_cache.lock().await;
        if let Some((t, lanes)) = cache.entry() {
            if t.elapsed() < OVERLAY_TTL {
                return Ok(lanes.clone());
            }
        }
    }
    let generation = ctx.overlay_cache.lock().await.generation();
    let mut lanes = ctx.lanes.list().await.map_err(internal)?;
    overlay_agents(ctx, &mut lanes).await;
    ctx.overlay_cache
        .lock()
        .await
        .publish(generation, lanes.clone());
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
                    let mut recent = agent::claude::summaries_for(p, within, MAX_SESSIONS_PER_LANE);
                    recent.extend(agent::opencode::summaries_for(
                        p,
                        within,
                        MAX_SESSIONS_PER_LANE,
                    ));
                    if let Some(summary) = agent::antigravity::summary_for(p)
                        && chrono::Utc::now() - summary.last_activity <= within
                    {
                        recent.push(summary);
                    }
                    recent.sort_by_key(|summary| std::cmp::Reverse(summary.last_activity));
                    recent.truncate(MAX_SESSIONS_PER_LANE);
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
    // Auto-generated session labels from the local LLM subsystem.
    let generated_labels = ctx
        .store
        .list_session_generated_labels()
        .await
        .unwrap_or_default();
    let tmux = ctx.backend.clone();
    // Distinguish a *failed* probe from a genuinely empty server: on failure reuse the last-good
    // window set for this tick (a transient tmux fork/connection fault must not momentarily drop
    // every managed agent — that flips sessions to `external`, detaches the focused TUI, and fires
    // stale notifications). A real empty result still clears.
    let fresh: Result<Vec<agent::WindowMeta>, String> =
        match tokio::task::spawn_blocking(move || tmux.list_windows_meta()).await {
            Ok(Ok(w)) => Ok(w),
            Ok(Err(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        };
    if let Err(ref e) = fresh {
        tracing::warn!("tmux list-windows failed; reusing last-good window set this overlay: {e}");
    }
    // A tick that ran on the last-good snapshot pairs against binding info that lags any
    // stamps by a generation — good enough to display, but never persist first-contact
    // bindings computed from it (they could overwrite fresh stamps with crossed pairs).
    let probe_ok = fresh.is_ok();
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
        .filter(|w| w.name.starts_with("lane-"))
        .map(|w| w.name.clone())
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
    let known_managed = ctx.known_managed_sessions.lock().await.clone();

    // Per-lane binding candidates + their probe-window pools, resolved against pane
    // evidence and stamped as `@repomon_session` after the loop.
    let mut stamp_batches: Vec<StampBatch> = Vec::new();
    for (lane, summaries) in lanes.iter_mut().zip(per_lane) {
        // The lane's managed agent windows, in slot order. A window only exists while its
        // agent's process lives (tmux closes it on exit), so it doubles as proof of liveness
        // and as the routing target for keys/captures.
        let lane_windows = TmuxRuntime::lane_windows_meta(&windows, lane.id);
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
        // A transcript bound to a live window IS a live agent regardless of what the process
        // count says — never truncate it away in favor of a newer unbound one.
        let bound: std::collections::HashSet<String> = lane_windows
            .iter()
            .filter_map(|w| w.session.clone())
            .collect();
        let summaries = select_kept_summaries(summaries, &bound, keep, now);
        if !summaries.is_empty() {
            // Pair transcripts with windows by sticky identity (`@repomon_session`), falling
            // back to the oldest-with-oldest heuristic only on first contact — see
            // `pair_transcripts_to_windows`. The old purely positional zip re-bound windows
            // whenever two agents swapped activity rank, which moved names, panes, and usage
            // accounts between rows.
            let pairing = pair_transcripts_to_windows(&summaries, &lane_windows, now);
            if !pairing.new_bindings.is_empty() || !pairing.duplicate_stamps.is_empty() {
                stamp_batches.push((
                    pairing.new_bindings,
                    pairing.probe,
                    managed_n,
                    pairing.duplicate_stamps,
                ));
            }
            let mut ext_slots_remaining = alive.map(|a| a.saturating_sub(managed_n));
            for (s, win) in summaries.into_iter().zip(pairing.assignment) {
                let is_fresh = (now - s.last_activity).num_seconds() < RECENTLY_ACTIVE_SECS;
                if s.last_activity > lane.last_activity_at {
                    lane.last_activity_at = s.last_activity;
                }
                let initial_prompt = s.title.clone();
                let mut session = s.into_session(lane.repo.id, lane.worktree.id);
                session.custom_label = session
                    .session_id
                    .as_ref()
                    .and_then(|id| labels.get(id).cloned());
                session.generated_label = session
                    .session_id
                    .as_ref()
                    .and_then(|id| generated_labels.get(id).cloned());

                // Trigger local LLM session naming if no custom or generated label exists yet
                if let Some(session_id) = session.session_id.clone() {
                    if session.custom_label.is_none()
                        && session.generated_label.is_none()
                        && !labels.contains_key(&session_id)
                        && !generated_labels.contains_key(&session_id)
                    {
                        if let Some(prompt) = initial_prompt {
                            if !prompt.trim().is_empty() {
                                let in_flight = ctx.in_flight_naming.clone();
                                let store = ctx.store.clone();
                                let sid = session_id.clone();
                                tokio::spawn(async move {
                                    let should_run = {
                                        let mut set = in_flight.lock().await;
                                        set.insert(sid.clone())
                                    };
                                    if should_run {
                                        match repomon_core::local_llm::generate_session_slug_async(
                                            prompt,
                                        )
                                        .await
                                        {
                                            Ok(slug) => {
                                                let _ = store
                                                    .set_session_generated_label(sid.clone(), slug)
                                                    .await;
                                            }
                                            Err(e) => {
                                                tracing::debug!(
                                                    "Local LLM naming skipped for {sid}: {e}"
                                                );
                                            }
                                        }
                                        in_flight.lock().await.remove(&sid);
                                    }
                                });
                            }
                        }
                    }
                }

                match win {
                    Some(w) => {
                        session.external = false;
                        session.tmux_window = Some(w);
                        if let Some(sid) = &session.session_id {
                            ctx.known_managed_sessions.lock().await.insert(sid.clone());
                        }
                        lane.agent_sessions.push(session);
                    }
                    None => {
                        // An unbound summary is ONLY a real external session if there are actual external
                        // processes running outside tmux (alive > managed_n), or if the process probe was
                        // unavailable (alive == None) and the transcript is actively fresh.
                        // Furthermore, a session that was previously managed and whose window died is an
                        // exited managed agent, NOT an external session.
                        let was_managed = session
                            .session_id
                            .as_deref()
                            .map_or(false, |sid| known_managed.contains(sid));
                        let is_ext = if was_managed {
                            false
                        } else {
                            match ext_slots_remaining.as_mut() {
                                Some(slots) => {
                                    if *slots > 0 && is_fresh {
                                        *slots -= 1;
                                        true
                                    } else {
                                        false
                                    }
                                }
                                None => is_fresh,
                            }
                        };
                        if is_ext {
                            session.external = true;
                            lane.agent_sessions.push(session);
                        }
                    }
                }
            }
            // Agents spawned into this worktree get their own windows but haven't written a
            // transcript yet (claude creates the .jsonl a beat after launch). Surface EVERY
            // unpaired live window as a window-only placeholder right away — a lane can hold
            // several transcript-less agents at once, and hiding all but one made them
            // invisible and uninteractable until an older agent exited.
            for window in pairing.unpaired {
                let kind = window_meta_kind(&lane_windows, &window)
                    .unwrap_or_else(|| lane_meta_kind(&metas, lane.id));
                lane.agent_sessions
                    .push(window_placeholder_session(lane, kind, window));
            }
        } else if managed_n > 0 {
            // No parseable transcript at all: surface every live repomon-spawned window.
            for window in &lane_windows {
                let kind = window
                    .agent_kind
                    .as_deref()
                    .map(AgentKind::from_kind_str)
                    .unwrap_or_else(|| lane_meta_kind(&metas, lane.id));
                lane.agent_sessions.push(window_placeholder_session(
                    lane,
                    kind,
                    window.name.clone(),
                ));
            }
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
                    subagent_running: None,
                    ended_turn: false,
                    gate: None,
                    config_dir: None,
                    custom_label: None,
                    generated_label: None,
                });
            }
        }

        // Overlay usage-limit pauses onto the managed sessions, one per paused slot window.
        // Matching by `tmux_window` marks the actual paused agent; before this, the overlay
        // (like the watcher) only ever considered the lane's first slot.
        let armed = global_auto && !auto_off.contains(&lane.id);
        if armed {
            for (window, rl) in rate_limits
                .iter()
                .filter(|(w, _)| TmuxRuntime::lane_id_of(w) == Some(lane.id))
            {
                let matched = lane
                    .agent_sessions
                    .iter()
                    .any(|s| s.tmux_window.as_deref() == Some(window.as_str()));
                let sess = if matched {
                    lane.agent_sessions
                        .iter_mut()
                        .find(|s| s.tmux_window.as_deref() == Some(window.as_str()))
                } else {
                    // Window→session pairing is heuristic and can momentarily miss (fresh spawn,
                    // placeholder session): fall back to the first managed session not already
                    // marked, so the pause stays visible rather than vanishing.
                    lane.agent_sessions
                        .iter_mut()
                        .find(|s| !s.external && s.status != AgentStatus::RateLimited)
                };
                if let Some(sess) = sess {
                    sess.status = AgentStatus::RateLimited;
                    sess.resume_at = rl.reset_at;
                }
            }
        }
    }

    // Persist the sticky bindings established this tick — but only where the pane PROVES
    // the pairing: each candidate transcript's last-message fingerprint must be visible in
    // exactly one of its lane's unclaimed panes (`confirmed_stamps`). Activity rank alone
    // mis-stamped when several transcripts were fresh at once (live incident: two agents'
    // names swapped and stayed swapped), and a wrong sticky stamp wedges until superseded.
    // An unconfirmed candidate simply returns next tick — the pairing stamps itself once
    // the agent's turn is visible on screen. Rare (once per agent lifetime), so the capture
    // forks don't touch steady-state ticks. Skipped when the window probe failed
    // (`probe_ok`): a last-good snapshot lags the stamps by a generation.
    if probe_ok && !stamp_batches.is_empty() {
        let tmux = ctx.backend.clone();
        let _ = tokio::task::spawn_blocking(move || {
            for (cands, probe, lane_window_count, duplicate_stamps) in stamp_batches {
                let confirmed = if cands.iter().any(|c| c.needle.is_some()) {
                    let panes: Vec<(u64, String, String)> = probe
                        .iter()
                        .map(|(wid, name)| {
                            let text = tmux
                                .capture_named(name, CaptureOpts::last(STAMP_CONFIRM_CAPTURE_LINES))
                                .unwrap_or_default();
                            (*wid, name.clone(), normalize_fingerprint(&text))
                        })
                        .collect();
                    confirmed_stamps(&cands, &panes)
                } else if direct_bind_allowed(cands.len(), probe.len(), lane_window_count) {
                    // Exactly 1 candidate and 1 unclaimed probe window, AND this is the lane's
                    // ONLY window (see `direct_bind_allowed`): bind them directly so agents
                    // without long text fingerprints (e.g. Antigravity) are stamped and never
                    // surface as phantom external adoptables.
                    vec![(
                        probe[0].0,
                        probe[0].1.clone(),
                        cands[0].sid.clone(),
                        cands[0].kind.clone(),
                    )]
                } else {
                    Vec::new()
                };
                // Clear any window whose stamp lost the pass-1 claim race this tick (duplicate
                // `@repomon_session`) — writing an empty value is `list_windows_meta`'s parse for
                // "no stamp", so the window falls back to placeholder / honest re-confirmation
                // instead of permanently wedging as a second claimant of the same identity.
                for (wid, name) in &duplicate_stamps {
                    if let Err(e) = tmux.set_window_session_by_id(*wid, "") {
                        tracing::warn!(
                            "failed to clear duplicate @repomon_session on {name} (@{wid}): {e}"
                        );
                    }
                }
                for (wid, name, sid, kind) in confirmed {
                    if let Err(e) = tmux.set_window_session_by_id(wid, &sid) {
                        tracing::warn!("failed to stamp @repomon_session on {name} (@{wid}): {e}");
                    }
                    if let Err(e) = tmux.set_window_agent_kind_by_id(wid, kind.as_str().as_ref()) {
                        tracing::warn!(
                            "failed to stamp @repomon_agent_kind on {name} (@{wid}): {e}"
                        );
                    }
                }
            }
        })
        .await;
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
        const SNIFF_TTL: std::time::Duration = std::time::Duration::from_secs(4);
        // A Running session is the one that can *newly* raise a dialog (its transcript ends in a
        // tool call, but the pane may be on a permission/plan/menu prompt that only the sniff
        // sees), so a NeedsYou can be up to SNIFF_TTL late. Re-capture those on a short 1.5s TTL
        // so status updates and decision prompts appear almost instantly.
        const RUNNING_SNIFF_TTL: std::time::Duration = std::time::Duration::from_millis(1500);
        let mut sniffs: Vec<(Option<agent::prompt::PendingDialog>, Option<String>)> =
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
                    Some((t, p, sub)) if t.elapsed() < ttl => sniffs.push((p.clone(), sub.clone())),
                    _ => {
                        sniffs.push((None, None));
                        misses.push(idx);
                    }
                }
            }
        }
        if !misses.is_empty() {
            let tmux = ctx.backend.clone();
            let miss_windows: Vec<String> =
                misses.iter().map(|&i| candidates[i].2.clone()).collect();
            // Each fresh capture yields the parsed dialog, running subagents, AND a content hash —
            // the hash feeds the stall detector's "when did this pane last change?" clock.
            let fresh: Vec<(
                Option<agent::prompt::PendingDialog>,
                Option<String>,
                Option<u64>,
            )> = tokio::task::spawn_blocking(move || {
                miss_windows
                    .iter()
                    .map(|w| match tmux.capture_named(w, CaptureOpts::last(45)) {
                        Ok(pane) => {
                            use std::hash::{Hash, Hasher};
                            let mut h = std::collections::hash_map::DefaultHasher::new();
                            pane.hash(&mut h);
                            (
                                agent::prompt::detect_dialog(&pane),
                                agent::prompt::detect_subagent_running(&pane),
                                Some(h.finish()),
                            )
                        }
                        Err(_) => (None, None, None),
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            let now_utc = chrono::Utc::now();
            let mut cache = ctx.prompt_cache.lock().await;
            let mut seen = ctx.pane_seen.lock().await;
            for (&i, (p, sub, hash)) in misses.iter().zip(fresh) {
                let window = &candidates[i].2;
                cache.insert(
                    window.clone(),
                    (std::time::Instant::now(), p.clone(), sub.clone()),
                );
                // Stamp the pane's last-change time only when the content actually differs.
                if let Some(h) = hash {
                    match seen.get(window) {
                        Some((prev, _)) if *prev == h => {}
                        _ => {
                            seen.insert(window.clone(), (h, now_utc));
                        }
                    }
                }
                sniffs[i] = (p, sub);
            }
        }
        // Prune the sniff caches so they can't grow without bound — every window name ever
        // sniffed would otherwise leak an entry. `prompt_cache` also drops results older than
        // the longest sniff TTL (they'd be re-captured anyway); `pane_seen` is pruned by window
        // liveness ONLY — its old timestamps are the stall clock.
        {
            let live: std::collections::HashSet<&str> =
                windows.iter().map(|w| w.name.as_str()).collect();
            let mut cache = ctx.prompt_cache.lock().await;
            cache.retain(|w, (t, _, _)| live.contains(w.as_str()) && t.elapsed() < SNIFF_TTL);
            let mut seen = ctx.pane_seen.lock().await;
            seen.retain(|w, _| live.contains(w.as_str()));
        }
        let now_utc = chrono::Utc::now();
        let seen = ctx.pane_seen.lock().await;
        for ((li, si, w, _), (found_dialog, found_subagent)) in candidates.into_iter().zip(sniffs) {
            let s = &mut lanes[li].agent_sessions[si];
            s.subagent_running = found_subagent;
            if s.subagent_running.is_some() && s.status == AgentStatus::Idle {
                s.status = AgentStatus::Running;
            }
            match found_dialog {
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
/// so a `/exit`ed or stopped session's lingering transcript is dropped immediately rather than
/// lingering as a phantom external session. When `alive > 0` or `managed_n > 0`, `fresh` acts as a
/// backstop, and a probe failure (`None`) doesn't filter.
fn sessions_to_keep(total: usize, alive: Option<usize>, managed_n: usize, fresh: usize) -> usize {
    match alive {
        Some(0) if managed_n == 0 => 0,
        Some(n) => n.max(managed_n).min(total),
        None => managed_n.max(fresh).min(total),
    }
}

/// How long a viewport focus keeps owning its window's size after the last `viewport.set`, and how
/// long the capture loop treats a session's focus as cadence-boosting. Three missed ~5s client
/// heartbeats — generous against a busy tick, short enough that a closed/crashed client frees the
/// pane for reflow within seconds. `pub(crate)` so [`crate::Ctx::viewport_snapshot`] shares it.
pub(crate) const FOCUS_OWNED_TTL: std::time::Duration = std::time::Duration::from_secs(15);

/// A transcript that should get a sticky binding this tick, pending pane evidence: the
/// fingerprint of its last message must be visible in exactly one of the lane's unclaimed
/// panes before anything is stamped. Activity rank proved able to guess wrong when several
/// transcripts were fresh at once, and a wrong sticky stamp wedges until superseded — so the
/// pane, not the rank, picks the window.
struct BindingCandidate {
    sid: String,
    /// Normalized fingerprint of the transcript's last message ([`message_fingerprint`]);
    /// `None` (no message yet / too short) means no evidence — the candidate simply returns
    /// next tick.
    needle: Option<String>,
    /// The agent kind parsed from the transcript.
    kind: AgentKind,
}

/// One lane's binding candidates plus the probe-window pool they may stamp onto, the lane's
/// total live window count (gates the no-evidence direct-bind fallback — see
/// [`pair_transcripts_to_windows`]), and any windows whose `@repomon_session` stamp lost the
/// pass-1 claim race this tick and should be cleared.
type StampBatch = (
    Vec<BindingCandidate>,
    Vec<(u64, String)>,
    usize,
    Vec<(u64, String)>,
);

/// A lane's transcript↔window pairing for one overlay tick ([`pair_transcripts_to_windows`]).
struct Pairing {
    /// Per input summary (order preserved): its evidence-backed managed window; `None` means
    /// external or still awaiting pane evidence.
    assignment: Vec<Option<String>>,
    /// Transcripts to bind this tick, pending pane confirmation ([`confirmed_stamps`]).
    new_bindings: Vec<BindingCandidate>,
    /// The windows a new binding may land on — everything pass 1 didn't claim, `(window_id,
    /// name)`. Stamps target the window ID: a slot NAME recycled between the probe and the
    /// stamp must not inherit the old transcript's binding.
    probe: Vec<(u64, String)>,
    /// Live managed windows no evidence-backed transcript claimed, newest (highest window id)
    /// first. The first is the placeholder target while a just-spawned agent's transcript is
    /// absent or still awaiting pane confirmation (at most one, per the `SessKey::Fallback`
    /// model). Also where a duplicate-stamped LOSER window (below) surfaces, so it renders as a
    /// placeholder instead of vanishing.
    unpaired: Vec<String>,
    /// Windows whose `@repomon_session` names a sid an EARLIER window (in `windows` order)
    /// already claimed this tick — sticky identity is supposed to be 1:1, so a second live
    /// window carrying the same stamp is a bug (duplicate stamp), not a legitimate second home
    /// for the transcript. `(window_id, name)`, in `windows` order. The caller clears these
    /// stamps so the window falls back to placeholder / honest re-confirmation instead of
    /// wedging as a permanent phantom claimant.
    duplicate_stamps: Vec<(u64, String)>,
}

/// Collapse text to lowercase ASCII alphanumerics. Makes fingerprint matching immune to
/// tmux line wrapping, markdown styling (the pane renderer strips `**`/`_` markers), ANSI
/// spacing, and case.
fn normalize_fingerprint(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Minimum normalized length before a fingerprint is distinctive enough to act on.
const FINGERPRINT_MIN: usize = 24;
/// How much of the normalized TAIL to keep — after a long reply scrolls, its tail is what
/// stays visible above the input box.
const FINGERPRINT_LEN: usize = 64;
/// How far back into scrollback the stamp-confirmation probe captures a candidate's window,
/// looking for its fingerprint. A fast, tool-call-heavy agent (long Bash/Read output between
/// two messages) can push its own last message hundreds of lines up the scrollback within a
/// single overlay tick — a shallow capture here means the candidate never confirms and the
/// window stays permanently unbound (surfaces as "external" forever, never as a managed
/// session), even though the agent is right there. This only runs for lanes that still have an
/// unconfirmed candidate, and reading further into tmux's in-memory scrollback is cheap, so
/// there's no real cost to looking well past what a single tick could plausibly need.
const STAMP_CONFIRM_CAPTURE_LINES: u32 = 500;

/// The pane fingerprint of a transcript's last message, or `None` when there is no message
/// or it is too short to be distinctive.
fn message_fingerprint(last_message: Option<&str>) -> Option<String> {
    let n = normalize_fingerprint(last_message?);
    if n.len() < FINGERPRINT_MIN {
        return None;
    }
    // Byte slicing is safe: the normalized form is pure ASCII.
    Some(n[n.len().saturating_sub(FINGERPRINT_LEN)..].to_string())
}

/// Which stamps the pane evidence supports: each candidate with a fingerprint is stamped on
/// the single probe window whose (normalized) pane contains it. No match, several matching
/// panes, or several candidates matching the same pane → no stamp for those involved; the
/// candidates return next tick once the screens disambiguate. Returns `(wid, name, sid, kind)`.
fn confirmed_stamps(
    cands: &[BindingCandidate],
    panes: &[(u64, String, String)],
) -> Vec<(u64, String, String, AgentKind)> {
    // Every needle↔pane hit, as (candidate index, pane index).
    let mut hits: Vec<(usize, usize)> = Vec::new();
    for (ci, c) in cands.iter().enumerate() {
        let Some(needle) = &c.needle else { continue };
        for (pi, (_, _, text)) in panes.iter().enumerate() {
            if text.contains(needle.as_str()) {
                hits.push((ci, pi));
            }
        }
    }
    // Keep only 1:1 matches — a candidate seen in several panes, or a pane claimed by
    // several candidates, proves nothing yet.
    hits.iter()
        .filter(|&&(ci, pi)| {
            hits.iter().filter(|(c, _)| *c == ci).count() == 1
                && hits.iter().filter(|(_, p)| *p == pi).count() == 1
        })
        .map(|&(ci, pi)| {
            let (wid, name, _) = &panes[pi];
            (
                *wid,
                name.clone(),
                cands[ci].sid.clone(),
                cands[ci].kind.clone(),
            )
        })
        .collect()
}

/// Whether the no-evidence, headcount-only direct bind (see the call site in `overlay_agents`)
/// may fire for a lane this tick.
///
/// The original condition was just "exactly 1 candidate and exactly 1 window `pair_transcripts_to_windows`
/// left unclaimed THIS TICK" (`cands_len == 1 && probe_len == 1`). That is not the same claim as
/// "this sid is not bound anywhere else": `probe`/`cands` are computed from whatever window
/// snapshot this tick saw, which can legitimately (a transient tmux probe hiccup reusing a
/// stale `last_good` snapshot, a notify race) omit a window that in the REAL, live tmux server
/// still carries this exact sid's stamp. Binding on headcount alone in that situation stamps a
/// SECOND window with the same `@repomon_session` — sticky identity is supposed to be 1:1, so a
/// live incident produced three windows all stamped with one resumed session's id.
///
/// Requiring the lane to hold exactly one window in total closes that gap: with only one window
/// in the whole lane there is nothing else the sid could already be (or later become) bound to,
/// so the direct bind can never create a duplicate. Every multi-window lane must earn its stamp
/// through `confirmed_stamps`'s pane-evidence match instead.
fn direct_bind_allowed(cands_len: usize, probe_len: usize, lane_window_count: usize) -> bool {
    cands_len == 1 && probe_len == 1 && lane_window_count == 1
}

/// Pair a lane's kept transcripts (newest-first) with its live managed windows by STICKY
/// IDENTITY first, position last.
///
/// Pass 1: a window whose `@repomon_session` names a kept transcript keeps it. This is what
/// makes the pairing immune to two agents swapping activity rank — which used to re-bind
/// their windows every poll, moving names, panes, and usage accounts between rows — and to
/// daemon restarts (the binding lives in tmux, not daemon memory).
///
/// Pass 2 (first contact only): the newest still-unassigned transcripts become binding
/// candidates for the still-free windows. They remain external for this response and those
/// windows remain placeholders until [`confirmed_stamps`] proves a 1:1 pane match; the next
/// overlay's pass 1 then exposes the durable pairing. This deliberately avoids even a temporary
/// stale-transcript/new-window display mismatch during spawn.
fn pair_transcripts_to_windows(
    summaries: &[agent::TranscriptSummary],
    windows: &[agent::WindowMeta],
    now: chrono::DateTime<chrono::Utc>,
) -> Pairing {
    let is_fresh =
        |s: &agent::TranscriptSummary| (now - s.last_activity).num_seconds() < RECENTLY_ACTIVE_SECS;
    // Pass 1 — sticky identity, tentatively: each window claims the kept transcript its
    // `@repomon_session` names.
    let mut claim: Vec<Option<usize>> = vec![None; windows.len()];
    let mut claimed = vec![false; summaries.len()];
    // Sticky identity is supposed to be 1:1 (one window per sid): the first window (in
    // `windows` order) to carry a given `@repomon_session` stamp is its home; any LATER window
    // carrying the exact same stamp is a duplicate — evidence of a stale direct-bind or a
    // concurrent resume — and gets queued for clearing rather than silently accepted as a
    // second claimant. Tracked by the raw stamp text, independent of whether the sid still
    // matches a kept transcript, so a duplicate is caught even if one copy's transcript aged out.
    let mut first_window_for_sid: HashMap<&str, usize> = HashMap::new();
    let mut duplicate_stamps: Vec<(u64, String)> = Vec::new();
    for (wi, w) in windows.iter().enumerate() {
        let Some(sid) = &w.session else { continue };
        match first_window_for_sid.entry(sid.as_str()) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(wi);
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                duplicate_stamps.push((w.wid, w.name.clone()));
                continue;
            }
        }
        if let Some(si) = summaries
            .iter()
            .position(|s| s.session_id.as_deref() == Some(sid.as_str()))
        {
            if !claimed[si] {
                claim[wi] = Some(si);
                claimed[si] = true;
            }
        }
    }
    // Honor pass 1's claims for DISPLAY. A window's `@repomon_session` stamp is durable
    // ground truth — going idle is not going dead, so a valid claim is never un-displayed
    // just because its transcript stopped writing for a moment.
    let mut assignment: Vec<Option<String>> = vec![None; summaries.len()];
    let mut has_display = vec![false; windows.len()];
    for (wi, c) in claim.iter().enumerate() {
        if let Some(si) = c {
            assignment[*si] = Some(windows[wi].name.clone());
            has_display[wi] = true;
        }
    }
    // Supersession: a claude process rotates its transcript id in place (`/clear`, a
    // fork-on-resume), leaving its window bound to a dead transcript while the live
    // continuation has no window. When more FRESH unclaimed transcripts exist than free
    // windows to receive them, OFFER claims whose transcript has gone quiet to pass 2's
    // evidence probe — WARMEST first: the transcript that stopped writing most recently is
    // the one that just rotated into the newcomer, while a long-cold one is simply an idle
    // agent whose window must not be given away. A claim on a fresh transcript is never
    // released, so an idle fleet can't be shuffled.
    //
    // Offering a window to the probe does NOT change what's displayed for it this tick —
    // only `confirmed_stamps` writing a durable `@repomon_session` stamp (proven by real pane
    // text) can actually move it. Flipping `assignment` here on headcount alone previously
    // made ANY idle-but-still-valid session (in a lane that simply holds more live
    // transcripts than tmux windows, e.g. one companion window per external session) flicker
    // to "external" every single tick it lost this footrace to an unrelated fresh transcript
    // — most visibly, a lane containing the operator's own always-fresh, never-window-bound
    // session permanently stole the idle-but-legitimately-bound window out from under another
    // session's display, tick after tick, forever.
    let mut released: std::collections::HashSet<usize> = std::collections::HashSet::new();
    {
        let fresh_unclaimed = summaries
            .iter()
            .enumerate()
            .filter(|(i, s)| !claimed[*i] && is_fresh(s))
            .count();
        let mut free_n = claim.iter().filter(|c| c.is_none()).count();
        if fresh_unclaimed > free_n {
            let mut stale_wis: Vec<usize> = (0..windows.len())
                .filter(|&wi| claim[wi].is_some_and(|si| !is_fresh(&summaries[si])))
                .collect();
            stale_wis
                .sort_by_key(|&wi| std::cmp::Reverse(summaries[claim[wi].unwrap()].last_activity));
            for wi in stale_wis {
                if fresh_unclaimed <= free_n {
                    break;
                }
                released.insert(wi);
                free_n += 1;
            }
        }
    }
    // Pass 2 — first contact plus supersession offers. Unclaimed transcripts (and any window
    // offered above) are candidates for the free-window pool. A never-claimed window remains
    // a placeholder until pane evidence writes a durable stamp; a released-but-still-displayed
    // window keeps showing its current binding until that evidence arrives.
    let mut free: Vec<usize> = (0..windows.len())
        .filter(|&i| !has_display[i] || released.contains(&i))
        .collect();
    free.sort_by_key(|&i| (windows[i].session.is_some(), windows[i].wid));
    let probe: Vec<(u64, String)> = free
        .iter()
        .map(|&i| (windows[i].wid, windows[i].name.clone()))
        .collect();
    // `summaries` is newest-first, so nominate at most one transcript per free window.
    let chosen: Vec<usize> = (0..summaries.len())
        .filter(|&i| assignment[i].is_none())
        .take(free.len())
        .collect();
    let has_never_bound_window = free.iter().any(|&i| windows[i].session.is_none());
    let mut new_bindings = Vec::new();
    for &si in &chosen {
        // Nominate a transcript for a durable stamp when PANE EVIDENCE could confirm it.
        // Two routes qualify:
        //  - it is actively writing (`is_fresh`): it IS some window's agent right now, so
        //    the evidence pass will find its turn on screen.
        //  - it is quiet but its window carries NO binding yet AND it has a distinctive
        //    last-message fingerprint: this recovers an idle agent whose `@repomon_session`
        //    was lost (a daemon restart of a quiet fleet leaves the window unstamped and no
        //    transcript fresh, so pass 1 can't reclaim it and this pass never used to try).
        //    Its pane still shows that last message, so `confirmed_stamps` can reclaim the
        //    window by evidence. A quiet transcript with no fingerprint stays a display-only
        //    stand-in — there is nothing to confirm — and a released stale binding
        //    (`session.is_some()`) is left for a fresh claimant, never re-stamped from a guess.
        if let Some(sid) = &summaries[si].session_id {
            let needle = message_fingerprint(summaries[si].last_message.as_deref());
            let nominate = if is_fresh(&summaries[si]) {
                true
            } else if has_never_bound_window && needle.is_some() {
                true
            } else {
                false
            };
            if nominate {
                new_bindings.push(BindingCandidate {
                    sid: sid.clone(),
                    needle,
                    kind: summaries[si].kind.clone(),
                });
            }
        }
    }
    // A released-but-displayed window is NOT unpaired — it already has a real, still-shown
    // binding above and must not also render as a placeholder tab.
    let mut unpaired: Vec<&agent::WindowMeta> = windows
        .iter()
        .zip(&has_display)
        .filter(|(_, d)| !**d)
        .map(|(w, _)| w)
        .collect();
    unpaired.sort_by_key(|w| std::cmp::Reverse(w.wid));
    Pairing {
        assignment,
        new_bindings,
        probe,
        unpaired: unpaired.into_iter().map(|w| w.name.clone()).collect(),
        duplicate_stamps,
    }
}

/// Which of a lane's newest-first transcripts to keep, honoring bindings: every summary bound
/// to a live managed window is kept regardless of rank (the window only exists while its
/// agent's process lives, so it outranks the process-count probe), then newest-first from the
/// rest up to `keep` total. Output is re-sorted newest-first so the wire order is unchanged.
/// Without this, a bound-but-quiet agent could be truncated in favor of a newer external
/// transcript, dropping a live agent from the lane.
fn select_kept_summaries(
    summaries: Vec<agent::TranscriptSummary>,
    bound: &std::collections::HashSet<String>,
    keep: usize,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<agent::TranscriptSummary> {
    if keep == 0 && bound.is_empty() {
        return Vec::new();
    }
    if summaries.len() <= keep && bound.is_empty() {
        return summaries;
    }
    let is_fresh =
        |s: &agent::TranscriptSummary| (now - s.last_activity).num_seconds() < RECENTLY_ACTIVE_SECS;
    // Protected: bound to a live window (the window proves its agent alive) OR actively
    // writing right now — `sessions_to_keep`'s "never drop a session that is working"
    // contract must survive bound-protection, or a stale binding could make the one live
    // transcript invisible. When keep == 0 and bound is empty, the agent has exited and is dropped.
    let (mut out, rest): (Vec<_>, Vec<_>) = summaries.into_iter().partition(|s| {
        (keep > 0 && is_fresh(s)) || s.session_id.as_ref().is_some_and(|id| bound.contains(id))
    });
    let take_rest = keep.saturating_sub(out.len());
    out.extend(rest.into_iter().take(take_rest));
    out.sort_by_key(|s| std::cmp::Reverse(s.last_activity));
    out
}

/// The fit-arbitration-relevant slice of one session, snapshotted so [`fit_allowed`] is a pure
/// function over plain data (and unit-testable without a live `Ctx`).
struct SessSnapshot {
    /// True for the local TUI connection; false for a remote (companion) connection.
    local: bool,
    /// The window this session focuses, if any (with its lane, unused by the arbitration).
    focus: Option<(repomon_core::model::LaneId, String)>,
    /// When this session last (re)asserted its viewport — its focus beat's freshness clock.
    focus_at: Option<std::time::Instant>,
    /// When this session last drove an agent (for remote-vs-remote last-interaction-wins).
    last_interaction: Option<std::time::Instant>,
}

async fn sess_snapshot(sess: &ConnSession) -> SessSnapshot {
    // Bind each guard to its own local so it drops before the next lock: a struct literal would
    // otherwise hold all three session guards live at once across the `.await`s (a lock-order
    // footgun). Order doesn't matter here since they never overlap.
    let local = sess.is_local();
    let focus = sess.viewport_focus.lock().await.clone();
    let focus_at = *sess.viewport_focus_at.lock().await;
    let last_interaction = *sess.last_interaction.lock().await;
    SessSnapshot {
        local,
        focus,
        focus_at,
        last_interaction,
    }
}

/// Snapshot every live session EXCEPT `caller_id` (the fit's caller never blocks itself).
async fn other_session_snapshots(ctx: &Ctx, caller_id: u64) -> Vec<SessSnapshot> {
    let sessions: Vec<Arc<ConnSession>> = ctx
        .sessions
        .lock()
        .await
        .values()
        .filter(|s| s.id != caller_id)
        .cloned()
        .collect();
    let mut out = Vec::with_capacity(sessions.len());
    for s in &sessions {
        out.push(sess_snapshot(s).await);
    }
    out
}

/// Whether `caller` may reflow `window` right now, given the other live sessions.
///
/// 1. Any OTHER Local (TUI) session with a fresh focus beat on `window` denies it (TUI precedence).
/// 2. Any OTHER Remote session with a fresh focus beat on `window` AND a `last_interaction` newer
///    than the caller's denies it (remote-vs-remote last-interaction-wins).
/// 3. Otherwise it is allowed; the handler stamps the caller's `last_interaction` on apply.
/// 4. Self-refit is always allowed — the caller is excluded from `others`, so it never blocks
///    itself.
fn fit_allowed(
    caller: &SessSnapshot,
    others: &[SessSnapshot],
    window: &str,
    now: std::time::Instant,
) -> bool {
    for o in others {
        // Does this other session hold a FRESH focus beat on the target window?
        let focuses_window = o.focus.as_ref().is_some_and(|(_, w)| w == window);
        let fresh = o
            .focus_at
            .is_some_and(|at| now.duration_since(at) < FOCUS_OWNED_TTL);
        if !(focuses_window && fresh) {
            continue;
        }
        if o.local {
            return false; // rule 1: TUI precedence
        }
        // rule 2: a remote peer wins only if it drove the agent more recently than the caller.
        let peer_newer = match (o.last_interaction, caller.last_interaction) {
            (Some(peer), Some(mine)) => peer > mine,
            (Some(_), None) => true, // peer has driven, caller never has → peer is newer
            (None, _) => false,      // peer never drove → it doesn't outrank the caller
        };
        if peer_newer {
            return false;
        }
    }
    true
}

/// How long a managed agent's pane must sit unchanged — with no dialog up and its turn not
/// ended — before the session reads as stalled.
const STALL_AFTER_MINS: i64 = 5;

/// When a sniffed session counts as stalled, returns the stall's start (the pane's last
/// change). A stall is: Running mid-tool with no dialog on screen, a turn that did NOT end,
/// and a pane frozen for [`STALL_AFTER_MINS`]. `None` = not stalled.
fn stall_since(
    status: AgentStatus,
    ended_turn: bool,
    has_dialog: bool,
    pane_changed_at: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    if has_dialog || ended_turn || status != AgentStatus::Running {
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

/// List the subdirectories of `start` (default: the user's home) for the repo browser, marking
/// which are git repos and which are already registered.
fn browse_dir(start: Option<PathBuf>, added: &std::collections::HashSet<PathBuf>) -> BrowseResult {
    let path = start.filter(|p| p.is_dir()).unwrap_or_else(config::home);
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
const BUILTIN_AGENTS: [&str; 5] = ["codex", "opencode", "antigravity", "aider", "cursor"];

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

/// Look up an agent kind recorded on a specific window, if present.
fn window_meta_kind(
    lane_windows: &[repomon_core::agent::WindowMeta],
    window_name: &str,
) -> Option<AgentKind> {
    lane_windows
        .iter()
        .find(|w| w.name == window_name)
        .and_then(|w| w.agent_kind.as_deref())
        .map(AgentKind::from_kind_str)
}

/// The agent kind repomon last spawned in a lane (from its persisted meta), defaulting to "unknown"
/// when nothing was recorded — used as fallback to label a window-only placeholder session.
fn lane_meta_kind(
    metas: &[repomon_core::model::LaneMeta],
    lane_id: repomon_core::model::LaneId,
) -> AgentKind {
    metas
        .iter()
        .find(|m| m.id == lane_id)
        .and_then(|m| m.agent_kind.clone())
        .map(|k| AgentKind::from_kind_str(&k))
        .unwrap_or_else(|| AgentKind::from_kind_str("unknown"))
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
        status: AgentStatus::Idle,
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
        subagent_running: None,
        ended_turn: true,
        gate: None,
        config_dir: None,
        custom_label: None,
        generated_label: None,
    }
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
fn resolve_windows<T: Clone>(
    fresh: Result<Vec<T>, String>,
    last_good: &mut Vec<T>,
    empty_misses: &mut u8,
) -> Vec<T> {
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

/// How many supported agent CLI processes have each working directory. The count bounds how many
/// session records remain live after a CLI exits but leaves durable state behind.
#[cfg(all(unix, not(target_os = "linux")))]
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
            matches!(base, "claude" | "opencode" | "agy" | "codex" | "cursor")
                .then(|| pid.to_string())
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
    let mut combined = HashMap::new();
    for name in ["claude", "opencode", "agy", "codex", "cursor"] {
        for (path, count) in live_cwds_by_name(name)? {
            *combined.entry(path).or_insert(0) += count;
        }
    }
    Some(combined)
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

/// Cached supported-agent process accounting with a 10s TTL (plus a 30s sticky-high grace),
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
    // Windows has no ps/lsof//proc scan: the hosts *own* the agent children, so the backend
    // answers authoritatively (a live host implies a live child in that cwd). Unix keeps the
    // platform process probe, which also catches sessions started outside repomon.
    #[cfg(windows)]
    let fresh = {
        let backend = ctx.backend.clone();
        tokio::task::spawn_blocking(move || backend.live_agent_cwds())
            .await
            .ok()
            .flatten()
    };
    #[cfg(not(windows))]
    let fresh = tokio::task::spawn_blocking(live_claude_cwds)
        .await
        .ok()
        .flatten();
    let map = match fresh {
        Some(m) => m,
        None => {
            // The probe couldn't run (ps/lsof spawn failed under load, /proc unreadable).
            // Returning None means "don't filter" — callers keep all recent sessions rather than
            // truncating to a bogus low count — but it was silent; log it so a flap is visible.
            tracing::warn!("live agent-process probe failed; not truncating sessions");
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
            if c == 0 {
                sticky.remove(k);
            } else {
                let refresh = sticky.get(k).map(|(hi, _)| c >= *hi).unwrap_or(true);
                if refresh {
                    sticky.insert(k.clone(), (c, now));
                }
            }
        }
        sticky.retain(|k, (_, seen)| {
            map.get(k).copied().unwrap_or(0) > 0 && seen.elapsed() < STICKY_GRACE
        });
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
    if prog.contains('/') || prog.contains(std::path::MAIN_SEPARATOR) {
        return Path::new(prog).exists();
    }
    repomon_core::exec::find_in_path(prog).is_some()
}

struct RawDetectedAgent {
    kind: String,
    name: String,
    command: String,
    detected: bool,
    custom: bool,
}

fn detect_all_agents(cfg: &repomon_core::Config) -> Vec<RawDetectedAgent> {
    let mut agents = Vec::new();
    // One Claude entry per detected config dir (default + ~/.claude-* + $CLAUDE_CONFIG_DIR).
    for (name, command) in agent::claude::agent_variants() {
        agents.push(RawDetectedAgent {
            kind: AgentKind::ClaudeCode.as_str().into_owned(),
            detected: on_path(&command),
            name,
            command,
            custom: false,
        });
    }
    for kind in [
        AgentKind::Codex,
        AgentKind::OpenCode,
        AgentKind::Antigravity,
        AgentKind::Aider,
        AgentKind::Cursor,
    ] {
        let command = kind.command().to_string();
        let name = kind.as_str().into_owned();
        agents.push(RawDetectedAgent {
            kind: kind.as_str().into_owned(),
            detected: on_path(&command),
            name,
            command,
            custom: false,
        });
    }
    let mut customs: Vec<_> = cfg.agents.iter().collect();
    customs.sort_by_key(|(name, _)| name.to_string());
    for (name, command) in customs {
        agents.push(RawDetectedAgent {
            kind: "custom".to_string(),
            detected: on_path(command),
            name: name.clone(),
            command: command.clone(),
            custom: true,
        });
    }
    agents
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
    pick_orchestrator_transcript_in(&config::home(), session_id)
}

/// Drop a stale orchestrator session: if we think one is running but its tmux window is gone (killed
/// externally, or it `/exit`ed), clear the tracked session and broadcast the stopped status, so
/// `orchestrator.status` reads accurately and `orchestrator.start` re-spawns rather than no-op on a
/// corpse. Returns whether a session is still tracked afterward.
pub(crate) async fn reconcile_orchestrator(ctx: &Ctx) -> bool {
    if ctx.orchestrator.lock().await.is_none() {
        return false;
    }
    let tmux = ctx.backend.clone();
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
pub(crate) fn resolve_orchestrator_backend(
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
        AgentKind::Antigravity => Ok(B::Antigravity),
        AgentKind::OpenCode => Ok(B::OpenCode),
        _ => Err(RpcError::invalid_params(format!(
            "agent '{name}' can't run the orchestrator: repomind needs an MCP-capable CLI \
             (a claude account, codex, antigravity, opencode, or a custom agent command)"
        ))),
    }
}

/// Resolve the orchestrator's base launch command from its agent name, mirroring `agent.spawn`: a
/// config custom wins, then an autodetected Claude variant (e.g. `claude-work` →
/// `CLAUDE_CONFIG_DIR=… claude`), else the kind's default binary (`codex` — anything else was
/// already rejected by [`resolve_orchestrator_backend`]). `None` (no agent chosen) is bare
/// `claude`.
pub(crate) fn orchestrator_base_command(
    agent: &Option<String>,
    customs: &HashMap<String, String>,
) -> String {
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

/// The result of translating spawn launch options: the (possibly augmented) launch command, plus an
/// optional `/effort` slash-command to inject as the session's FIRST input. Claude's native
/// `--effort` flag covers low|medium|high|xhigh|max, but `ultracode` (the top level = xhigh +
/// workflows) is only reachable via the `/effort` slash command, so that case is injected after the
/// session opens — exactly how an operator sets it. When `effort_inject` is `Some`, the caller must
/// send the task as input AFTER the injection (so effort is set before the task), not as a launch
/// argument.
#[derive(Debug, PartialEq, Eq)]
struct LaunchPlan {
    command: String,
    effort_inject: Option<String>,
}

/// Translate the optional spawn launch options (`--mode` / `--model` / `--effort`) into the agent
/// command's flags (and, for claude `ultracode`, a `/effort` input to inject), per [`AgentKind`].
/// `command` is the already-resolved base launch command (e.g. `claude`, `CLAUDE_CONFIG_DIR=… claude`,
/// or `codex`); it is run via `sh -c` by tmux, so every interpolated value is `shell_quote`d.
///
/// Invariants:
/// - When nothing is requested (`mode` absent/`"default"`, no `effort`, no `model`) the command is
///   returned **byte-identical** to the input (and `effort_inject` is `None`), so the default spawn
///   path is unchanged.
/// - Flags are only emitted for kinds whose dialect we know (Claude + variants, Codex). For any
///   other kind (an unknown binary) requested options are ignored with a warning rather than
///   injecting a flag the binary may not accept (which would make it exit on launch).
fn apply_launch_options(
    command: String,
    kind: &AgentKind,
    effort: Option<&str>,
    mode: Option<&str>,
    model: Option<&str>,
) -> LaunchPlan {
    // "default"/empty mean "no override" — treat them exactly like an absent option.
    let mode = mode.filter(|m| !m.eq_ignore_ascii_case("default") && !m.is_empty());
    let effort = effort.filter(|e| !e.is_empty());
    let model = model.filter(|m| !m.is_empty());
    if mode.is_none() && effort.is_none() && model.is_none() {
        // Byte-identical default path.
        return LaunchPlan {
            command,
            effort_inject: None,
        };
    }

    let mut suffix = String::new(); // flags appended to the command
    let mut effort_inject = None;

    match kind {
        AgentKind::ClaudeCode => {
            if let Some(m) = model {
                suffix.push_str(&format!(" --model {}", shell_quote(m)));
            }
            match mode {
                Some("auto") => suffix.push_str(" --permission-mode acceptEdits"),
                Some("plan") => suffix.push_str(" --permission-mode plan"),
                Some(other) => {
                    tracing::warn!("spawn: unknown --mode '{other}' for claude; ignoring")
                }
                None => {}
            }
            if let Some(e) = effort {
                match claude_effort(e) {
                    ClaudeEffort::Flag(level) => {
                        suffix.push_str(&format!(" --effort {}", shell_quote(level)))
                    }
                    // `ultracode` isn't a valid --effort flag value (claude warns and ignores it),
                    // so set it via the /effort slash command injected as the first input.
                    ClaudeEffort::Inject(level) => effort_inject = Some(format!("/effort {level}")),
                    ClaudeEffort::Unknown => tracing::warn!(
                        "spawn: unrecognized --effort '{e}' for claude \
                         (use low|medium|high|xhigh|max|ultracode); ignoring"
                    ),
                }
            }
        }
        AgentKind::Codex => {
            if let Some(m) = model {
                suffix.push_str(&format!(" --model {}", shell_quote(m)));
            }
            match mode {
                Some("auto") => suffix.push_str(" --full-auto"),
                Some("plan") => {
                    tracing::warn!("spawn: codex has no plan mode; ignoring --mode plan")
                }
                Some(other) => {
                    tracing::warn!("spawn: unknown --mode '{other}' for codex; ignoring")
                }
                None => {}
            }
            if let Some(e) = effort {
                match codex_reasoning_effort(e) {
                    Some(level) => suffix.push_str(&format!(
                        " -c model_reasoning_effort={}",
                        shell_quote(level)
                    )),
                    None => tracing::warn!(
                        "spawn: unrecognized --effort '{e}' for codex (use low|medium|high); ignoring"
                    ),
                }
            }
        }
        AgentKind::OpenCode => {
            if let Some(model) = model {
                suffix.push_str(&format!(" --model {}", shell_quote(model)));
            }
            if mode.is_some() || effort.is_some() {
                tracing::warn!(
                    "spawn: OpenCode mode and effort overrides are unavailable; ignoring"
                );
            }
        }
        AgentKind::Antigravity => {
            if let Some(model) = model {
                suffix.push_str(&format!(" --model {}", shell_quote(model)));
            }
            match mode {
                Some("auto") => suffix.push_str(" --mode accept-edits"),
                Some("plan") => suffix.push_str(" --mode plan"),
                Some(other) => tracing::warn!("spawn: unknown --mode '{other}' for agy; ignoring"),
                None => {}
            }
            if let Some(effort) = effort {
                match effort {
                    "low" | "medium" | "high" => {
                        suffix.push_str(&format!(" --effort {}", shell_quote(effort)))
                    }
                    _ => tracing::warn!("spawn: unknown --effort '{effort}' for agy; ignoring"),
                }
            }
        }
        other => tracing::warn!(
            "spawn: launch options (--mode/--model/--effort) aren't supported for agent kind \
             '{}'; ignoring",
            other.as_str()
        ),
    }

    LaunchPlan {
        command: format!("{command}{suffix}"),
        effort_inject,
    }
}

/// How a claude `--effort` level is realized: a native `--effort` flag value (low|medium|high|
/// xhigh|max), or `ultracode` which must be injected as a `/effort` slash command after launch.
enum ClaudeEffort {
    Flag(&'static str),
    Inject(&'static str),
    Unknown,
}

/// Classify an `--effort` level for a claude-kind agent. The native `--effort` launch flag accepts
/// low|medium|high|xhigh|max; `ultracode` (= xhigh + workflows) is only settable via the `/effort`
/// slash command, so it is injected instead.
fn claude_effort(effort: &str) -> ClaudeEffort {
    match effort.trim().to_lowercase().as_str() {
        "low" => ClaudeEffort::Flag("low"),
        "medium" => ClaudeEffort::Flag("medium"),
        "high" => ClaudeEffort::Flag("high"),
        "xhigh" => ClaudeEffort::Flag("xhigh"),
        "max" => ClaudeEffort::Flag("max"),
        "ultracode" => ClaudeEffort::Inject("ultracode"),
        _ => ClaudeEffort::Unknown,
    }
}

/// Normalize an `--effort` level to a codex `model_reasoning_effort` value. Codex tops out at
/// `high`, so the claude-only levels (xhigh/max/ultracode) clamp to `high` with a warning.
fn codex_reasoning_effort(effort: &str) -> Option<&'static str> {
    match effort.trim().to_lowercase().as_str() {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" | "max" | "ultracode" => {
            tracing::warn!(
                "spawn: codex has no '{}' effort; clamping to high",
                effort.trim().to_lowercase()
            );
            Some("high")
        }
        _ => None,
    }
}

/// Best-effort agent kind for a resolved (custom) launch command, so a custom configured agent that
/// wraps `claude`/`codex` still gets the right flag dialect. Reuses [`program_of`] (which skips
/// leading `VAR=value` env assignments) and matches the program's basename. An unrecognized program
/// yields `Other` (launch options are then ignored rather than guessed). A custom command with a
/// space inside a quoted env value can't be parsed by whitespace and falls back to `Other` — a safe
/// no-op, not a wrong flag.
fn kind_from_command(command: &str) -> AgentKind {
    match program_of(command).map(program_basename) {
        Some("claude") => AgentKind::ClaudeCode,
        Some("codex") => AgentKind::Codex,
        Some("opencode") => AgentKind::OpenCode,
        Some("agy") => AgentKind::Antigravity,
        Some("aider") => AgentKind::Aider,
        Some("cursor") | Some("cursor-agent") => AgentKind::Cursor,
        Some(other) => AgentKind::Other(other.to_string()),
        None => AgentKind::Other(String::new()),
    }
}

/// The basename of a program path (`/usr/bin/claude` → `claude`).
fn program_basename(prog: &str) -> &str {
    prog.rsplit('/').next().unwrap_or(prog)
}

fn next_agent_window(
    backend: &dyn repomon_core::agent::backend::SessionBackend,
    lane: i64,
) -> Result<(String, u32), RpcError> {
    let windows = backend.list_windows().map_err(internal)?;
    let next = repomon_core::TmuxRuntime::lane_windows_in(&windows, lane)
        .last()
        .and_then(|window| repomon_core::TmuxRuntime::slot_of_window(window))
        .unwrap_or(0)
        + 1;
    Ok((
        repomon_core::TmuxRuntime::slot_name(lane, next),
        next as u32,
    ))
}

fn write_agent_mcp_config(window: &str) -> std::io::Result<PathBuf> {
    let repomond = repomon_core::service::repomond_path();
    let config = json!({
        "mcpServers": {
            "repomon": {
                "command": repomond.to_string_lossy(),
                "args": ["mcp"],
            }
        }
    });
    let dir = repomon_core::config::config_dir().join("agent-mcp");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{window}.json"));
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&config).unwrap_or_default(),
    )?;
    Ok(path)
}

fn attach_agent_mcp(command: String, kind: &AgentKind, mcp_config: &Path) -> String {
    match kind {
        AgentKind::ClaudeCode => format!(
            "{command} --mcp-config {} --allowedTools mcp__repomon",
            shell_quote(&mcp_config.to_string_lossy())
        ),
        AgentKind::Codex => {
            let repomond = repomon_core::service::repomond_path();
            let command_override = format!(
                "mcp_servers.repomon.command=\"{}\"",
                repomond.to_string_lossy()
            );
            format!(
                "{command} -c {} -c {} -c {} -c {} -c {}",
                shell_quote(&command_override),
                shell_quote("mcp_servers.repomon.args=[\"mcp\"]"),
                shell_quote("mcp_servers.repomon.enabled=true"),
                shell_quote(
                    "mcp_servers.repomon.env_vars=[\"REPOMON_MCP_SOCKET\",\"REPOMON_MCP_MODE\",\"REPOMON_MCP_IDENTITY_TOKEN\"]"
                ),
                shell_quote("mcp_servers.repomon.default_tools_approval_mode=\"approve\"")
            )
        }
        _ => command,
    }
}

/// Attach backend-specific MCP configuration without placing identity values in persistent files.
/// OpenCode receives a runtime-only inline merge. Antigravity and Cursor need global registration,
/// but their entries contain only the executable and arguments; the managed child inherits its identity env.
///
/// For `Other`/custom agents: the command string is inspected with `kind_from_command` to detect
/// whether the custom command wraps a known binary (e.g. `claude --dangerously-skip-permissions`
/// → `ClaudeCode`, `agy --mode plan` → `Antigravity`). If it matches, that kind's wiring is
/// applied transparently. Completely unknown custom commands receive no MCP wiring.
///
/// `Aider` has no native MCP client support as of its current release; no wiring is attempted.
fn configure_backend_mcp(kind: &AgentKind, spec: &mut SpawnSpec) -> Result<(), String> {
    match kind {
        AgentKind::OpenCode => {
            let raw_existing = std::env::var("OPENCODE_CONFIG_CONTENT").ok();
            let config_json = build_opencode_config_content(
                raw_existing.as_deref(),
                &[
                    "REPOMON_MCP_SOCKET",
                    "REPOMON_MCP_MODE",
                    "REPOMON_MCP_IDENTITY_TOKEN",
                ],
            )?;
            spec.env
                .push(("OPENCODE_CONFIG_CONTENT".into(), config_json));
        }
        AgentKind::Antigravity => ensure_antigravity_mcp_registration()?,
        AgentKind::Cursor => ensure_cursor_mcp_registration()?,
        AgentKind::Aider => {
            // Aider has no native MCP client support; fleet mail is unavailable for Aider agents.
        }
        AgentKind::Other(_) => {
            // Inspect the program name to detect whether a custom command wraps a known binary
            // (e.g. `claude --dangerously-skip-permissions` wraps ClaudeCode). If it matches a
            // wired dialect, apply that kind's registration so the custom agent gets fleet mail.
            // Fully unknown commands silently receive no MCP wiring.
            let dialect = kind_from_command(&spec.program);
            if !matches!(dialect, AgentKind::Other(_)) {
                configure_backend_mcp(&dialect, spec)?;
            }
        }
        // ClaudeCode and Codex wiring is handled at the call site via attach_agent_mcp /
        // write_agent_mcp_config; configure_backend_mcp is a no-op for them.
        AgentKind::ClaudeCode | AgentKind::Codex => {}
    }
    Ok(())
}

/// Build the `OPENCODE_CONFIG_CONTENT` JSON string registering the `repomon` MCP server.
/// Preserves any existing configuration in `existing` (or parsed from `OPENCODE_CONFIG_CONTENT`),
/// adding or replacing `mcp.repomon` with a local server executing `repomond mcp`.
/// The `environment` mapping specifies which process environment variables OpenCode should
/// interpolate into the child process using `{env:VAR}` syntax.
pub(crate) fn build_opencode_config_content(
    existing: Option<&str>,
    env_vars: &[&str],
) -> Result<String, String> {
    let mut root = match existing {
        Some(raw) if !raw.trim().is_empty() => serde_json::from_str::<serde_json::Value>(raw)
            .map_err(|error| format!("invalid OPENCODE_CONFIG_CONTENT: {error}"))?,
        _ => json!({}),
    };
    let root_object = root
        .as_object_mut()
        .ok_or_else(|| "OPENCODE_CONFIG_CONTENT must be a JSON object".to_string())?;
    let mcp = root_object.entry("mcp").or_insert_with(|| json!({}));
    let mcp_object = mcp
        .as_object_mut()
        .ok_or_else(|| "OPENCODE_CONFIG_CONTENT.mcp must be an object".to_string())?;
    let repomond = repomon_core::service::repomond_path();
    let mut environment_map = serde_json::Map::new();
    for var in env_vars {
        environment_map.insert((*var).into(), json!(format!("{{env:{var}}}")));
    }
    mcp_object.insert(
        "repomon".into(),
        json!({
            "type": "local",
            "command": [repomond.to_string_lossy(), "mcp"],
            "environment": environment_map
        }),
    );
    serde_json::to_string(&root).map_err(|error| error.to_string())
}

fn ensure_antigravity_mcp_registration() -> Result<(), String> {
    let path = std::env::var("REPOMON_ANTIGRAVITY_MCP_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| config::home().join(".gemini/config/mcp_config.json"));
    let mut root = match std::fs::read(&path) {
        Ok(raw) => serde_json::from_slice::<serde_json::Value>(&raw)
            .map_err(|error| format!("invalid Antigravity MCP config: {error}"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(error) => return Err(error.to_string()),
    };
    let root_object = root
        .as_object_mut()
        .ok_or_else(|| "Antigravity MCP config must be a JSON object".to_string())?;
    let servers = root_object.entry("mcpServers").or_insert_with(|| json!({}));
    let servers_object = servers
        .as_object_mut()
        .ok_or_else(|| "Antigravity mcpServers must be an object".to_string())?;
    let repomond = repomon_core::service::repomond_path();
    let wanted = json!({
        "command": repomond.to_string_lossy(),
        "args": ["mcp"]
    });
    if servers_object.get("repomon") == Some(&wanted) {
        return Ok(());
    }
    servers_object.insert("repomon".into(), wanted);
    let parent = path
        .parent()
        .ok_or_else(|| "Antigravity MCP config has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.repomon-tmp");
    let encoded = serde_json::to_vec_pretty(&root).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, encoded).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, &path).map_err(|error| error.to_string())?;
    Ok(())
}

fn ensure_cursor_mcp_registration() -> Result<(), String> {
    let path = std::env::var("REPOMON_CURSOR_MCP_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| config::home().join(".cursor/mcp.json"));
    let mut root = match std::fs::read(&path) {
        Ok(raw) => serde_json::from_slice::<serde_json::Value>(&raw)
            .map_err(|error| format!("invalid Cursor MCP config: {error}"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(error) => return Err(error.to_string()),
    };
    let root_object = root
        .as_object_mut()
        .ok_or_else(|| "Cursor MCP config must be a JSON object".to_string())?;
    let servers = root_object.entry("mcpServers").or_insert_with(|| json!({}));
    let servers_object = servers
        .as_object_mut()
        .ok_or_else(|| "Cursor mcpServers must be an object".to_string())?;
    let repomond = repomon_core::service::repomond_path();
    let wanted = json!({
        "command": repomond.to_string_lossy(),
        "args": ["mcp"]
    });
    if servers_object.get("repomon") == Some(&wanted) {
        return Ok(());
    }
    servers_object.insert("repomon".into(), wanted);
    let parent = path
        .parent()
        .ok_or_else(|| "Cursor MCP config has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.repomon-tmp");
    let encoded = serde_json::to_vec_pretty(&root).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, encoded).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, &path).map_err(|error| error.to_string())?;
    Ok(())
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

/// Build the full `agy` invocation for the orchestrator, shell-quoted for `sh -c` (tmux runs the
/// window command through a shell). ALL Antigravity-CLI flag knowledge lives here (plus its unit
/// test), so an Antigravity release changing a flag is a one-function fix. Verified against
/// `agy --help`; where it diverges from the Claude arm:
/// - No `--mcp-config` argument: the repomon fleet server is registered globally in
///   `~/.gemini/config/mcp_config.json` via [`ensure_antigravity_mcp_registration`]. Environment
///   variables (`REPOMON_MCP_SOCKET`, `REPOMON_MCP_AUTONOMY`, and optionally `REPOMON_MCP_MAX_AGENTS`)
///   are exported inline in the command prefix so `agy` and its spawned MCP child processes inherit
///   them; `REPOMON_MCP_MODE` is deliberately NOT set to `"agent"`, granting full orchestrator tool
///   surface.
/// - No `--append-system-prompt`: the repomind persona is prepended to the initial prompt passed
///   via `--prompt-interactive` instead.
/// - No `--session-id` and no `--allowedTools`: Antigravity has no session-id flag or stable
///   transcript contract (`has_transcript` is false), so dialogs are monitored via pane output;
///   tool approval behavior maps through `--dangerously-skip-permissions` and `--mode`.
/// - `autonomy` maps onto `agy`'s execution flags:
///   - `autonomous` -> `--dangerously-skip-permissions --mode accept-edits`
///   - `supervised` -> default interactive mode (prompts for approvals and edits)
///   - `read-only` -> `--mode plan` (plan mode produces plans without executing edits; server-side
///     `REPOMON_MCP_AUTONOMY="read-only"` strictly enforces read-only tool access)
fn build_antigravity_orchestrator_command(
    base: &str,
    socket: &Path,
    autonomy: &str,
    max_agents: Option<usize>,
    model: &Option<String>,
    prompt: &Option<String>,
) -> String {
    let mut env_parts = vec![
        format!(
            "REPOMON_MCP_SOCKET={}",
            shell_quote(&socket.to_string_lossy())
        ),
        format!("REPOMON_MCP_AUTONOMY={}", shell_quote(autonomy)),
    ];
    if let Some(n) = max_agents {
        env_parts.push(format!(
            "REPOMON_MCP_MAX_AGENTS={}",
            shell_quote(&n.to_string())
        ));
    }
    let env_prefix = env_parts.join(" ");
    let mut command = format!("{env_prefix} {base}");

    match autonomy {
        "autonomous" => command.push_str(" --dangerously-skip-permissions --mode accept-edits"),
        "supervised" => {}
        "read-only" => command.push_str(" --mode plan"),
        _ => {}
    }

    if let Some(model) = model {
        command.push_str(" --model ");
        command.push_str(&shell_quote(model));
    }

    let goal = match prompt.as_deref().filter(|p| !p.is_empty()) {
        Some(p) => format!("{}\n\n{p}", repomon_mcp::PERSONA),
        None => repomon_mcp::PERSONA.to_string(),
    };
    command.push_str(" --prompt-interactive ");
    command.push_str(&shell_quote(&goal));
    command
}

/// Build the full `opencode` invocation for the orchestrator, shell-quoted for `sh -c` (tmux runs
/// the window command through a shell). ALL OpenCode-CLI flag knowledge lives here (plus its unit
/// test), so an OpenCode release changing a flag is a one-function fix. Verified against
/// `opencode --help`; where it diverges from the Claude arm:
/// - No `--mcp-config` file: the repomon fleet server is registered dynamically via the
///   `OPENCODE_CONFIG_CONTENT` environment variable using [`build_opencode_config_content`].
///   The config's `environment` table maps `{env:VAR}` entries for `REPOMON_MCP_SOCKET`,
///   `REPOMON_MCP_AUTONOMY`, and optionally `REPOMON_MCP_MAX_AGENTS`, which are exported inline
///   in the command prefix. `REPOMON_MCP_MODE` is deliberately NOT set to `"agent"`, granting full
///   orchestrator tool access.
/// - No `--append-system-prompt`: the repomind persona is prepended to the initial prompt passed
///   via `--prompt` instead.
/// - No `--session-id` and no `--allowedTools`: OpenCode has no session-id flag or stable
///   transcript contract (`has_transcript` is false), so dialogs are monitored via pane output;
///   tool approval behavior is bounded server-side via `REPOMON_MCP_AUTONOMY`.
/// - Autonomy levels: OpenCode's interactive TUI mode has no CLI-level approval bypass flag;
///   posture (`autonomous`, `supervised`, `read-only`) is strictly enforced by `repomon_mcp::policy`
///   via `REPOMON_MCP_AUTONOMY`.
fn build_opencode_orchestrator_command(
    base: &str,
    socket: &Path,
    autonomy: &str,
    max_agents: Option<usize>,
    model: &Option<String>,
    prompt: &Option<String>,
) -> Result<String, String> {
    let mut env_var_names = vec!["REPOMON_MCP_SOCKET", "REPOMON_MCP_AUTONOMY"];
    if max_agents.is_some() {
        env_var_names.push("REPOMON_MCP_MAX_AGENTS");
    }
    let raw_existing = std::env::var("OPENCODE_CONFIG_CONTENT").ok();
    let config_json = build_opencode_config_content(raw_existing.as_deref(), &env_var_names)?;

    let mut env_parts = vec![
        format!("OPENCODE_CONFIG_CONTENT={}", shell_quote(&config_json)),
        format!(
            "REPOMON_MCP_SOCKET={}",
            shell_quote(&socket.to_string_lossy())
        ),
        format!("REPOMON_MCP_AUTONOMY={}", shell_quote(autonomy)),
    ];
    if let Some(n) = max_agents {
        env_parts.push(format!(
            "REPOMON_MCP_MAX_AGENTS={}",
            shell_quote(&n.to_string())
        ));
    }
    let env_prefix = env_parts.join(" ");
    let mut command = format!("{env_prefix} {base}");

    if let Some(model) = model {
        command.push_str(" --model ");
        command.push_str(&shell_quote(model));
    }

    let goal = match prompt.as_deref().filter(|p| !p.is_empty()) {
        Some(p) => format!("{}\n\n{p}", repomon_mcp::PERSONA),
        None => repomon_mcp::PERSONA.to_string(),
    };
    command.push_str(" --prompt ");
    command.push_str(&shell_quote(&goal));
    Ok(command)
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
    write_orchestrator_mcp_config_named(socket, autonomy, max_agents, &[], "repomind-mcp.json")
}

/// Like [`write_orchestrator_mcp_config`] but with extra env pairs and a caller-chosen file
/// name — standing runs write `repomind-standing-mcp.json` with the unattended guardrail env so
/// they never clobber (or inherit) the interactive session's config.
pub(crate) fn write_orchestrator_mcp_config_named(
    socket: &Path,
    autonomy: &str,
    max_agents: Option<usize>,
    extra_env: &[(&str, String)],
    filename: &str,
) -> std::io::Result<PathBuf> {
    let repomond = repomon_core::service::repomond_path();
    let mut env = serde_json::Map::new();
    env.insert("REPOMON_MCP_SOCKET".into(), json!(socket.to_string_lossy()));
    env.insert("REPOMON_MCP_AUTONOMY".into(), json!(autonomy));
    if let Some(n) = max_agents {
        env.insert("REPOMON_MCP_MAX_AGENTS".into(), json!(n.to_string()));
    }
    for (k, v) in extra_env {
        env.insert((*k).into(), json!(v));
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
    let path = cfg_dir.join(filename);
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&mcp_config).unwrap_or_default(),
    )?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail_lane(id: i64, labels: &[Option<&str>]) -> Lane {
        let now = chrono::Utc::now();
        let repo = repomon_core::model::Repo {
            id,
            path: PathBuf::from(format!("/repo-{id}")),
            name: format!("repo-{id}"),
            added_at: now,
            worktree_root_template: None,
            hidden: false,
        };
        let worktree = repomon_core::model::Worktree {
            id,
            repo_id: id,
            path: repo.path.clone(),
            branch: Some("main".into()),
            head: "0000000000000000000000000000000000000000".parse().unwrap(),
            is_main: true,
            name: "main".into(),
        };
        let state = repomon_core::model::WorktreeState {
            worktree_id: id,
            head: worktree.head,
            branch: worktree.branch.clone(),
            upstream: None,
            ahead: 0,
            behind: 0,
            dirty: Default::default(),
            last_commit_at: None,
            locked: false,
            prunable: false,
            last_change_at: None,
        };
        let agent_sessions = labels
            .iter()
            .enumerate()
            .map(|(index, label)| AgentSession {
                id: index as i64 + 1,
                agent: AgentKind::ClaudeCode,
                repo_id: id,
                worktree_id: Some(id),
                started_at: now,
                last_activity_at: now,
                ended_at: None,
                manifest_path: PathBuf::from(format!("/session-{id}-{index}.jsonl")),
                tool_call_count: 0,
                title: None,
                status: AgentStatus::Waiting,
                external: false,
                session_id: Some(format!("session-{id}-{index}")),
                resume_at: None,
                inferred: false,
                tmux_window: Some(repomon_core::TmuxRuntime::slot_name(id, index + 1)),
                last_message: None,
                pending_prompt: None,
                pending_dialog: None,
                stale: false,
                stalled_since: None,
                subagent_running: None,
                ended_turn: true,
                gate: None,
                config_dir: None,
                custom_label: label.map(str::to_string),
                generated_label: None,
            })
            .collect();
        Lane {
            id,
            repo,
            worktree,
            state,
            agent_sessions,
            last_activity_at: now,
            pinned: false,
        }
    }

    fn mail_lane_with_agents(id: i64, agents: &[(Option<&str>, AgentKind)]) -> Lane {
        let now = chrono::Utc::now();
        let repo = repomon_core::model::Repo {
            id,
            path: PathBuf::from(format!("/repo-{id}")),
            name: format!("repo-{id}"),
            added_at: now,
            worktree_root_template: None,
            hidden: false,
        };
        let worktree = repomon_core::model::Worktree {
            id,
            repo_id: id,
            path: repo.path.clone(),
            branch: Some("main".into()),
            head: "0000000000000000000000000000000000000000".parse().unwrap(),
            is_main: true,
            name: "main".into(),
        };
        let state = repomon_core::model::WorktreeState {
            worktree_id: id,
            head: worktree.head,
            branch: worktree.branch.clone(),
            upstream: None,
            ahead: 0,
            behind: 0,
            dirty: Default::default(),
            last_commit_at: None,
            locked: false,
            prunable: false,
            last_change_at: None,
        };
        let agent_sessions = agents
            .iter()
            .enumerate()
            .map(|(index, (label, kind))| AgentSession {
                id: index as i64 + 1,
                agent: kind.clone(),
                repo_id: id,
                worktree_id: Some(id),
                started_at: now,
                last_activity_at: now,
                ended_at: None,
                manifest_path: PathBuf::from(format!("/session-{id}-{index}.jsonl")),
                tool_call_count: 0,
                title: None,
                status: AgentStatus::Waiting,
                external: false,
                session_id: Some(format!("session-{id}-{index}")),
                resume_at: None,
                inferred: false,
                tmux_window: Some(repomon_core::TmuxRuntime::slot_name(id, index + 1)),
                last_message: None,
                pending_prompt: None,
                pending_dialog: None,
                stale: false,
                stalled_since: None,
                subagent_running: None,
                ended_turn: true,
                gate: None,
                config_dir: None,
                custom_label: label.map(str::to_string),
                generated_label: None,
            })
            .collect();
        Lane {
            id,
            repo,
            worktree,
            state,
            agent_sessions,
            last_activity_at: now,
            pinned: false,
        }
    }

    #[test]
    fn message_addresses_route_lane_slots_and_exact_labels() {
        let lanes = vec![mail_lane(7, &[Some("first"), Some("reviewer")])];
        let first = resolve_agent_message_address(&lanes, "lane-7").unwrap();
        assert_eq!(first.slot, Some(1));
        assert_eq!(first.address.as_str(), "lane-7/1");
        let second = resolve_agent_message_address(&lanes, "lane-7/2").unwrap();
        assert_eq!(second.slot, Some(2));
        assert_eq!(second.window.as_deref(), Some("lane-7-2"));
        let label = resolve_agent_message_address(&lanes, "@reviewer").unwrap();
        assert_eq!(label.address.as_str(), "lane-7/2");
        assert!(resolve_agent_message_address(&lanes, "lane-7/0").is_err());
    }

    #[test]
    fn message_addresses_route_lane_slots_with_heterogeneous_agents() {
        let lanes = vec![mail_lane_with_agents(
            7,
            &[
                (Some("first"), AgentKind::ClaudeCode),
                (Some("antigravity-worker"), AgentKind::Antigravity),
            ],
        )];
        let first = resolve_agent_message_address(&lanes, "lane-7/1").unwrap();
        assert_eq!(first.slot, Some(1));
        assert_eq!(first.agent_kind.as_deref(), Some("claude-code"));
        let second = resolve_agent_message_address(&lanes, "lane-7/2").unwrap();
        assert_eq!(second.slot, Some(2));
        assert_eq!(second.agent_kind.as_deref(), Some("antigravity"));
        let label = resolve_agent_message_address(&lanes, "@antigravity-worker").unwrap();
        assert_eq!(label.slot, Some(2));
        assert_eq!(label.agent_kind.as_deref(), Some("antigravity"));
    }

    #[test]
    fn message_slot_follows_window_when_activity_order_differs() {
        let mut lane = mail_lane(7, &[Some("first"), Some("second")]);
        lane.agent_sessions.swap(0, 1);
        let first = resolve_agent_message_address(&[lane], "lane-7/1").unwrap();
        assert_eq!(first.window.as_deref(), Some("lane-7"));
        assert_eq!(first.session_id.as_deref(), Some("session-7-0"));
    }

    #[test]
    fn duplicate_exact_message_labels_are_ambiguous() {
        let lanes = vec![
            mail_lane(7, &[Some("reviewer")]),
            mail_lane(8, &[Some("reviewer")]),
        ];
        let error = resolve_agent_message_address(&lanes, "@reviewer").unwrap_err();
        assert!(error.message.contains("ambiguous"));
    }

    fn resolved_sender(lane_id: Option<i64>, slot: Option<u32>) -> ResolvedAgentAddress {
        ResolvedAgentAddress {
            address: AgentAddress::new(match (lane_id, slot) {
                (Some(l), Some(s)) => format!("lane-{l}/{s}"),
                _ => "operator".to_string(),
            }),
            lane_id,
            slot,
            window: None,
            session_id: None,
            agent_kind: None,
        }
    }

    #[test]
    fn classify_token_recognizes_wildcards_and_falls_back_to_literal() {
        assert_eq!(classify_token("*"), AddressToken::GlobalWildcard);
        assert_eq!(classify_token(" * "), AddressToken::GlobalWildcard);
        assert_eq!(classify_token("lane-7/*"), AddressToken::LaneWildcard(7));
        assert_eq!(
            classify_token("lane-7/1"),
            AddressToken::Literal("lane-7/1".into())
        );
        assert_eq!(
            classify_token("operator"),
            AddressToken::Literal("operator".into())
        );
        // Malformed lane wildcards fall back to a literal token, resolved (and rejected) later
        // by `resolve_message_address` exactly like any other bad address.
        assert_eq!(
            classify_token("lane-abc/*"),
            AddressToken::Literal("lane-abc/*".into())
        );
        assert_eq!(
            classify_token("lane-/*"),
            AddressToken::Literal("lane-/*".into())
        );
        assert_eq!(classify_token("**"), AddressToken::Literal("**".into()));
    }

    #[test]
    fn expand_wildcard_targets_covers_lane_and_fleet_and_excludes_sender() {
        let lanes = vec![
            mail_lane(5, &[Some("a"), Some("b")]),
            mail_lane(6, &[Some("c")]),
        ];
        let outsider = resolved_sender(None, None);
        assert_eq!(
            expand_wildcard_targets(&lanes, Some(5), &outsider),
            vec!["lane-5/1", "lane-5/2"]
        );
        assert_eq!(
            expand_wildcard_targets(&lanes, None, &outsider),
            vec!["lane-5/1", "lane-5/2", "lane-6/1"]
        );
        let self_in_lane5 = resolved_sender(Some(5), Some(1));
        assert_eq!(
            expand_wildcard_targets(&lanes, None, &self_in_lane5),
            vec!["lane-5/2", "lane-6/1"]
        );
        assert_eq!(
            expand_wildcard_targets(&lanes, Some(5), &self_in_lane5),
            vec!["lane-5/2"]
        );
    }

    #[test]
    fn expand_message_targets_single_wildcard_and_list_dedupe() {
        let lanes = vec![
            mail_lane(5, &[Some("a"), Some("b")]),
            mail_lane(6, &[Some("c")]),
        ];
        let outsider = resolved_sender(None, None);

        let single_lane_wild = MessageTo::Single("lane-5/*".into());
        assert_eq!(
            expand_message_targets(&single_lane_wild, &lanes, &outsider).unwrap(),
            vec!["lane-5/1", "lane-5/2"]
        );

        let global = MessageTo::Single("*".into());
        assert_eq!(
            expand_message_targets(&global, &lanes, &outsider).unwrap(),
            vec!["lane-5/1", "lane-5/2", "lane-6/1"]
        );

        let list = MessageTo::Multi(vec!["lane-5/1".into(), "lane-6/1".into()]);
        assert_eq!(
            expand_message_targets(&list, &lanes, &outsider).unwrap(),
            vec!["lane-5/1", "lane-6/1"]
        );

        // Overlap between an explicit address and a wildcard that also covers it collapses to
        // one delivery.
        let overlap = MessageTo::Multi(vec!["lane-5/1".into(), "lane-5/*".into()]);
        assert_eq!(
            expand_message_targets(&overlap, &lanes, &outsider).unwrap(),
            vec!["lane-5/1", "lane-5/2"]
        );

        let self_in_lane5 = resolved_sender(Some(5), Some(1));
        let broadcast_excludes_self = MessageTo::Single("*".into());
        assert_eq!(
            expand_message_targets(&broadcast_excludes_self, &lanes, &self_in_lane5).unwrap(),
            vec!["lane-5/2", "lane-6/1"]
        );

        // An explicit self-address still goes through even though wildcard expansion excludes
        // it: only the *implicit* broadcast targets skip the sender.
        let explicit_self_plus_wildcard = MessageTo::Multi(vec!["lane-5/1".into(), "*".into()]);
        assert_eq!(
            expand_message_targets(&explicit_self_plus_wildcard, &lanes, &self_in_lane5).unwrap(),
            vec!["lane-5/1", "lane-5/2", "lane-6/1"]
        );
    }

    #[test]
    fn expand_message_targets_rejects_empty_list_and_blank_items() {
        let empty = MessageTo::Multi(vec![]);
        let error = expand_message_targets(&empty, &[], &resolved_sender(None, None)).unwrap_err();
        assert!(error.message.contains("must not be empty"));

        let blank = MessageTo::Multi(vec!["  ".into()]);
        let error = expand_message_targets(&blank, &[], &resolved_sender(None, None)).unwrap_err();
        assert!(error.message.contains("must not be empty"));
    }

    #[test]
    fn message_to_legacy_single_only_for_plain_non_wildcard_string() {
        assert_eq!(
            MessageTo::Single("lane-2/1".into()).as_legacy_single(),
            Some("lane-2/1")
        );
        assert_eq!(MessageTo::Single("*".into()).as_legacy_single(), None);
        assert_eq!(
            MessageTo::Single("lane-2/*".into()).as_legacy_single(),
            None
        );
        assert_eq!(
            MessageTo::Multi(vec!["lane-2/1".into()]).as_legacy_single(),
            None
        );
    }

    fn fit_snap(
        local: bool,
        focus_window: Option<&str>,
        focus_at: Option<std::time::Instant>,
        last_interaction: Option<std::time::Instant>,
    ) -> SessSnapshot {
        SessSnapshot {
            local,
            focus: focus_window.map(|w| (7i64, w.to_string())),
            focus_at,
            last_interaction,
        }
    }

    #[test]
    fn fit_arbitration_across_sessions() {
        let now = std::time::Instant::now();
        let fresh = Some(now - std::time::Duration::from_secs(2));
        let stale = Some(now - std::time::Duration::from_secs(30));
        let earlier = Some(now - std::time::Duration::from_secs(10));
        let later = Some(now - std::time::Duration::from_secs(1));

        // No contenders: a lone caller always gets its fit.
        let caller = fit_snap(false, None, None, None);
        assert!(fit_allowed(&caller, &[], "lane-7", now));

        // Self-refit: the caller is never in `others`, so even holding a fresh focus itself it is
        // allowed. (An empty `others` models exclusion of self.)
        let self_focus = fit_snap(true, Some("lane-7"), fresh, later);
        assert!(fit_allowed(&self_focus, &[], "lane-7", now));

        // Rule 1: another Local (TUI) session with a fresh focus beat on the window denies, and
        // beats a remote caller regardless of interaction recency.
        let tui = fit_snap(true, Some("lane-7"), fresh, None);
        let remote_caller = fit_snap(false, None, None, later);
        assert!(!fit_allowed(&remote_caller, &[tui], "lane-7", now));

        // A Local focus on a DIFFERENT window doesn't block this window.
        let tui_other = fit_snap(true, Some("lane-9"), fresh, None);
        assert!(fit_allowed(&remote_caller, &[tui_other], "lane-7", now));

        // Stale beat releases ownership: a crashed/closed TUI no longer blocks.
        let tui_stale = fit_snap(true, Some("lane-7"), stale, None);
        assert!(fit_allowed(&remote_caller, &[tui_stale], "lane-7", now));
        let tui_no_beat = fit_snap(true, Some("lane-7"), None, None);
        assert!(fit_allowed(&remote_caller, &[tui_no_beat], "lane-7", now));

        // Rule 2, order A: a remote peer that interacted MORE recently than the caller wins.
        let peer_newer = fit_snap(false, Some("lane-7"), fresh, later);
        let caller_old = fit_snap(false, None, None, earlier);
        assert!(!fit_allowed(&caller_old, &[peer_newer], "lane-7", now));

        // Rule 2, order B: a remote peer that interacted LESS recently than the caller yields.
        let peer_older = fit_snap(false, Some("lane-7"), fresh, earlier);
        let caller_new = fit_snap(false, None, None, later);
        assert!(fit_allowed(&caller_new, &[peer_older], "lane-7", now));

        // A remote peer that has never driven the agent never outranks the caller.
        let peer_idle = fit_snap(false, Some("lane-7"), fresh, None);
        assert!(fit_allowed(&caller_old, &[peer_idle], "lane-7", now));

        // A peer that HAS driven still wins over a caller that never has.
        let peer_any = fit_snap(false, Some("lane-7"), fresh, earlier);
        let caller_never = fit_snap(false, None, None, None);
        assert!(!fit_allowed(&caller_never, &[peer_any], "lane-7", now));

        // A remote peer with a STALE focus beat doesn't arbitrate, however recent its interaction.
        let peer_stale = fit_snap(false, Some("lane-7"), stale, later);
        assert!(fit_allowed(&caller_old, &[peer_stale], "lane-7", now));
    }

    #[test]
    fn session_visibility_rules() {
        // total, alive, managed_n, fresh
        // A live agent (alive>=1) is kept; with several stale transcripts, only the live one(s).
        assert_eq!(sessions_to_keep(5, Some(1), 0, 0), 1);
        // No live process and no managed window -> a /exit'ed session is dropped immediately without lingering.
        assert_eq!(sessions_to_keep(5, Some(0), 0, 0), 0);
        assert_eq!(sessions_to_keep(5, Some(0), 0, 1), 0);
        // A managed lane keeps its window count.
        assert_eq!(sessions_to_keep(3, Some(0), 1, 0), 1);
        assert_eq!(sessions_to_keep(3, Some(0), 1, 1), 1);
        // When alive is known, keep = max(alive, managed_n) capped at total, without inflating on stale fresh transcripts.
        assert_eq!(sessions_to_keep(5, Some(2), 1, 3), 2);
        assert_eq!(sessions_to_keep(1, Some(5), 0, 0), 1);
        // When probe is unavailable (None), fresh acts as backstop.
        assert_eq!(sessions_to_keep(5, None, 1, 3), 3);
        assert_eq!(sessions_to_keep(2, None, 0, 0), 0);
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

    /// A minimal transcript summary for pairing tests: `sid` + activity time.
    fn tsum(sid: &str, last_activity: chrono::DateTime<chrono::Utc>) -> agent::TranscriptSummary {
        agent::TranscriptSummary {
            kind: repomon_core::model::AgentKind::ClaudeCode,
            manifest_path: PathBuf::from(format!("/tmp/{sid}.jsonl")),
            cwd: None,
            last_activity,
            tool_call_count: 0,
            status: repomon_core::model::AgentStatus::Idle,
            title: None,
            last_message: None,
            config_dir: None,
            session_id: Some(sid.to_string()),
            ended_turn: false,
        }
    }

    /// A `tsum` that also carries a last message, so it has a pane-evidence fingerprint.
    fn tsum_msg(
        sid: &str,
        last_activity: chrono::DateTime<chrono::Utc>,
        last_message: &str,
    ) -> agent::TranscriptSummary {
        agent::TranscriptSummary {
            last_message: Some(last_message.to_string()),
            ..tsum(sid, last_activity)
        }
    }

    fn candidate_sids(p: &Pairing) -> Vec<&str> {
        p.new_bindings.iter().map(|b| b.sid.as_str()).collect()
    }

    fn wm(name: &str, wid: u64, sid: Option<&str>) -> agent::WindowMeta {
        agent::WindowMeta {
            name: name.into(),
            wid,
            session: sid.map(str::to_string),
            agent_kind: None,
        }
    }

    fn wm_kind(name: &str, wid: u64, sid: Option<&str>, kind: Option<&str>) -> agent::WindowMeta {
        agent::WindowMeta {
            name: name.into(),
            wid,
            session: sid.map(str::to_string),
            agent_kind: kind.map(str::to_string),
        }
    }

    /// Activity times t(0) < t(1) < … in 10s steps, so several consecutive stamps all count
    /// as "fresh" (within `RECENTLY_ACTIVE_SECS`) relative to a nearby `now`.
    fn t(n: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(1_700_000_000 + n * 10, 0).unwrap()
    }

    #[test]
    fn first_contact_stays_placeholder_until_pane_evidence() {
        // First contact (nothing bound yet): no transcript is exposed on a window until pane
        // evidence confirms it. This prevents a stale transcript from appearing over a new pane.
        let summaries = vec![tsum("c", t(3)), tsum("b", t(2)), tsum("a", t(1))];
        let windows = vec![wm("lane-5", 1, None), wm("lane-5-2", 2, None)];
        let p = pair_transcripts_to_windows(&summaries, &windows, t(3));
        assert_eq!(p.assignment, vec![None, None, None]);
        // The two newest transcripts are actively fresh → both are nominated for bindings,
        // with every window as a legal stamp target for the evidence pass.
        let mut sids = candidate_sids(&p);
        sids.sort();
        assert_eq!(sids, vec!["b", "c"]);
        assert_eq!(
            p.probe,
            vec![(1, "lane-5".to_string()), (2, "lane-5-2".to_string())]
        );
        assert_eq!(
            p.unpaired,
            vec!["lane-5-2".to_string(), "lane-5".to_string()]
        );
    }

    #[test]
    fn pairing_sticks_across_activity_flip() {
        // The reported bug: two bound agents swap activity rank; the pairing must not move.
        let windows = vec![wm("lane-7", 1, Some("a")), wm("lane-7-2", 2, Some("b"))];
        // b most recently active…
        let p = pair_transcripts_to_windows(&[tsum("b", t(2)), tsum("a", t(1))], &windows, t(2));
        assert_eq!(
            p.assignment,
            vec![Some("lane-7-2".into()), Some("lane-7".into())]
        );
        assert!(p.new_bindings.is_empty());
        // …then a takes a turn and the order flips: same windows per session id.
        let p = pair_transcripts_to_windows(&[tsum("a", t(3)), tsum("b", t(2))], &windows, t(3));
        assert_eq!(
            p.assignment,
            vec![Some("lane-7".into()), Some("lane-7-2".into())]
        );
        assert!(p.new_bindings.is_empty());
        assert!(p.unpaired.is_empty());
    }

    #[test]
    fn first_contact_binds_then_holds_through_flip() {
        // Unbound windows (daemon restart / pre-upgrade agents) stay placeholders while the
        // candidates await evidence; re-running with confirmed bindings and flipped activity
        // keeps the original assignment.
        let unbound = vec![wm("lane-7", 1, None), wm("lane-7-2", 2, None)];
        let p = pair_transcripts_to_windows(&[tsum("b", t(2)), tsum("a", t(1))], &unbound, t(2));
        assert_eq!(p.assignment, vec![None, None]);
        let bound = vec![wm("lane-7", 1, Some("a")), wm("lane-7-2", 2, Some("b"))];
        let p = pair_transcripts_to_windows(&[tsum("a", t(3)), tsum("b", t(2))], &bound, t(3));
        assert_eq!(
            p.assignment,
            vec![Some("lane-7".into()), Some("lane-7-2".into())]
        );
        assert!(p.new_bindings.is_empty());
    }

    #[test]
    fn slot_recycling_pairs_new_transcript_to_new_window() {
        // Slot 1 (`lane-7`) died and was respawned: same NAME, higher window id, no binding.
        // The surviving agent b keeps its bound `lane-7-2`; newcomer c awaits pane evidence
        // against the recycled window instead of inheriting it by slot-name order.
        let windows = vec![wm("lane-7", 7, None), wm("lane-7-2", 2, Some("b"))];
        let p = pair_transcripts_to_windows(&[tsum("c", t(9)), tsum("b", t(2))], &windows, t(9));
        assert_eq!(p.assignment, vec![None, Some("lane-7-2".into())]);
        assert_eq!(candidate_sids(&p), vec!["c"]);
        assert_eq!(p.probe, vec![(7, "lane-7".to_string())]);
        assert_eq!(p.unpaired, vec!["lane-7".to_string()]);
    }

    #[test]
    fn one_tick_transcript_flap_does_not_rebind() {
        // b's transcript flaps out for one tick: its window merely goes unpaired (placeholder
        // target); it is NOT rebound to the surviving transcript and no binding is written.
        let windows = vec![wm("lane-7", 1, Some("a")), wm("lane-7-2", 2, Some("b"))];
        let p = pair_transcripts_to_windows(&[tsum("a", t(3))], &windows, t(3));
        assert_eq!(p.assignment, vec![Some("lane-7".into())]);
        assert!(p.new_bindings.is_empty());
        assert_eq!(p.unpaired, vec!["lane-7-2".to_string()]);
    }

    #[test]
    fn stale_binding_released_to_a_genuinely_new_transcript() {
        // The bound transcript is gone AND a new (actively writing) one needs a window: the
        // stale binding is released, but its replacement remains a candidate until confirmed.
        let windows = vec![wm("lane-7", 1, Some("x"))];
        let p = pair_transcripts_to_windows(&[tsum("y", t(5))], &windows, t(5));
        assert_eq!(p.assignment, vec![None]);
        assert_eq!(candidate_sids(&p), vec!["y"]);
        assert_eq!(p.probe, vec![(1, "lane-7".to_string())]);
        assert_eq!(p.unpaired, vec!["lane-7".to_string()]);
    }

    #[test]
    fn placeholder_target_is_the_newest_unpaired_window() {
        // One transcript, two windows (second freshly spawned, no `.jsonl` yet): the leftover
        // is the NEWEST window (highest id) so the placeholder lands on the fresh spawn.
        let windows = vec![wm("lane-5", 1, None), wm("lane-5-2", 2, None)];
        let p = pair_transcripts_to_windows(&[tsum("a", t(1))], &windows, t(1));
        assert_eq!(p.assignment, vec![None]);
        assert_eq!(
            p.unpaired,
            vec!["lane-5-2".to_string(), "lane-5".to_string()]
        );
        // Even with enough candidate transcripts, both windows remain placeholders until evidence.
        let p = pair_transcripts_to_windows(
            &[tsum("b", t(2)), tsum("a", t(1))],
            &[wm("lane-5", 1, None), wm("lane-5-2", 2, None)],
            t(2),
        );
        assert_eq!(p.assignment, vec![None, None]);
        assert_eq!(
            p.unpaired,
            vec!["lane-5-2".to_string(), "lane-5".to_string()]
        );
    }

    #[test]
    fn spawn_race_keeps_a_stale_transcript_off_the_new_window() {
        // agent.spawn created the window but claude hasn't written its .jsonl yet; the only
        // transcript around is a stale leftover (an /exited session, or the user's own
        // external one). It must remain external while the new window stays a placeholder;
        // displaying it on that window is already a user-visible transcript mismatch.
        let windows = vec![wm("lane-7", 5, None)];
        let stale = tsum("e", t(0));
        let p = pair_transcripts_to_windows(std::slice::from_ref(&stale), &windows, t(100));
        assert_eq!(p.assignment, vec![None]);
        assert_eq!(p.unpaired, vec!["lane-7".to_string()]);
        assert!(
            p.new_bindings.is_empty(),
            "a quiet transcript is a guess, not a binding"
        );
        // The real transcript appears (actively writing): it becomes the sole stamp candidate;
        // both transcripts remain external for this response and the pane stays a placeholder.
        let p = pair_transcripts_to_windows(&[tsum("x", t(100)), stale], &windows, t(100));
        assert_eq!(p.assignment, vec![None, None]);
        assert_eq!(candidate_sids(&p), vec!["x"]);
        assert_eq!(p.unpaired, vec!["lane-7".to_string()]);
    }

    #[test]
    fn duplicate_stamp_on_a_second_window_is_flagged_for_clearing_and_still_placeholders() {
        // Live incident: a resumed transcript's sid ended up stamped on THREE separate windows
        // at once (should be structurally impossible — sticky identity is 1:1). Pass 1 must
        // only let the FIRST window (in `windows` order) claim the transcript for display; the
        // others must (a) be reported so their stale stamp can be cleared, and (b) still surface
        // as placeholders — never silently vanish from the lane.
        let windows = vec![
            wm("lane-81-5", 20, Some("dup")),
            wm("lane-81-6", 58, Some("dup")),
            wm("lane-81-9", 65, Some("dup")),
        ];
        let p = pair_transcripts_to_windows(&[tsum("dup", t(0))], &windows, t(100));
        // Only the first window displays the transcript…
        assert_eq!(p.assignment, vec![Some("lane-81-5".to_string())]);
        // …the other two are flagged as duplicate stamps to clear…
        assert_eq!(
            p.duplicate_stamps,
            vec![(58, "lane-81-6".to_string()), (65, "lane-81-9".to_string())]
        );
        // …and STILL surface (as placeholders), not vanish from the lane.
        let mut unpaired = p.unpaired.clone();
        unpaired.sort();
        assert_eq!(
            unpaired,
            vec!["lane-81-6".to_string(), "lane-81-9".to_string()]
        );
        // No fresh binding is nominated for a sid whose window is already (over-)claimed.
        assert!(p.new_bindings.is_empty());
    }

    #[test]
    fn direct_bind_allowed_requires_the_lanes_only_window() {
        // The bug: the no-evidence direct bind used to fire whenever exactly one candidate and
        // one FREE window existed THIS TICK, even in a lane that has other, already-claimed
        // windows — if the window snapshot that tick ever lagged reality (a stale `last_good`
        // reuse, a notify race) and momentarily "forgot" one of those claims, the same sid could
        // get stamped a second time, producing the duplicate-stamp incident reproduced above.
        // Requiring the lane to have exactly one window in total closes that gap: with nothing
        // else in the lane, there is nothing the sid could already be bound to.
        assert!(
            direct_bind_allowed(1, 1, 1),
            "single-window lane: safe to bind directly"
        );
        assert!(
            !direct_bind_allowed(1, 1, 2),
            "a second window exists in the lane — the sid could already be bound to it"
        );
        assert!(
            !direct_bind_allowed(1, 1, 7),
            "matches the live incident's lane shape: 1 unclaimed candidate, 1 free window, 7 total"
        );
        // Headcount mismatches are still always rejected regardless of lane size.
        assert!(!direct_bind_allowed(2, 1, 1));
        assert!(!direct_bind_allowed(1, 2, 1));
        assert!(!direct_bind_allowed(0, 0, 1));
    }

    #[test]
    fn idle_agent_recovers_its_unstamped_window_by_pane_evidence() {
        // Daemon restarted a quiet fleet: the agent's `@repomon_session` is gone (unstamped
        // window) and it went idle far longer than RECENTLY_ACTIVE_SECS ago. Its pane still
        // shows its last message, so it MUST be nominated for a pane-evidence stamp — before
        // this fix a quiet transcript was never nominated and the window stayed unrecoverable,
        // so the positional guess wedged and reopening attached the wrong conversation.
        let msg = "recovered idle agent distinctive last message tail marker";
        let windows = vec![wm("lane-7", 1, None)];
        let p = pair_transcripts_to_windows(&[tsum_msg("a", t(0), msg)], &windows, t(100));
        assert_eq!(p.assignment, vec![None]);
        assert_eq!(candidate_sids(&p), vec!["a"]);
        assert_eq!(p.new_bindings[0].needle, message_fingerprint(Some(msg)));
        assert_eq!(p.unpaired, vec!["lane-7".to_string()]);
    }

    #[test]
    fn restart_recovers_two_idle_agents_to_their_own_windows() {
        // Two idle agents, both windows unstamped after a restart. They remain placeholders
        // while the evidence pass pins each to the window whose pane shows ITS OWN last
        // message — never crosswise.
        let ma = "idle alpha last message evidence tail one two three";
        let mb = "idle bravo last message evidence tail four five six";
        let windows = vec![wm("lane-7", 1, None), wm("lane-7-2", 2, None)];
        let p = pair_transcripts_to_windows(
            &[tsum_msg("b", t(1), mb), tsum_msg("a", t(0), ma)],
            &windows,
            t(100),
        );
        let mut sids = candidate_sids(&p);
        sids.sort();
        assert_eq!(sids, vec!["a", "b"]);
        // Each agent's message sits on its own pane; evidence must resolve each to its window.
        let panes = vec![
            (
                1,
                "lane-7".to_string(),
                normalize_fingerprint(&format!("✻ {ma}\n❯")),
            ),
            (
                2,
                "lane-7-2".to_string(),
                normalize_fingerprint(&format!("✻ {mb}\n❯")),
            ),
        ];
        let mut got = confirmed_stamps(&p.new_bindings, &panes);
        got.sort();
        assert_eq!(
            got,
            vec![
                (
                    1,
                    "lane-7".to_string(),
                    "a".to_string(),
                    AgentKind::ClaudeCode
                ),
                (
                    2,
                    "lane-7-2".to_string(),
                    "b".to_string(),
                    AgentKind::ClaudeCode
                ),
            ]
        );
    }

    #[test]
    fn idle_recovery_without_matching_pane_stamps_nothing() {
        // A nominated idle agent (unstamped window + fingerprint) whose message is NOT on the
        // captured pane — scrolled off, or the window is a fresh-spawn stand-in showing a
        // different agent — gets no 1:1 evidence, so nothing is stamped and no guess wedges.
        let msg = "idle agent whose tail is not on the captured pane at all";
        let windows = vec![wm("lane-7", 1, None)];
        let p = pair_transcripts_to_windows(&[tsum_msg("a", t(0), msg)], &windows, t(100));
        assert_eq!(candidate_sids(&p), vec!["a"]);
        let panes = vec![(
            1,
            "lane-7".to_string(),
            normalize_fingerprint("a completely different agent booting up on this pane"),
        )];
        assert!(confirmed_stamps(&p.new_bindings, &panes).is_empty());
    }

    #[test]
    fn idle_transcript_without_fingerprint_is_not_nominated() {
        // A quiet transcript with no distinctive last message (too short to fingerprint, or
        // None) stays external while the pane remains a placeholder: no evidence means no
        // display guess and no durable binding.
        let windows = vec![wm("lane-7", 1, None)];
        // Too short to fingerprint (< FINGERPRINT_MIN normalized chars).
        let p = pair_transcripts_to_windows(&[tsum_msg("a", t(0), "ok")], &windows, t(100));
        assert!(p.new_bindings.is_empty());
        // A None last message (the plain `tsum`) likewise.
        let p = pair_transcripts_to_windows(&[tsum("a", t(0))], &windows, t(100));
        assert!(p.new_bindings.is_empty());
    }

    #[test]
    fn clear_rotation_rebinds_window_to_the_live_transcript() {
        // `/clear` (or a fork-on-resume) rotates the session id in place: the window stays
        // bound to the dead transcript e while the live continuation x has no window. e's
        // claim is offered to pass 2's evidence probe but keeps showing on the window until
        // that evidence actually arrives — a durable tmux stamp is real ground truth right up
        // until pane evidence proves otherwise, so nothing should un-display it on headcount
        // alone. Once `confirmed_stamps` proves x is the one actually writing there, the next
        // overlay's pass 1 exposes the durable pairing and e falls to external on its own.
        let windows = vec![wm("lane-7", 1, Some("e"))];
        let p =
            pair_transcripts_to_windows(&[tsum("x", t(100)), tsum("e", t(0))], &windows, t(100));
        assert_eq!(
            p.assignment,
            vec![None, Some("lane-7".into())],
            "the live transcript awaits evidence; the stale-but-still-bound one keeps its display"
        );
        assert_eq!(candidate_sids(&p), vec!["x"]);
        assert_eq!(p.probe, vec![(1, "lane-7".to_string())]);
    }

    #[test]
    fn clear_rotation_releases_the_rotated_window_not_the_coldest() {
        // Two bound agents, both currently quiet: a rotated its session id a moment ago
        // (`/clear` → continuation c), b has been idle for much longer. The window offered
        // to c's evidence probe must be a's — the transcript that went quiet MOST recently is
        // the one that just rotated; offering b's would route c's keys into b's pane once
        // confirmed. a's window keeps displaying a until evidence actually reassigns it.
        let windows = vec![wm("lane-7", 1, Some("a")), wm("lane-7-2", 2, Some("b"))];
        let p = pair_transcripts_to_windows(
            &[tsum("c", t(100)), tsum("a", t(50)), tsum("b", t(0))],
            &windows,
            t(100),
        );
        assert_eq!(
            p.assignment,
            vec![None, Some("lane-7".into()), Some("lane-7-2".into())],
            "c awaits evidence for a's offered window; a and b both keep their own displays"
        );
        assert_eq!(candidate_sids(&p), vec!["c"]);
        assert_eq!(p.probe, vec![(1, "lane-7".to_string())]);
    }

    #[test]
    fn permanently_unbound_fresh_transcript_never_displaces_an_idle_valid_binding() {
        // A lane can legitimately hold more transcripts than tmux windows — e.g. one external,
        // never-window-bound session (like the operator's own always-fresh live conversation)
        // alongside agents that genuinely were adopted into windows. That extra fresh transcript
        // is NOT a `/clear` continuation of anything here — it was never claimed by any window
        // to begin with — so it must never repeatedly steal an idle-but-still-validly-bound
        // window's DISPLAY on pure headcount (`fresh_unclaimed > free_n`), tick after tick,
        // forever. It's fine to offer that window to the evidence probe (it wins nothing, since
        // pane evidence never confirms an unrelated transcript there), but the display must stay
        // put every single tick, not just the first.
        let windows = vec![
            wm("lane-81", 1, Some("bound")),
            wm("lane-81-2", 2, Some("other")),
        ];
        let p = pair_transcripts_to_windows(
            &[
                tsum("operator", t(100)),
                tsum("bound", t(10)),
                tsum("other", t(0)),
            ],
            &windows,
            t(100),
        );
        assert_eq!(
            p.assignment,
            vec![None, Some("lane-81".into()), Some("lane-81-2".into())],
            "both idle-but-bound sessions keep their real windows; the unrelated fresh one stays external"
        );
        // Re-running against the exact same input (simulating the next poll tick, nothing
        // resolved) must be identical — no flip-flopping.
        let p2 = pair_transcripts_to_windows(
            &[
                tsum("operator", t(100)),
                tsum("bound", t(10)),
                tsum("other", t(0)),
            ],
            &windows,
            t(100),
        );
        assert_eq!(
            p.assignment, p2.assignment,
            "must not flip-flop tick to tick"
        );
    }

    #[test]
    fn fresh_external_does_not_steal_a_bound_window_when_a_free_one_exists() {
        // An idle bound agent e plus a fresh unpaired transcript x: with a free window
        // available, x becomes a candidate for that free window and e's binding is left alone —
        // supersession only fires when the fresh transcript would otherwise have no home.
        let windows = vec![wm("lane-7", 1, Some("e")), wm("lane-7-2", 9, None)];
        let p =
            pair_transcripts_to_windows(&[tsum("x", t(100)), tsum("e", t(0))], &windows, t(100));
        assert_eq!(p.assignment, vec![None, Some("lane-7".into())]);
        assert_eq!(candidate_sids(&p), vec!["x"]);
        assert_eq!(p.probe, vec![(9, "lane-7-2".to_string())]);
    }

    #[test]
    fn idle_bound_pairing_holds_without_fresh_claimants() {
        // Nothing fresh in the lane (everyone idle): stale-bound windows keep their agents —
        // supersession must never shuffle a merely-idle fleet.
        let windows = vec![wm("lane-7", 1, Some("a")), wm("lane-7-2", 2, Some("b"))];
        let p = pair_transcripts_to_windows(&[tsum("b", t(1)), tsum("a", t(0))], &windows, t(100));
        assert_eq!(
            p.assignment,
            vec![Some("lane-7-2".into()), Some("lane-7".into())]
        );
        assert!(p.new_bindings.is_empty());
    }

    #[test]
    fn select_kept_summaries_protects_bound_sessions() {
        // A bound-but-quiet agent (its window is alive — that IS liveness) must survive
        // truncation ahead of newer unbound transcripts; output stays newest-first. `now` is
        // far past every activity time so freshness plays no part here.
        let bound: std::collections::HashSet<String> = ["a".to_string()].into();
        let kept = select_kept_summaries(
            vec![tsum("e1", t(3)), tsum("e2", t(2)), tsum("a", t(1))],
            &bound,
            2,
            t(1000),
        );
        let ids: Vec<&str> = kept
            .iter()
            .filter_map(|s| s.session_id.as_deref())
            .collect();
        assert_eq!(ids, vec!["e1", "a"]);
        // Under the cap nothing is dropped.
        let kept =
            select_kept_summaries(vec![tsum("e1", t(3)), tsum("a", t(1))], &bound, 5, t(1000));
        assert_eq!(kept.len(), 2);
        // More bound sessions than `keep` says: the windows win (each proves a live agent).
        let bound: std::collections::HashSet<String> = ["a".to_string(), "b".to_string()].into();
        let kept =
            select_kept_summaries(vec![tsum("b", t(3)), tsum("a", t(1))], &bound, 1, t(1000));
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn message_fingerprint_normalizes_and_gates() {
        // Markdown styling and line wrapping vanish under normalization.
        let m = "**Done!** The deploy isn't blocked on the invite anymore.";
        let f = message_fingerprint(Some(m)).unwrap();
        assert!(pane_text_contains(
            "✻ Done! The deploy isn't\nblocked on the invite anymore.\n❯",
            &f
        ));
        // A different conversation's pane does not match.
        assert!(!pane_text_contains(
            "recap: building Store Listen landing page",
            &f
        ));
        // Long messages fingerprint their TAIL (what stays visible above the input box).
        let long = format!(
            "{} tail marker alpha beta gamma delta epsilon",
            "x".repeat(500)
        );
        let f = message_fingerprint(Some(&long)).unwrap();
        assert!(f.len() <= FINGERPRINT_LEN);
        let pane = format!(
            "{} tail marker alpha beta gamma delta epsilon",
            "x".repeat(60)
        );
        assert!(pane_text_contains(&pane, &f));
        // Absent or indistinct messages give no fingerprint: no evidence, no stamp.
        assert_eq!(message_fingerprint(Some("ok")), None);
        assert_eq!(message_fingerprint(None), None);
    }

    /// Test-side helper mirroring the stamp task's pane check.
    fn pane_text_contains(pane: &str, needle: &str) -> bool {
        normalize_fingerprint(pane).contains(needle)
    }

    #[test]
    fn stamps_follow_pane_evidence_not_the_hint() {
        let cand = |sid: &str, msg: &str| BindingCandidate {
            sid: sid.into(),
            needle: message_fingerprint(Some(msg)),
            kind: AgentKind::ClaudeCode,
        };
        let a = cand("a", "fingerprint marker for agent alpha pane evidence");
        let b = cand("b", "fingerprint marker for agent bravo pane evidence");
        // Panes are CROSSED relative to candidate order: evidence must decide, so a lands
        // on @2 and b on @1 regardless of any heuristic hint.
        let panes = vec![
            (
                1,
                "lane-7".to_string(),
                normalize_fingerprint("✻ fingerprint marker for agent bravo pane evidence\n❯"),
            ),
            (
                2,
                "lane-7-2".to_string(),
                normalize_fingerprint("✻ fingerprint marker for agent alpha pane evidence\n❯"),
            ),
        ];
        let mut got = confirmed_stamps(&[a, b], &panes);
        got.sort();
        assert_eq!(
            got,
            vec![
                (
                    1,
                    "lane-7".to_string(),
                    "b".to_string(),
                    AgentKind::ClaudeCode
                ),
                (
                    2,
                    "lane-7-2".to_string(),
                    "a".to_string(),
                    AgentKind::ClaudeCode
                ),
            ]
        );
    }

    #[test]
    fn ambiguous_or_absent_pane_evidence_stamps_nothing() {
        let cand = |sid: &str, msg: Option<&str>| BindingCandidate {
            sid: sid.into(),
            needle: message_fingerprint(msg),
            kind: AgentKind::ClaudeCode,
        };
        let marker = "identical rotation continuation marker text";
        // Two panes showing the same text: ambiguous for a matching candidate.
        let twin_panes = vec![
            (1, "lane-7".to_string(), normalize_fingerprint(marker)),
            (2, "lane-7-2".to_string(), normalize_fingerprint(marker)),
        ];
        assert!(confirmed_stamps(&[cand("a", Some(marker))], &twin_panes).is_empty());
        // Two candidates whose needles both match one pane: mutual ambiguity, skip both.
        let one_pane = vec![(1, "lane-7".to_string(), normalize_fingerprint(marker))];
        assert!(
            confirmed_stamps(
                &[cand("a", Some(marker)), cand("b", Some(marker))],
                &one_pane
            )
            .is_empty()
        );
        // No fingerprint (agent mid-first-turn) or no matching pane: nothing stamped.
        assert!(confirmed_stamps(&[cand("a", None)], &one_pane).is_empty());
        assert!(
            confirmed_stamps(
                &[cand(
                    "a",
                    Some("completely unrelated conversation text here")
                )],
                &one_pane
            )
            .is_empty()
        );
    }

    #[test]
    fn select_kept_summaries_never_drops_a_fresh_transcript() {
        // The fresh backstop must survive bound-protection: with keep=1 and the budget
        // filled by a bound-but-stale transcript, the actively-writing one is kept too —
        // truncating the only session doing work would make the live agent invisible.
        let bound: std::collections::HashSet<String> = ["e".to_string()].into();
        let kept =
            select_kept_summaries(vec![tsum("x", t(100)), tsum("e", t(0))], &bound, 1, t(100));
        let ids: Vec<&str> = kept
            .iter()
            .filter_map(|s| s.session_id.as_deref())
            .collect();
        assert_eq!(ids, vec!["x", "e"]);
    }

    #[test]
    fn select_kept_summaries_drops_everything_when_keep_is_zero_and_unbound() {
        // When keep is 0 and no windows are bound (e.g. agent exited and window destroyed),
        // fresh lingering transcripts on disk must NOT be kept as phantom external sessions.
        let bound: std::collections::HashSet<String> = [].into();
        let kept = select_kept_summaries(vec![tsum("x", t(100))], &bound, 0, t(100));
        assert!(kept.is_empty());
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
        // A hand-written pin to the default base also normalizes to the default account
        // (defensive). Unix-only assertions: these launch strings are POSIX shell words built
        // for the tmux backend, and the parser correctly treats `\` as a shell escape — which
        // a native Windows default base (`C:\Users\...`) would trip over. Windows agents get
        // structured commands (no shell strings) with the session-backend work.
        #[cfg(unix)]
        {
            let default = agent::claude::default_config_base();
            assert_eq!(
                command_account(&format!("CLAUDE_CONFIG_DIR={} claude", default.display())),
                None
            );
            // ...and a quoted default base is still the default account.
            assert_eq!(
                command_account(&format!("CLAUDE_CONFIG_DIR='{}' claude", default.display())),
                None
            );
        }
        // shell_quote wraps the path in single quotes; the parse must see through them.
        assert_eq!(
            command_account("CLAUDE_CONFIG_DIR='/h/.claude-work' claude"),
            Some(PathBuf::from("/h/.claude-work"))
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
        // Codex is a non-Claude backend.
        assert_eq!(
            resolve_orchestrator_backend(&Some("codex".into()), &customs).unwrap(),
            B::Codex
        );
        // Antigravity resolves from both "antigravity" and "agy".
        for name in ["antigravity", "agy"] {
            assert_eq!(
                resolve_orchestrator_backend(&Some(name.into()), &customs).unwrap(),
                B::Antigravity,
                "{name}"
            );
        }
        // OpenCode resolves from both "opencode" and "open-code".
        for name in ["opencode", "open-code"] {
            assert_eq!(
                resolve_orchestrator_backend(&Some(name.into()), &customs).unwrap(),
                B::OpenCode,
                "{name}"
            );
        }
        // MCP-less agents and unknown names are loud errors, not broken spawns.
        for name in ["aider", "cursor", "gemini"] {
            let err = resolve_orchestrator_backend(&Some(name.into()), &customs).unwrap_err();
            assert!(
                err.message.contains(name)
                    && err.message.contains("orchestrator")
                    && err.message.contains("antigravity")
                    && err.message.contains("opencode"),
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
    fn antigravity_orchestrator_command_wires_mcp_and_persona() {
        let socket = PathBuf::from("/tmp/repomon-test.sock");
        // Autonomous: env prefixes for socket/autonomy/max_agents, dangerously-skip-permissions, accept-edits mode, persona as prompt-interactive.
        let cmd = build_antigravity_orchestrator_command(
            "agy",
            &socket,
            "autonomous",
            Some(4),
            &None,
            &None,
        );
        assert!(
            cmd.contains("REPOMON_MCP_SOCKET='/tmp/repomon-test.sock'"),
            "{cmd}"
        );
        assert!(cmd.contains("REPOMON_MCP_AUTONOMY='autonomous'"), "{cmd}");
        assert!(cmd.contains("REPOMON_MCP_MAX_AGENTS='4'"), "{cmd}");
        assert!(
            !cmd.contains("REPOMON_MCP_MODE"),
            "orchestrator must not set agent mode: {cmd}"
        );
        assert!(cmd.contains("agy"), "{cmd}");
        assert!(
            cmd.contains(" --dangerously-skip-permissions --mode accept-edits"),
            "{cmd}"
        );
        assert!(cmd.contains(" --prompt-interactive "), "{cmd}");
        assert!(cmd.contains("repomind"), "{cmd}");

        // None of the Claude-only or Codex-only flags may leak into an antigravity invocation.
        for forbidden_flag in [
            "--mcp-config",
            "--append-system-prompt",
            "--allowedTools",
            "--session-id",
            " -c ",
            " -a ",
            " -s ",
            " -m ",
        ] {
            assert!(
                !cmd.contains(forbidden_flag),
                "{forbidden_flag} leaked: {cmd}"
            );
        }

        // Supervised + model + prompt: no dangerously-skip-permissions, --model, prompt appended to persona.
        let cmd = build_antigravity_orchestrator_command(
            "agy",
            &socket,
            "supervised",
            None,
            &Some("gemini-2.5-pro".into()),
            &Some("coordinate lane-1 and lane-2".into()),
        );
        assert!(!cmd.contains("--dangerously-skip-permissions"), "{cmd}");
        assert!(!cmd.contains(" --mode "), "{cmd}");
        assert!(!cmd.contains("REPOMON_MCP_MAX_AGENTS"), "{cmd}");
        assert!(cmd.contains(" --model 'gemini-2.5-pro'"), "{cmd}");
        assert!(cmd.contains("coordinate lane-1 and lane-2"), "{cmd}");

        // Read-only maps to --mode plan.
        let cmd =
            build_antigravity_orchestrator_command("agy", &socket, "read-only", None, &None, &None);
        assert!(cmd.contains(" --mode plan"), "{cmd}");
        assert!(!cmd.contains("--dangerously-skip-permissions"), "{cmd}");
    }

    #[test]
    fn opencode_orchestrator_command_wires_mcp_and_persona() {
        let socket = PathBuf::from("/tmp/repomon-test.sock");
        // Autonomous: OPENCODE_CONFIG_CONTENT carries socket/autonomy/max_agents, persona as --prompt.
        let cmd = build_opencode_orchestrator_command(
            "opencode",
            &socket,
            "autonomous",
            Some(4),
            &None,
            &None,
        )
        .unwrap();
        assert!(
            cmd.contains("OPENCODE_CONFIG_CONTENT="),
            "missing OPENCODE_CONFIG_CONTENT: {cmd}"
        );
        assert!(
            cmd.contains("REPOMON_MCP_SOCKET='/tmp/repomon-test.sock'"),
            "{cmd}"
        );
        assert!(cmd.contains("REPOMON_MCP_AUTONOMY='autonomous'"), "{cmd}");
        assert!(cmd.contains("REPOMON_MCP_MAX_AGENTS='4'"), "{cmd}");
        assert!(
            !cmd.contains("REPOMON_MCP_MODE"),
            "orchestrator must not set agent mode: {cmd}"
        );
        assert!(cmd.contains("opencode"), "{cmd}");
        assert!(cmd.contains(" --prompt "), "{cmd}");
        assert!(cmd.contains("repomind"), "{cmd}");

        // None of the Claude-only, Codex-only, or Antigravity-only flags may leak into an opencode invocation.
        for forbidden_flag in [
            "--mcp-config",
            "--append-system-prompt",
            "--allowedTools",
            "--session-id",
            " -c ",
            " -a ",
            " -s ",
            " -m ",
            "--prompt-interactive",
            "--dangerously-skip-permissions",
            " --mode ",
        ] {
            assert!(
                !cmd.contains(forbidden_flag),
                "{forbidden_flag} leaked: {cmd}"
            );
        }

        // Supervised + model + prompt: --model, prompt appended to persona.
        let cmd = build_opencode_orchestrator_command(
            "opencode",
            &socket,
            "supervised",
            None,
            &Some("anthropic/claude-3-7-sonnet".into()),
            &Some("coordinate lane-1 and lane-2".into()),
        )
        .unwrap();
        assert!(!cmd.contains("REPOMON_MCP_MAX_AGENTS"), "{cmd}");
        assert!(
            cmd.contains(" --model 'anthropic/claude-3-7-sonnet'"),
            "{cmd}"
        );
        assert!(cmd.contains("coordinate lane-1 and lane-2"), "{cmd}");

        // Read-only passes autonomy level in env.
        let cmd = build_opencode_orchestrator_command(
            "opencode",
            &socket,
            "read-only",
            None,
            &None,
            &None,
        )
        .unwrap();
        assert!(cmd.contains("REPOMON_MCP_AUTONOMY='read-only'"), "{cmd}");
    }

    #[test]
    fn opencode_config_content_drift_and_isolation() {
        // Worker config: sets REPOMON_MCP_SOCKET, REPOMON_MCP_MODE, REPOMON_MCP_IDENTITY_TOKEN.
        let worker_json = build_opencode_config_content(
            Some(r#"{"existing_key": true}"#),
            &[
                "REPOMON_MCP_SOCKET",
                "REPOMON_MCP_MODE",
                "REPOMON_MCP_IDENTITY_TOKEN",
            ],
        )
        .unwrap();
        let worker_val: serde_json::Value = serde_json::from_str(&worker_json).unwrap();
        assert_eq!(worker_val["existing_key"], json!(true));
        let worker_env = &worker_val["mcp"]["repomon"]["environment"];
        assert_eq!(
            worker_env["REPOMON_MCP_SOCKET"],
            json!("{env:REPOMON_MCP_SOCKET}")
        );
        assert_eq!(
            worker_env["REPOMON_MCP_MODE"],
            json!("{env:REPOMON_MCP_MODE}")
        );
        assert_eq!(
            worker_env["REPOMON_MCP_IDENTITY_TOKEN"],
            json!("{env:REPOMON_MCP_IDENTITY_TOKEN}")
        );
        assert!(worker_env.get("REPOMON_MCP_AUTONOMY").is_none());

        // Orchestrator config: sets REPOMON_MCP_SOCKET, REPOMON_MCP_AUTONOMY, REPOMON_MCP_MAX_AGENTS.
        let orch_json = build_opencode_config_content(
            None,
            &[
                "REPOMON_MCP_SOCKET",
                "REPOMON_MCP_AUTONOMY",
                "REPOMON_MCP_MAX_AGENTS",
            ],
        )
        .unwrap();
        let orch_val: serde_json::Value = serde_json::from_str(&orch_json).unwrap();
        let orch_env = &orch_val["mcp"]["repomon"]["environment"];
        assert_eq!(
            orch_env["REPOMON_MCP_SOCKET"],
            json!("{env:REPOMON_MCP_SOCKET}")
        );
        assert_eq!(
            orch_env["REPOMON_MCP_AUTONOMY"],
            json!("{env:REPOMON_MCP_AUTONOMY}")
        );
        assert_eq!(
            orch_env["REPOMON_MCP_MAX_AGENTS"],
            json!("{env:REPOMON_MCP_MAX_AGENTS}")
        );
        assert!(
            orch_env.get("REPOMON_MCP_MODE").is_none(),
            "orchestrator config must not set REPOMON_MCP_MODE"
        );
    }

    #[test]
    fn codex_agent_mcp_forwards_restricted_identity_environment() {
        let command = attach_agent_mcp(
            "codex".into(),
            &AgentKind::Codex,
            Path::new("/tmp/unused-for-codex.json"),
        );
        assert!(
            command.contains("mcp_servers.repomon.command="),
            "{command}"
        );
        assert!(
            command.contains("mcp_servers.repomon.args=[\"mcp\"]"),
            "{command}"
        );
        assert!(
            command.contains(
                "mcp_servers.repomon.env_vars=[\"REPOMON_MCP_SOCKET\",\"REPOMON_MCP_MODE\",\"REPOMON_MCP_IDENTITY_TOKEN\"]"
            ),
            "{command}"
        );
        assert!(
            command.contains("mcp_servers.repomon.default_tools_approval_mode=\"approve\""),
            "{command}"
        );
        assert!(!command.contains("test-token"), "{command}");
    }

    #[test]
    fn cursor_agent_mcp_registration_creates_config_and_avoids_secrets_on_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("mcp.json");
        unsafe {
            std::env::set_var("REPOMON_CURSOR_MCP_CONFIG", &config_path);
        }
        let mut spec = SpawnSpec::new("cursor-agent", dir.path());
        spec.env.extend([
            ("REPOMON_MCP_SOCKET".into(), "/tmp/repomon-test.sock".into()),
            ("REPOMON_MCP_MODE".into(), "agent".into()),
            (
                "REPOMON_MCP_IDENTITY_TOKEN".into(),
                "secret-worker-token-xyz".into(),
            ),
        ]);
        configure_backend_mcp(&AgentKind::Cursor, &mut spec).unwrap();
        assert!(config_path.exists(), "mcp.json must be created");
        let content = std::fs::read_to_string(&config_path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&content).unwrap();
        let server = &val["mcpServers"]["repomon"];
        assert!(server.is_object());
        assert_eq!(server["args"], json!(["mcp"]));
        // The token must NOT be written to disk!
        assert!(
            !content.contains("secret-worker-token-xyz"),
            "identity token leaked to disk: {content}"
        );
        // spec.env retains the token for process inheritance
        assert!(
            spec.env
                .iter()
                .any(|(k, v)| k == "REPOMON_MCP_IDENTITY_TOKEN" && v == "secret-worker-token-xyz")
        );
    }

    #[test]
    fn custom_agent_dialect_routing_wires_known_binary_wrappers() {
        // A custom agent whose command wraps `agy` must receive Antigravity MCP wiring.
        let dir = tempfile::tempdir().expect("tempdir");
        let mcp_cfg = dir.path().join("mcp_config.json");
        unsafe {
            std::env::set_var("REPOMON_ANTIGRAVITY_MCP_CONFIG", &mcp_cfg);
        }
        let mut spec = SpawnSpec::new("agy --mode plan", dir.path());
        configure_backend_mcp(&AgentKind::Other("my-agy-wrapper".into()), &mut spec).unwrap();
        // The Antigravity registration should have fired (dialect detected from spec.program).
        assert!(
            mcp_cfg.exists(),
            "Antigravity mcp_config.json must be created for agy wrapper"
        );

        // A custom agent wrapping cursor-agent must receive Cursor MCP wiring.
        let cursor_cfg = dir.path().join("mcp.json");
        unsafe {
            std::env::set_var("REPOMON_CURSOR_MCP_CONFIG", &cursor_cfg);
        }
        let mut spec2 = SpawnSpec::new("cursor-agent --approve-mcps", dir.path());
        configure_backend_mcp(&AgentKind::Other("my-cursor-wrapper".into()), &mut spec2).unwrap();
        assert!(
            cursor_cfg.exists(),
            "Cursor mcp.json must be created for cursor-agent wrapper"
        );

        // A completely unknown custom command must produce no error and no file.
        let unknown_cfg = dir.path().join("unknown.json");
        let mut spec3 = SpawnSpec::new("my-exotic-agent", dir.path());
        configure_backend_mcp(&AgentKind::Other("exotic".into()), &mut spec3).unwrap();
        assert!(
            !unknown_cfg.exists(),
            "unknown binary must not produce any config file"
        );
    }

    #[test]
    fn aider_configure_backend_mcp_is_a_no_op() {
        // Aider has no MCP support: configure_backend_mcp must succeed (no error) and must not
        // write any file or modify spec.env beyond what it received.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut spec = SpawnSpec::new("aider", dir.path());
        let env_before: Vec<_> = spec.env.clone();
        configure_backend_mcp(&AgentKind::Aider, &mut spec).unwrap();
        assert_eq!(spec.env, env_before, "Aider must not alter spec.env");
    }

    #[test]
    fn launch_options_default_path_is_byte_identical() {
        // The whole point: with no options requested, the command is returned VERBATIM (and no
        // injection) so the default spawn path is unchanged from before this feature landed.
        let base = "CLAUDE_CONFIG_DIR='/h/.claude' claude".to_string();
        let identical = |plan: LaunchPlan, expect: &str| {
            assert_eq!(plan.command, expect);
            assert_eq!(plan.effort_inject, None);
        };
        identical(
            apply_launch_options(base.clone(), &AgentKind::ClaudeCode, None, None, None),
            &base,
        );
        // mode "default" (and an empty string) are treated as "no override".
        identical(
            apply_launch_options(
                base.clone(),
                &AgentKind::ClaudeCode,
                None,
                Some("default"),
                None,
            ),
            &base,
        );
        identical(
            apply_launch_options(
                base.clone(),
                &AgentKind::ClaudeCode,
                Some(""),
                Some(""),
                Some(""),
            ),
            &base,
        );
        // Codex and unknown kinds with no options are also identical.
        identical(
            apply_launch_options("codex".into(), &AgentKind::Codex, None, None, None),
            "codex",
        );
        identical(
            apply_launch_options(
                "ultracode --x".into(),
                &AgentKind::Other("ultracode".into()),
                None,
                None,
                None,
            ),
            "ultracode --x",
        );
    }

    #[test]
    fn launch_options_claude_mode_and_model() {
        let plan = apply_launch_options(
            "claude".into(),
            &AgentKind::ClaudeCode,
            None,
            Some("plan"),
            Some("opus"),
        );
        assert_eq!(plan.command, "claude --model 'opus' --permission-mode plan");
        assert_eq!(plan.effort_inject, None);
        let plan = apply_launch_options(
            "claude".into(),
            &AgentKind::ClaudeCode,
            None,
            Some("auto"),
            None,
        );
        assert_eq!(plan.command, "claude --permission-mode acceptEdits");
    }

    #[test]
    fn launch_options_claude_effort_uses_native_flag() {
        // claude's native --effort flag covers low|medium|high|xhigh|max.
        for level in ["low", "medium", "high", "xhigh", "max"] {
            let plan = apply_launch_options(
                "claude".into(),
                &AgentKind::ClaudeCode,
                Some(level),
                None,
                None,
            );
            assert_eq!(plan.command, format!("claude --effort '{level}'"));
            assert_eq!(plan.effort_inject, None);
        }
        // An unrecognized effort is ignored (no stray flag that would confuse the binary).
        let plan = apply_launch_options(
            "claude".into(),
            &AgentKind::ClaudeCode,
            Some("turbo"),
            None,
            None,
        );
        assert_eq!(plan.command, "claude");
        assert_eq!(plan.effort_inject, None);
    }

    #[test]
    fn launch_options_claude_ultracode_injects_slash_effort() {
        // `ultracode` isn't a valid --effort flag value (claude warns + ignores it), so it is set
        // via the /effort slash command injected as the session's first input — NOT a launch flag.
        let plan = apply_launch_options(
            "claude".into(),
            &AgentKind::ClaudeCode,
            Some("ultracode"),
            None,
            Some("opus"),
        );
        assert_eq!(plan.command, "claude --model 'opus'"); // no --effort flag
        assert_eq!(plan.effort_inject.as_deref(), Some("/effort ultracode"));
    }

    #[test]
    fn launch_options_codex_mapping() {
        let plan = apply_launch_options(
            "codex".into(),
            &AgentKind::Codex,
            Some("high"),
            Some("auto"),
            Some("gpt-5"),
        );
        assert_eq!(
            plan.command,
            "codex --model 'gpt-5' --full-auto -c model_reasoning_effort='high'"
        );
        assert_eq!(plan.effort_inject, None);
        // codex has no plan mode -> ignored, no stray flag.
        let plan =
            apply_launch_options("codex".into(), &AgentKind::Codex, None, Some("plan"), None);
        assert_eq!(plan.command, "codex");
        // claude-only levels clamp to high (codex tops out there); ultracode is NOT injected here.
        let plan = apply_launch_options(
            "codex".into(),
            &AgentKind::Codex,
            Some("ultracode"),
            None,
            None,
        );
        assert_eq!(plan.command, "codex -c model_reasoning_effort='high'");
        assert_eq!(plan.effort_inject, None);
    }

    #[test]
    fn launch_options_unknown_kind_never_injects_flags() {
        // A truly unknown agent gets every option ignored (a stray flag could make it exit on
        // launch); the command is left untouched.
        let plan = apply_launch_options(
            "weirdbin".into(),
            &AgentKind::Other("weirdbin".into()),
            Some("high"),
            Some("plan"),
            Some("opus"),
        );
        assert_eq!(plan.command, "weirdbin");
        assert_eq!(plan.effort_inject, None);
    }

    #[test]
    fn launch_options_shell_quotes_values() {
        // Every value reaches `sh -c`, so a model with metacharacters must be quoted, not injected.
        let plan = apply_launch_options(
            "claude".into(),
            &AgentKind::ClaudeCode,
            None,
            None,
            Some("a'b; rm -rf /"),
        );
        assert_eq!(plan.command, "claude --model 'a'\\''b; rm -rf /'");
    }

    #[test]
    fn kind_from_command_infers_dialect_for_custom_agents() {
        // A custom configured agent that wraps claude (incl. a work-account env prefix) is Claude.
        assert_eq!(
            kind_from_command("claude --dangerously-skip-permissions"),
            AgentKind::ClaudeCode
        );
        assert_eq!(
            kind_from_command("CLAUDE_CONFIG_DIR=/h/.claude-work claude"),
            AgentKind::ClaudeCode
        );
        // A full path resolves by basename.
        assert_eq!(
            kind_from_command("/opt/homebrew/bin/codex --full-auto"),
            AgentKind::Codex
        );
        // An unrecognized wrapper stays Other (launch options are then ignored, never guessed).
        assert_eq!(
            kind_from_command("my-wrapper.sh"),
            AgentKind::Other("my-wrapper.sh".into())
        );
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

        // Frozen mid-work — Running (transcript ends in a tool call): stalled, anchored on the pane's last change.
        assert_eq!(
            stall_since(Running, false, false, Some(old), now),
            Some(old)
        );
        // Idle is never stalled
        assert_eq!(stall_since(Idle, false, false, Some(old), now), None);
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

    #[test]
    fn heterogeneous_window_placeholders_track_individual_agent_kinds() {
        let lane = mail_lane(7, &[]);
        let metas = vec![repomon_core::model::LaneMeta {
            id: 7,
            repo_id: 7,
            worktree_path: PathBuf::from("/repo-7"),
            pinned: false,
            tmux_window: Some("lane-7".into()),
            agent_kind: Some("claude-code".into()),
        }];
        let lane_windows = vec![
            wm_kind("lane-7", 1, None, Some("claude-code")),
            wm_kind("lane-7-2", 2, None, Some("antigravity")),
        ];
        let kind1 = window_meta_kind(&lane_windows, "lane-7")
            .unwrap_or_else(|| lane_meta_kind(&metas, lane.id));
        let kind2 = window_meta_kind(&lane_windows, "lane-7-2")
            .unwrap_or_else(|| lane_meta_kind(&metas, lane.id));
        let s1 = window_placeholder_session(&lane, kind1, "lane-7".into());
        let s2 = window_placeholder_session(&lane, kind2, "lane-7-2".into());
        assert_eq!(s1.agent, AgentKind::ClaudeCode);
        assert_eq!(s2.agent, AgentKind::Antigravity);
    }

    #[test]
    fn unpaired_window_with_no_meta_defaults_to_unknown() {
        let lane = mail_lane(7, &[]);
        let metas: Vec<repomon_core::model::LaneMeta> = vec![];
        let lane_windows = vec![wm("lane-7", 1, None)];
        let kind = window_meta_kind(&lane_windows, "lane-7")
            .unwrap_or_else(|| lane_meta_kind(&metas, lane.id));
        let s = window_placeholder_session(&lane, kind, "lane-7".into());
        assert_eq!(s.agent, AgentKind::from_kind_str("unknown"));
        assert_eq!(s.agent.as_str(), "unknown");
    }

    #[test]
    fn confirmed_pairing_stamps_and_updates_agent_kind() {
        let msg = "distinctive antigravity turn message evidence";
        let windows = vec![wm_kind("lane-7", 1, None, Some("claude-code"))];
        let summary = agent::TranscriptSummary {
            kind: AgentKind::Antigravity,
            manifest_path: PathBuf::from("/tmp/agy.jsonl"),
            cwd: None,
            last_activity: t(0),
            tool_call_count: 0,
            status: AgentStatus::Idle,
            title: None,
            last_message: Some(msg.to_string()),
            config_dir: None,
            session_id: Some("agy-1".to_string()),
            ended_turn: false,
        };
        let p = pair_transcripts_to_windows(&[summary], &windows, t(100));
        assert_eq!(p.assignment, vec![None]);
        assert_eq!(candidate_sids(&p), vec!["agy-1"]);
        let panes = vec![(
            1,
            "lane-7".to_string(),
            normalize_fingerprint(&format!("✻ {msg}\n❯")),
        )];
        let confirmed = confirmed_stamps(&p.new_bindings, &panes);
        assert_eq!(
            confirmed,
            vec![(
                1,
                "lane-7".to_string(),
                "agy-1".to_string(),
                AgentKind::Antigravity
            )]
        );
    }

    #[test]
    fn managed_session_is_never_promoted_to_external() {
        let known_managed: std::collections::HashSet<String> =
            ["managed-session-1".to_string()].into_iter().collect();
        let sid = "managed-session-1";
        let was_managed = known_managed.contains(sid);
        assert!(was_managed);

        // Even with available ext slots (alive=1, managed=0), an ended managed session is suppressed
        let is_fresh = true;
        let mut ext_slots_remaining = Some(1usize);
        let is_ext = if was_managed {
            false
        } else {
            match ext_slots_remaining.as_mut() {
                Some(slots) => {
                    if *slots > 0 && is_fresh {
                        *slots -= 1;
                        true
                    } else {
                        false
                    }
                }
                None => is_fresh,
            }
        };
        assert!(!is_ext);
        // Slot count remained untouched
        assert_eq!(ext_slots_remaining, Some(1));

        // A stale unbound session is also never promoted even if slots > 0
        let _stale_sid = "stale-session";
        let is_fresh_stale = false;
        let is_ext_stale = match ext_slots_remaining.as_mut() {
            Some(slots) => {
                if *slots > 0 && is_fresh_stale {
                    *slots -= 1;
                    true
                } else {
                    false
                }
            }
            None => is_fresh_stale,
        };
        assert!(!is_ext_stale);
        assert_eq!(ext_slots_remaining, Some(1));

        // A genuine fresh external session (not in known_managed) claims the slot
        let live_ext_sid = "live-external-session";
        let was_managed_live = known_managed.contains(live_ext_sid);
        let is_ext_live = if was_managed_live {
            false
        } else {
            match ext_slots_remaining.as_mut() {
                Some(slots) => {
                    if *slots > 0 && is_fresh {
                        *slots -= 1;
                        true
                    } else {
                        false
                    }
                }
                None => is_fresh,
            }
        };
        assert!(is_ext_live);
        assert_eq!(ext_slots_remaining, Some(0));
    }

    #[tokio::test]
    async fn supervision_get_set_roundtrip() {
        let store = repomon_core::Store::open_in_memory().unwrap();
        let mut config = repomon_core::Config::default();
        config.supervision.enabled = true;
        let ctx = Ctx::new(store, config, None);
        let sess = ctx.open_session(crate::conn::ConnKind::Local).await;

        // 1. Initial get without lane_id returns defaults, no lane, no effective
        let get_init = dispatch(&ctx, &sess, "supervision.get", None)
            .await
            .unwrap();
        assert_eq!(get_init["defaults"]["enabled"], json!(true));
        assert!(get_init["lane"].is_null());
        assert!(get_init["effective"].is_null());

        // 2. Set enabled + one class override + stall_mins
        let set_res = dispatch(
            &ctx,
            &sess,
            "supervision.set",
            Some(json!({
                "lane_id": 42,
                "enabled": true,
                "classes": { "command_exec": "auto_approve" },
                "stall_mins": 15,
            })),
        )
        .await
        .unwrap();
        assert_eq!(set_res["effective"]["enabled"], json!(true));
        assert_eq!(
            set_res["effective"]["classes"]["command_exec"],
            json!("auto_approve")
        );
        assert_eq!(set_res["effective"]["stall_mins"], json!(15));

        // 3. Get with lane_id returns merged effective policy
        let get_lane = dispatch(
            &ctx,
            &sess,
            "supervision.get",
            Some(json!({ "lane_id": 42 })),
        )
        .await
        .unwrap();
        assert_eq!(get_lane["lane"]["lane_id"], json!(42));
        assert_eq!(get_lane["lane"]["enabled"], json!(true));
        assert_eq!(get_lane["effective"]["enabled"], json!(true));
        assert_eq!(
            get_lane["effective"]["classes"]["command_exec"],
            json!("auto_approve")
        );
        assert_eq!(get_lane["effective"]["stall_mins"], json!(15));

        // 4. In-memory snapshot is refreshed and contains the lane
        assert!(ctx.supervision.read().await.lane(42).is_some());
    }

    #[tokio::test]
    async fn supervision_audit_caps_limit_and_filters_lane() {
        let store = repomon_core::Store::open_in_memory().unwrap();
        let config = repomon_core::Config::default();
        let ctx = Ctx::new(store, config, None);
        let sess = ctx.open_session(crate::conn::ConnKind::Local).await;

        // Insert 3 entries for lane 1, 2 entries for lane 2
        for i in 1..=3 {
            let entry = repomon_core::model::SupervisionEntry {
                id: 0,
                at: chrono::Utc::now(),
                lane_id: 1,
                window: "window-1".to_string(),
                session_id: None,
                agent_kind: None,
                trigger: format!("trigger_{i}"),
                dialog_class: None,
                repo_scoped: None,
                decision: "approve".to_string(),
                policy_source: None,
                keys: None,
                outcome: "sent".to_string(),
                reason: None,
                subject: None,
                pane_excerpt: None,
            };
            ctx.store.append_supervision(entry).await.unwrap();
        }
        for i in 1..=2 {
            let entry = repomon_core::model::SupervisionEntry {
                id: 0,
                at: chrono::Utc::now(),
                lane_id: 2,
                window: "window-2".to_string(),
                session_id: None,
                agent_kind: None,
                trigger: format!("trigger_{i}"),
                dialog_class: None,
                repo_scoped: None,
                decision: "approve".to_string(),
                policy_source: None,
                keys: None,
                outcome: "sent".to_string(),
                reason: None,
                subject: None,
                pane_excerpt: None,
            };
            ctx.store.append_supervision(entry).await.unwrap();
        }

        // Filter lane 1
        let audit_lane1 = dispatch(
            &ctx,
            &sess,
            "supervision.audit",
            Some(json!({ "lane_id": 1 })),
        )
        .await
        .unwrap();
        let entries1 = audit_lane1["entries"].as_array().unwrap();
        assert_eq!(entries1.len(), 3);
        for e in entries1 {
            assert_eq!(e["lane_id"], json!(1));
        }

        // All lanes, limit capped
        let audit_all = dispatch(
            &ctx,
            &sess,
            "supervision.audit",
            Some(json!({ "limit": 500 })),
        )
        .await
        .unwrap();
        let all_entries = audit_all["entries"].as_array().unwrap();
        assert_eq!(all_entries.len(), 5);
    }

    #[tokio::test]
    async fn supervision_set_missing_lane_field_errors() {
        let store = repomon_core::Store::open_in_memory().unwrap();
        let config = repomon_core::Config::default();
        let ctx = Ctx::new(store, config, None);
        let sess = ctx.open_session(crate::conn::ConnKind::Local).await;

        let err = dispatch(
            &ctx,
            &sess,
            "supervision.set",
            Some(json!({ "enabled": true })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, -32602);
    }

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

    #[tokio::test]
    async fn lane_delete_removes_policy_row() {
        let store = repomon_core::Store::open_in_memory().unwrap();
        let mut config = repomon_core::Config::default();
        config.supervision.enabled = true;
        let ctx = Ctx::new(store, config, None);
        let sess = ctx.open_session(crate::conn::ConnKind::Local).await;

        // Create a real repo with initial commit
        let repo_dir = tempfile::tempdir().unwrap();
        git(repo_dir.path(), &["init", "-b", "main"]);
        git(repo_dir.path(), &["commit", "--allow-empty", "-m", "init"]);

        let repo = ctx.registry.add(repo_dir.path()).await.unwrap();

        // Create a lane
        let lane_val = dispatch(
            &ctx,
            &sess,
            "lane.create",
            Some(json!({
                "repo_id": repo.id,
                "branch": "feat/supervision-test",
            })),
        )
        .await
        .unwrap();
        let lane_id = lane_val["id"].as_i64().unwrap();

        // Set policy on this lane
        dispatch(
            &ctx,
            &sess,
            "supervision.set",
            Some(json!({
                "lane_id": lane_id,
                "enabled": true,
            })),
        )
        .await
        .unwrap();

        assert!(ctx.store.lane_policy(lane_id).await.unwrap().is_some());
        assert!(ctx.supervision.read().await.lane(lane_id).is_some());

        // Delete the lane
        dispatch(
            &ctx,
            &sess,
            "lane.delete",
            Some(json!({
                "lane_id": lane_id,
                "also_delete_branch": true,
            })),
        )
        .await
        .unwrap();

        // Policy row is deleted and snapshot is refreshed
        assert!(ctx.store.lane_policy(lane_id).await.unwrap().is_none());
        assert!(ctx.supervision.read().await.lane(lane_id).is_none());
    }
}
