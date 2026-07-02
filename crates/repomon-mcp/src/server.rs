//! The repomind MCP server: orchestrator-ergonomic tools over the repomon daemon.
//!
//! These tools are a *translation layer*, not new business logic — each maps to one or two
//! existing daemon RPCs. They are deliberately token-economical (compact digests, capped
//! transcripts, the raw pane only on request and even then capped) so the orchestrator can stay
//! oriented without drowning its context in worker output. Guardrails (autonomy, caps, dedupe)
//! are enforced here in [`crate::policy`], not merely asked for in the persona.

use chrono::Utc;
use repomon_core::agent::text::strip_ansi;
use repomon_core::client::DaemonClient;
use repomon_core::model::{AgentChoice, AgentSession, Lane, Repo, TranscriptItem};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

use crate::fleet::{self, Attention, Fleet, LaneDigest};
use crate::mcp::{ToolDef, ToolHandler, ToolResult};
use crate::policy::Policy;

pub struct Server {
    client: DaemonClient,
    fleet: Fleet,
    policy: Policy,
}

impl Server {
    pub fn new(client: DaemonClient, fleet: Fleet, policy: Policy) -> Self {
        Server {
            client,
            fleet,
            policy,
        }
    }
}

#[async_trait::async_trait]
impl ToolHandler for Server {
    fn tools(&self) -> Vec<ToolDef> {
        tool_catalog()
    }

    async fn call(&self, name: &str, args: Value) -> ToolResult {
        let out = match name {
            "fleet_status" => self.fleet_status(args).await,
            "read_agent" => self.read_agent(args).await,
            "spawn_agent" => self.spawn_agent(args).await,
            "send_to_agent" => self.send_to_agent(args).await,
            "approve_agent" => self.approve_agent(args).await,
            "interrupt_agent" => self.interrupt_agent(args).await,
            "stop_agent" => self.stop_agent(args).await,
            "create_lane" => self.create_lane(args).await,
            "delete_lane" => self.delete_lane(args).await,
            "merge_lane" => self.merge_lane(args).await,
            "lane_diff" => self.lane_diff(args).await,
            "list_repos" => self.list_repos(args).await,
            "wait_for_change" => self.wait_for_change(args).await,
            other => Err(format!("unknown tool: {other}")),
        };
        match out {
            Ok(v) => ToolResult::ok(v.to_string()),
            Err(e) => ToolResult::error(e),
        }
    }
}

impl Server {
    async fn fleet_status(&self, args: Value) -> Result<Value, String> {
        let a: FleetStatusArgs = parse(args)?;
        let (generation, mut lanes) = self.fleet.current().await;
        if let Some(repo) = &a.repo {
            lanes.retain(|l| &l.repo == repo);
        }
        let counts = tally(&lanes);
        if a.only_attention.unwrap_or(false) {
            lanes.retain(|l| l.attention().needs_you());
        }
        Ok(json!({ "generation": generation, "counts": counts, "lanes": lanes }))
    }

    async fn read_agent(&self, args: Value) -> Result<Value, String> {
        let a: ReadAgentArgs = parse(args)?;
        let limit = clamp_transcript_limit(a.transcript_limit);
        let max_chars = clamp_max_chars(a.max_chars);
        let lane: Lane = self
            .client
            .call_typed("lane.get", Some(json!({ "lane_id": a.lane_id })))
            .await
            .map_err(rpc_err)?;
        let digest = fleet::project_lane(&lane, Utc::now());
        let primary = fleet::primary_agent(&lane);
        let session_id = primary.and_then(|s| s.session_id.clone());
        let pending_prompt = primary.and_then(|s| s.pending_prompt.clone());

        let transcript: Vec<TranscriptItem> = self
            .client
            .call_typed(
                "agent.transcript",
                Some(json!({ "lane_id": a.lane_id, "limit": limit, "session_id": session_id })),
            )
            .await
            .unwrap_or_default();
        // Truncate each item first, then enforce the total budget by dropping the oldest
        // (transcript items arrive oldest-first) until the remainder fits.
        let truncated: Vec<String> = transcript
            .iter()
            .map(|t| truncate(&t.text, max_chars))
            .collect();
        let lens: Vec<usize> = truncated.iter().map(|s| s.chars().count()).collect();
        let drop = drop_oldest_for_budget(&lens, TRANSCRIPT_BUDGET_CHARS);
        let tail: Vec<Value> = transcript
            .iter()
            .zip(truncated.iter())
            .skip(drop)
            .map(|(t, text)| json!({ "role": t.role, "text": text, "at": t.at }))
            .collect();

        // Pane enrichment is best-effort: a gone/crashed window is exactly the crash-debugging
        // case a caller reaches for include_pane in, so a resolution or capture failure here
        // must not discard the transcript/git-state that *are* available. Failures degrade to
        // pane: null + a one-line pane_error instead of failing the whole call.
        let (pane, pane_error) = if a.include_pane.unwrap_or(false) {
            shape_pane(self.capture_pane(a.lane_id, primary).await)
        } else {
            (None, None)
        };

        Ok(json!({
            "lane_id": a.lane_id,
            "repo": digest.repo,
            "branch": digest.branch,
            "dirty": digest.dirty,
            "ahead": lane.state.ahead,
            "behind": lane.state.behind,
            "upstream": lane.state.upstream,
            "dirty_counts": {
                "staged": lane.state.dirty.staged,
                "unstaged": lane.state.dirty.unstaged,
                "untracked": lane.state.dirty.untracked,
            },
            "agent": digest.agent,
            "pending_prompt": pending_prompt,
            "transcript_tail": tail,
            "pane": pane,
            "pane_error": pane_error,
        }))
    }

    /// Fetch the live terminal pane for `read_agent`'s `include_pane` option. Read-only, so
    /// callers should treat failure as "no pane available", not a reason to fail the response —
    /// see the call site.
    async fn capture_pane(
        &self,
        lane_id: i64,
        primary: Option<&AgentSession>,
    ) -> Result<String, String> {
        let window = target_window(primary, None)?;
        let cap: CaptureResult = self
            .client
            .call_typed(
                "agent.capture",
                Some(json!({ "lane_id": lane_id, "window": window, "lines": 40 })),
            )
            .await
            .map_err(rpc_err)?;
        Ok(last_lines(&strip_ansi(&cap.content), 40, 4000))
    }

    async fn spawn_agent(&self, args: Value) -> Result<Value, String> {
        let a: SpawnAgentArgs = parse(args)?;
        self.policy.record_mutation()?;

        let (_, lanes) = self.fleet.current().await;
        // Count every active managed session, not one per lane: a lane can host several agents at
        // once, and the cap is a hard promise to the user.
        let active: usize = lanes.iter().map(|l| l.active_agents).sum();
        if active >= self.policy.max_concurrent_agents {
            return Err(format!(
                "at the concurrent-agent cap ({} active). Stop or finish an agent before \
                 spawning another, or relaunch with a higher --max-agents.",
                self.policy.max_concurrent_agents
            ));
        }

        let agent = match a.agent {
            Some(name) => name,
            None => self.default_agent().await,
        };
        let res: Value = self
            .client
            .call(
                "agent.spawn",
                Some(json!({ "lane_id": a.lane_id, "agent": agent, "task": a.task })),
            )
            .await
            .map_err(rpc_err)?;
        Ok(res)
    }

    async fn send_to_agent(&self, args: Value) -> Result<Value, String> {
        let a: SendToAgentArgs = parse(args)?;
        self.policy.record_mutation()?;
        let submit = a.submit.unwrap_or(true);
        self.policy.check_send_dedupe(a.lane_id, &a.text)?;
        // Target the session the orchestrator reasons about (the primary), not the daemon's default
        // first window — they differ in a multi-agent lane.
        let lane: Lane = self
            .client
            .call_typed("lane.get", Some(json!({ "lane_id": a.lane_id })))
            .await
            .map_err(rpc_err)?;
        let window = target_window(fleet::primary_agent(&lane), a.window)?;
        self.client
            .call(
                "agent.send_input",
                Some(json!({
                    "lane_id": a.lane_id,
                    "text": a.text,
                    "enter": submit,
                    "window": window,
                })),
            )
            .await
            .map_err(rpc_err)?;
        Ok(json!({ "ok": true }))
    }

    async fn approve_agent(&self, args: Value) -> Result<Value, String> {
        let a: ApproveAgentArgs = parse(args)?;
        self.policy.record_mutation()?;

        // Re-read fresh state: only routine permission dialogs may be auto-answered.
        let lane: Lane = self
            .client
            .call_typed("lane.get", Some(json!({ "lane_id": a.lane_id })))
            .await
            .map_err(rpc_err)?;
        let primary = fleet::primary_agent(&lane);
        let attention = primary
            .map(fleet::agent_attention)
            .unwrap_or(Attention::None);
        match attention {
            Attention::Permission => {}
            Attention::Decision => {
                return Err(
                    "this lane is on a DECISION, not a routine permission. Refusing to \
                     auto-answer — surface the exact question to the human, then relay their \
                     choice with approve_agent {choice: <number>} or send_to_agent."
                        .into(),
                );
            }
            Attention::EndOfTurn => {
                return Err(
                    "the agent ended its turn (no open dialog) — use send_to_agent to \
                     give it the next instruction, not approve_agent."
                        .into(),
                );
            }
            Attention::None => {
                return Err(
                    "no pending dialog on this lane to approve. Use read_agent to check \
                     its current state."
                        .into(),
                );
            }
        }

        // Answer the window the dialog is actually on (the primary), not the lane's first window.
        let window = target_window(primary, a.window)?;
        let (key, answered) = approve_key(a.choice.as_ref())?;
        self.client
            .call(
                "agent.key",
                Some(json!({ "lane_id": a.lane_id, "key": key, "window": window })),
            )
            .await
            .map_err(rpc_err)?;
        Ok(json!({
            "ok": true,
            "sent": key,
            "answered": answered,
            "prompt": primary.and_then(|s| s.pending_prompt.clone()),
        }))
    }

    async fn interrupt_agent(&self, args: Value) -> Result<Value, String> {
        let a: InterruptAgentArgs = parse(args)?;
        self.policy.record_mutation()?;
        if a.hard.unwrap_or(false) {
            self.client
                .call(
                    "agent.signal",
                    Some(json!({ "lane_id": a.lane_id, "key": "C-c" })),
                )
                .await
                .map_err(rpc_err)?;
        } else {
            self.client
                .call(
                    "agent.key",
                    Some(json!({ "lane_id": a.lane_id, "key": "Escape" })),
                )
                .await
                .map_err(rpc_err)?;
        }
        Ok(json!({ "ok": true }))
    }

    async fn stop_agent(&self, args: Value) -> Result<Value, String> {
        let a: StopAgentArgs = parse(args)?;
        self.policy.record_mutation()?;
        // Target the session the orchestrator reasons about (the primary), same as
        // send_to_agent — killing the daemon's default (first) window in a multi-agent lane
        // could end the wrong session.
        let lane: Lane = self
            .client
            .call_typed("lane.get", Some(json!({ "lane_id": a.lane_id })))
            .await
            .map_err(rpc_err)?;
        let window = target_window(fleet::primary_agent(&lane), a.window)?;
        self.client
            .call(
                "agent.stop",
                Some(json!({ "lane_id": a.lane_id, "window": window })),
            )
            .await
            .map_err(rpc_err)?;
        Ok(json!({ "ok": true }))
    }

    async fn create_lane(&self, args: Value) -> Result<Value, String> {
        let a: CreateLaneArgs = parse(args)?;
        self.policy.record_mutation()?;
        if !self.policy.autonomy.allows_structural() {
            return Err(
                "creating a lane needs the human's go-ahead at this autonomy level. Ask \
                 them to confirm the repo + branch, then proceed (or relaunch with \
                 --autonomy autonomous)."
                    .into(),
            );
        }
        let repos: Vec<Repo> = self
            .client
            .call_typed("repo.list", None)
            .await
            .map_err(rpc_err)?;
        let repo = repos
            .iter()
            .find(|r| r.name == a.repo || r.id.to_string() == a.repo)
            .ok_or_else(|| format!("no registered repo matching '{}'. Try list_repos.", a.repo))?;
        let lane: Lane = self
            .client
            .call_typed(
                "lane.create",
                Some(json!({
                    "repo_id": repo.id,
                    "branch": a.branch,
                    "source_branch": a.source_branch,
                    "copy_files": [],
                })),
            )
            .await
            .map_err(rpc_err)?;
        Ok(json!({
            "lane_id": lane.id,
            "path": lane.worktree.path,
            "branch": a.branch,
            "repo": repo.name,
        }))
    }

    /// Two-phase destructive delete: no `confirm` mints an impact summary + token (a normal,
    /// non-error tool result — the point is to hand the orchestrator something to relay to the
    /// human, not to fail); a matching `confirm` redeems the token and performs the delete.
    async fn delete_lane(&self, args: Value) -> Result<Value, String> {
        let a: DeleteLaneArgs = parse(args)?;
        // Defense-in-depth on top of the two-phase confirm below: gate on the same
        // structural-autonomy check create_lane/merge_lane use, checked before either phase so
        // supervised/read-only mode refuses early — before a token is ever minted.
        if !self.policy.autonomy.allows_structural() {
            return Err(
                "deleting a lane needs the human's go-ahead at this autonomy level. Ask them \
                 to confirm the delete, then proceed (or relaunch with --autonomy autonomous)."
                    .into(),
            );
        }
        let delete_branch = a.delete_branch.unwrap_or(false);
        // The discriminator a token is bound to: a token minted for delete_branch=false must not
        // confirm a delete_branch=true call on the same lane, or vice versa.
        let flags = format!("delete_branch={delete_branch}");

        match a.confirm {
            None => {
                let lane: Lane = self
                    .client
                    .call_typed("lane.get", Some(json!({ "lane_id": a.lane_id })))
                    .await
                    .map_err(rpc_err)?;
                let primary = fleet::primary_agent(&lane);
                let impact = json!({
                    "lane_id": a.lane_id,
                    "repo": lane.repo.name,
                    "branch": lane.state.branch.clone().unwrap_or_else(|| "(detached)".into()),
                    "worktree_path": lane.worktree.path,
                    "dirty_counts": {
                        "staged": lane.state.dirty.staged,
                        "unstaged": lane.state.dirty.unstaged,
                        "untracked": lane.state.dirty.untracked,
                    },
                    "ahead": lane.state.ahead,
                    "behind": lane.state.behind,
                    "active_agent": primary.map(|s| json!({
                        "kind": format!("{:?}", s.agent),
                        "status": format!("{:?}", s.status),
                    })),
                    "delete_branch": delete_branch,
                });
                let token = self.policy.mint_confirm(a.lane_id, &flags);
                Ok(json!({
                    "confirmation_required": true,
                    "impact": impact,
                    "token": token,
                    "instructions": "Show this impact to the human verbatim. Only after they \
                        explicitly approve, call delete_lane again with confirm=<token>.",
                }))
            }
            Some(token) => {
                self.policy.take_confirm(&token, a.lane_id, &flags)?;
                self.policy.record_mutation()?;
                self.client
                    .call(
                        "lane.delete",
                        Some(json!({
                            "lane_id": a.lane_id,
                            "also_delete_branch": delete_branch,
                        })),
                    )
                    .await
                    .map_err(rpc_err)?;
                Ok(json!({ "ok": true, "lane_id": a.lane_id, "delete_branch": delete_branch }))
            }
        }
    }

    async fn merge_lane(&self, args: Value) -> Result<Value, String> {
        let a: MergeLaneArgs = parse(args)?;
        self.policy.record_mutation()?;
        if !self.policy.autonomy.allows_structural() {
            return Err(
                "merging a lane needs the human's go-ahead at this autonomy level. Ask them \
                 to confirm the merge, then proceed (or relaunch with --autonomy autonomous)."
                    .into(),
            );
        }
        let lane: Lane = self
            .client
            .call_typed("lane.get", Some(json!({ "lane_id": a.lane_id })))
            .await
            .map_err(rpc_err)?;
        if !lane.state.dirty.is_clean() {
            return Err(
                "the worker has uncommitted changes that would NOT be merged — have it commit \
                 first (send_to_agent), then merge."
                    .into(),
            );
        }
        let res: Value = self
            .client
            .call(
                "lane.merge",
                Some(json!({ "lane_id": a.lane_id, "into": a.into })),
            )
            .await
            .map_err(rpc_err)?;
        Ok(res)
    }

    /// Read-only: no `record_mutation()` — this only inspects the lane, it changes nothing.
    async fn lane_diff(&self, args: Value) -> Result<Value, String> {
        let a: LaneDiffArgs = parse(args)?;
        let res: Value = self
            .client
            .call(
                "lane.diff",
                Some(json!({
                    "lane_id": a.lane_id,
                    "include_patch": a.include_patch,
                    "max_patch_chars": a.max_patch_chars,
                })),
            )
            .await
            .map_err(rpc_err)?;
        Ok(res)
    }

    async fn list_repos(&self, _args: Value) -> Result<Value, String> {
        let repos: Vec<Repo> = self
            .client
            .call_typed("repo.list", None)
            .await
            .map_err(rpc_err)?;
        let out: Vec<Value> = repos
            .iter()
            .map(|r| json!({ "repo_id": r.id, "name": r.name, "path": r.path }))
            .collect();
        Ok(json!({ "repos": out }))
    }

    async fn wait_for_change(&self, args: Value) -> Result<Value, String> {
        let a: WaitForChangeArgs = parse(args)?;
        let timeout = Duration::from_secs(a.timeout_secs.unwrap_or(60).clamp(1, 120));
        let until_needs_you = a.until.as_deref() == Some("needs_you");
        let filter = a.lanes.as_deref();

        let started = Instant::now();
        // Mark the current generation seen first, so `changed()` only reports genuinely new
        // edges (a freshly cloned receiver otherwise treats all past changes as unseen).
        let mut rx = self.fleet.watch();
        let mut seen_gen = *rx.borrow_and_update();
        let (_, baseline) = self.fleet.current().await;

        loop {
            // Evaluate whenever the generation has advanced past what we last looked at.
            let cur_gen = *rx.borrow();
            if cur_gen != seen_gen {
                let (_, current) = self.fleet.current().await;
                let deltas = diff(&baseline, &current, filter);
                let relevant = if until_needs_you {
                    deltas.iter().any(|d| d.needs_you)
                } else {
                    !deltas.is_empty()
                };
                if relevant {
                    return Ok(json!({
                        "changed": true,
                        "deltas": deltas,
                        "elapsed_secs": started.elapsed().as_secs(),
                    }));
                }
                seen_gen = cur_gen; // accumulate further changes against the original baseline
            }

            let remaining = match timeout.checked_sub(started.elapsed()) {
                Some(r) if !r.is_zero() => r,
                _ => {
                    let (_, current) = self.fleet.current().await;
                    let deltas = diff(&baseline, &current, filter);
                    return Ok(json!({
                        "changed": false,
                        "deltas": deltas,
                        "elapsed_secs": started.elapsed().as_secs(),
                    }));
                }
            };

            tokio::select! {
                res = rx.changed() => {
                    if res.is_err() {
                        return Err("fleet watch closed (daemon connection lost)".into());
                    }
                }
                _ = tokio::time::sleep(remaining) => {}
            }
        }
    }

    async fn default_agent(&self) -> String {
        match self
            .client
            .call_typed::<Vec<AgentChoice>>("agent.detect", None)
            .await
        {
            Ok(choices) => choices
                .into_iter()
                .find(|c| c.default)
                .map(|c| c.name)
                .unwrap_or_else(|| "claude-code".into()),
            Err(_) => "claude-code".into(),
        }
    }
}

// ---- argument structs ----

#[derive(Deserialize)]
struct FleetStatusArgs {
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    only_attention: Option<bool>,
}
#[derive(Deserialize)]
struct ReadAgentArgs {
    lane_id: i64,
    #[serde(default)]
    transcript_limit: Option<usize>,
    /// Per-item truncation in chars. Defaults to 500, clamped to 100-2000.
    #[serde(default)]
    max_chars: Option<usize>,
    /// Include the last ~40 lines of the live terminal pane (default false).
    #[serde(default)]
    include_pane: Option<bool>,
}
#[derive(Deserialize)]
struct SpawnAgentArgs {
    lane_id: i64,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    task: Option<String>,
}
#[derive(Deserialize)]
struct SendToAgentArgs {
    lane_id: i64,
    text: String,
    #[serde(default)]
    submit: Option<bool>,
    /// Target a specific agent window in a multi-agent lane. Defaults to the lane's primary
    /// (most-attention-worthy) managed session.
    #[serde(default)]
    window: Option<String>,
}
#[derive(Deserialize)]
struct ApproveAgentArgs {
    lane_id: i64,
    #[serde(default)]
    choice: Option<Value>,
    /// Target a specific agent window in a multi-agent lane. Defaults to the lane's primary
    /// (most-attention-worthy) managed session.
    #[serde(default)]
    window: Option<String>,
}
#[derive(Deserialize)]
struct InterruptAgentArgs {
    lane_id: i64,
    #[serde(default)]
    hard: Option<bool>,
}
#[derive(Deserialize)]
struct StopAgentArgs {
    lane_id: i64,
    /// Target a specific agent window in a multi-agent lane. Defaults to the lane's primary
    /// (most-attention-worthy) managed session.
    #[serde(default)]
    window: Option<String>,
}
#[derive(Deserialize)]
struct CreateLaneArgs {
    repo: String,
    branch: String,
    #[serde(default)]
    source_branch: Option<String>,
}
#[derive(Deserialize)]
struct DeleteLaneArgs {
    lane_id: i64,
    #[serde(default)]
    delete_branch: Option<bool>,
    /// The token from a prior no-`confirm` call. Absent (phase 1) returns an impact summary and
    /// mints a token instead of deleting anything.
    #[serde(default)]
    confirm: Option<String>,
}
#[derive(Deserialize)]
struct MergeLaneArgs {
    lane_id: i64,
    #[serde(default)]
    into: Option<String>,
}
#[derive(Deserialize)]
struct LaneDiffArgs {
    lane_id: i64,
    #[serde(default)]
    include_patch: bool,
    #[serde(default = "default_max_patch_chars")]
    max_patch_chars: usize,
}
fn default_max_patch_chars() -> usize {
    8000
}
#[derive(Deserialize)]
struct WaitForChangeArgs {
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    until: Option<String>,
    #[serde(default)]
    lanes: Option<Vec<i64>>,
}

// ---- helpers ----

/// The daemon's `agent.capture` response.
#[derive(Deserialize)]
struct CaptureResult {
    content: String,
}

#[derive(Serialize)]
struct Delta {
    lane_id: i64,
    repo: String,
    from: String,
    to: String,
    attention: Attention,
    needs_you: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    headline: Option<String>,
}

fn diff(baseline: &[LaneDigest], current: &[LaneDigest], filter: Option<&[i64]>) -> Vec<Delta> {
    use std::collections::HashMap;
    let want = |id: i64| filter.is_none_or(|f| f.contains(&id));
    let base: HashMap<i64, &LaneDigest> = baseline.iter().map(|d| (d.lane_id, d)).collect();
    let cur: HashMap<i64, &LaneDigest> = current.iter().map(|d| (d.lane_id, d)).collect();
    let mut deltas = Vec::new();

    for d in current.iter().filter(|d| want(d.lane_id)) {
        let to_att = d.attention();
        match base.get(&d.lane_id) {
            Some(b) if b.status() == d.status() && b.attention() == to_att => {}
            Some(b) => deltas.push(Delta {
                lane_id: d.lane_id,
                repo: d.repo.clone(),
                from: b.status().to_string(),
                to: d.status().to_string(),
                attention: to_att,
                needs_you: to_att.needs_you(),
                headline: d.headline().map(str::to_string),
            }),
            None => deltas.push(Delta {
                lane_id: d.lane_id,
                repo: d.repo.clone(),
                from: "absent".into(),
                to: d.status().to_string(),
                attention: to_att,
                needs_you: to_att.needs_you(),
                headline: d.headline().map(str::to_string),
            }),
        }
    }
    for b in baseline.iter().filter(|b| want(b.lane_id)) {
        if !cur.contains_key(&b.lane_id) {
            deltas.push(Delta {
                lane_id: b.lane_id,
                repo: b.repo.clone(),
                from: b.status().to_string(),
                to: "gone".into(),
                attention: Attention::None,
                needs_you: false,
                headline: None,
            });
        }
    }
    deltas
}

fn tally(lanes: &[LaneDigest]) -> Value {
    let (mut running, mut waiting, mut permission, mut decision) = (0, 0, 0, 0);
    let (mut end_of_turn, mut rate_limited, mut idle, mut no_agent) = (0, 0, 0, 0);
    for l in lanes {
        match l.attention() {
            Attention::Permission => {
                permission += 1;
                waiting += 1;
            }
            Attention::Decision => {
                decision += 1;
                waiting += 1;
            }
            Attention::EndOfTurn => {
                end_of_turn += 1;
                waiting += 1;
            }
            Attention::None => match l.status() {
                "running" => running += 1,
                "rate-limited" => rate_limited += 1,
                "idle" => idle += 1,
                "no-agent" => no_agent += 1,
                _ => {}
            },
        }
    }
    json!({
        "running": running,
        "needs_you": waiting,
        "permission": permission,
        "decision": decision,
        "end_of_turn": end_of_turn,
        "rate_limited": rate_limited,
        "idle": idle,
        "no_agent": no_agent,
    })
}

/// Map an `approve_agent` choice to the tmux key to send and a human-readable summary.
/// Default (absent / "yes") selects the highlighted option (Yes); "no" cancels with Escape; a
/// number selects that menu option.
fn approve_key(choice: Option<&Value>) -> Result<(String, String), String> {
    match choice {
        None => Ok(("Enter".into(), "yes (default)".into())),
        Some(Value::String(s)) => {
            let l = s.trim().to_lowercase();
            match l.as_str() {
                "" | "yes" | "y" | "approve" | "accept" => Ok(("Enter".into(), "yes".into())),
                "no" | "n" | "reject" | "deny" | "cancel" => {
                    Ok(("Escape".into(), "no (cancelled)".into()))
                }
                digits if digits.chars().all(|c| c.is_ascii_digit()) && !digits.is_empty() => {
                    Ok((digits.to_string(), format!("option {digits}")))
                }
                other => Err(format!(
                    "choice must be \"yes\", \"no\", or an option number — got '{other}'"
                )),
            }
        }
        Some(Value::Number(n)) => {
            let i = n
                .as_u64()
                .ok_or("option number must be a positive integer")?;
            Ok((i.to_string(), format!("option {i}")))
        }
        Some(_) => Err("choice must be a string (\"yes\"/\"no\") or an option number".into()),
    }
}

/// Resolve which agent window an action should target on a lane: an explicit override wins,
/// otherwise the primary (most-attention-worthy) managed session's window. Errors when the resolved
/// session is external or windowless, so we never blind-send to the daemon's default (first) window
/// — which in a multi-agent lane may be a different session than the one the orchestrator inspected.
fn target_window(
    primary: Option<&AgentSession>,
    explicit: Option<String>,
) -> Result<String, String> {
    if let Some(w) = explicit {
        return Ok(w);
    }
    let p = primary.ok_or("no agent session on this lane to target")?;
    if p.external {
        return Err(
            "the lane's active session is external (not managed by repomon); refusing to \
             act on it automatically. Surface it to the human instead."
                .into(),
        );
    }
    p.tmux_window
        .clone()
        .ok_or_else(|| "the lane's active session has no tmux window to target".into())
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("invalid arguments: {e}"))
}

fn rpc_err(e: anyhow::Error) -> String {
    format!("daemon error: {e}")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The default transcript item count, clamped to a sane range so a caller can't ask for zero
/// items or a pathologically deep (and expensive) history.
fn clamp_transcript_limit(n: Option<usize>) -> usize {
    n.unwrap_or(12).clamp(1, 50)
}

/// The default per-item truncation, clamped so `max_chars` can't be set so low it mangles
/// output or so high it defeats the point of truncating at all.
fn clamp_max_chars(n: Option<usize>) -> usize {
    n.unwrap_or(500).clamp(100, 2000)
}

/// Server-side ceiling on the total transcript payload `read_agent` returns, regardless of how
/// many items/chars-per-item the caller asked for.
const TRANSCRIPT_BUDGET_CHARS: usize = 24_000;

/// Given each (already per-item-truncated) transcript item's char length, oldest first, return
/// how many of the oldest items must be dropped so the remaining total fits within `budget`.
/// Never drops past the end of the slice, even if every item is individually over budget.
fn drop_oldest_for_budget(lens: &[usize], budget: usize) -> usize {
    let mut total: usize = lens.iter().sum();
    let mut drop = 0;
    while total > budget && drop < lens.len() {
        total -= lens[drop];
        drop += 1;
    }
    drop
}

/// The last `max_lines` lines of (already ANSI-stripped) pane text, further capped to
/// `max_chars` — keeping the *end* of the text when it must be trimmed, since a pane capture is
/// read for its most recent state.
fn last_lines(s: &str, max_lines: usize, max_chars: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    let joined = lines[start..].join("\n");
    if joined.chars().count() <= max_chars {
        return joined;
    }
    let skip = joined.chars().count() - max_chars;
    joined.chars().skip(skip).collect()
}

/// Shape a best-effort pane-fetch outcome into the two fields `read_agent` returns: on success,
/// the pane text with no error; on any failure (unresolvable window, capture RPC error), no
/// pane text plus the one-line reason, so the caller still gets the rest of the response.
fn shape_pane(result: Result<String, String>) -> (Option<String>, Option<String>) {
    match result {
        Ok(text) => (Some(text), None),
        Err(e) => (None, Some(e)),
    }
}

// ---- tool catalog (schemas) ----

fn obj(props: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": false,
    })
}

fn tool_catalog() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "fleet_status",
            description: "Primary situational-awareness call: a compact digest of every lane \
                (repo, branch, dirty, and its agent's status + attention + one-line headline). \
                Read this first to orient. attention is one of none/end_of_turn/permission/decision; \
                'needs_you' in counts = waiting agents that want you.",
            input_schema: obj(
                json!({
                    "repo": { "type": "string", "description": "Only this repo's lanes." },
                    "only_attention": { "type": "boolean", "description": "Only lanes whose agent needs you." }
                }),
                &[],
            ),
        },
        ToolDef {
            name: "read_agent",
            description: "Deep-dive one lane: fresh status, attention, the open dialog \
                (pending_prompt), the agent's last message, dirty state, and a capped transcript \
                tail. Use before answering a permission (to see the proposed command) or when an \
                agent is stuck. Defaults are cheap. When debugging a stuck worker, raise \
                transcript_limit/max_chars, or set include_pane to see the live terminal.",
            input_schema: obj(
                json!({
                    "lane_id": { "type": "integer", "description": "The lane to inspect." },
                    "transcript_limit": { "type": "integer", "description": "Transcript items to include, 1-50 (default 12)." },
                    "max_chars": { "type": "integer", "description": "Per-item truncation in chars, 100-2000 (default 500)." },
                    "include_pane": { "type": "boolean", "description": "Best-effort: include the last ~40 lines of the live terminal pane (default false). If the window can't be resolved or captured (e.g. it crashed or closed), the rest of the response is still returned with pane: null and a pane_error explaining why." }
                }),
                &["lane_id"],
            ),
        },
        ToolDef {
            name: "spawn_agent",
            description: "Start a coding agent working on a lane with a concrete task. Counts \
                against the concurrent-agent cap. Omit 'agent' to use the configured default \
                (usually claude-code).",
            input_schema: obj(
                json!({
                    "lane_id": { "type": "integer", "description": "The lane (worktree) to work in." },
                    "agent": { "type": "string", "description": "Agent kind/name, e.g. claude-code or codex. Optional." },
                    "task": { "type": "string", "description": "The task prompt to start the agent with." }
                }),
                &["lane_id"],
            ),
        },
        ToolDef {
            name: "send_to_agent",
            description: "Type an instruction or reply into a running/waiting agent and submit \
                it. Use to steer, answer an end-of-turn agent, or give a relayed human decision.",
            input_schema: obj(
                json!({
                    "lane_id": { "type": "integer" },
                    "text": { "type": "string", "description": "What to send." },
                    "submit": { "type": "boolean", "description": "Press Enter after (default true)." },
                    "window": { "type": "string", "description": "Target a specific agent window in a multi-agent lane (default: the lane's primary session)." }
                }),
                &["lane_id", "text"],
            ),
        },
        ToolDef {
            name: "approve_agent",
            description: "Answer a pending PERMISSION dialog (attention=permission). Default/'yes' \
                accepts; 'no' cancels; a number picks that option. Refuses on a decision-class \
                prompt — those must be escalated to the human. Read the proposed action first if \
                it could be destructive.",
            input_schema: obj(
                json!({
                    "lane_id": { "type": "integer" },
                    "choice": { "description": "\"yes\" (default), \"no\", or an option number." },
                    "window": { "type": "string", "description": "Target a specific agent window in a multi-agent lane (default: the lane's primary session)." }
                }),
                &["lane_id"],
            ),
        },
        ToolDef {
            name: "interrupt_agent",
            description: "Stop what an agent is currently doing: soft (Escape) by default, or \
                hard=true to send Ctrl-C. Use to redirect a misfiring agent.",
            input_schema: obj(
                json!({
                    "lane_id": { "type": "integer" },
                    "hard": { "type": "boolean", "description": "Ctrl-C instead of Escape." }
                }),
                &["lane_id"],
            ),
        },
        ToolDef {
            name: "stop_agent",
            description: "End an agent's session by closing its terminal window. Use for a \
                finished or hung agent. The lane, its worktree files, and the conversation \
                transcript all survive — only the live process ends. If the lane is dirty (see \
                fleet_status/read_agent), mention the uncommitted work when you report.",
            input_schema: obj(
                json!({
                    "lane_id": { "type": "integer" },
                    "window": { "type": "string", "description": "Target a specific agent window in a multi-agent lane (default: the lane's primary session)." }
                }),
                &["lane_id"],
            ),
        },
        ToolDef {
            name: "create_lane",
            description: "Create a new branch + worktree (a lane) in a repo, ready to spawn an \
                agent into. In supervised mode this asks for human confirmation first.",
            input_schema: obj(
                json!({
                    "repo": { "type": "string", "description": "Repo name or id (see list_repos)." },
                    "branch": { "type": "string", "description": "New branch name." },
                    "source_branch": { "type": "string", "description": "Branch to fork from (optional)." }
                }),
                &["repo", "branch"],
            ),
        },
        ToolDef {
            name: "delete_lane",
            description: "Delete a lane (its worktree; with delete_branch also its branch — a \
                force delete). DESTRUCTIVE: requires explicit human approval. First call returns \
                an impact summary and a confirmation token; relay the impact to the human and \
                re-call with confirm=<token> only after they say yes. Refuses dirty worktrees.",
            input_schema: obj(
                json!({
                    "lane_id": { "type": "integer", "description": "The lane to delete." },
                    "delete_branch": { "type": "boolean", "description": "Also force-delete the lane's branch (default false)." },
                    "confirm": { "type": "string", "description": "The token from a prior call without confirm. Omit to get the impact summary + token first." }
                }),
                &["lane_id"],
            ),
        },
        ToolDef {
            name: "merge_lane",
            description: "Merge a lane's branch into the repo's main checkout (a normal, \
                no-force git merge; conflicts abort cleanly and are reported as an error). The \
                main checkout must be clean and already on the target branch. Verify the work \
                first (read_agent, lane_diff) and make sure the worker committed everything. On \
                conflict, stop and tell the human.",
            input_schema: obj(
                json!({
                    "lane_id": { "type": "integer", "description": "The lane whose branch to merge." },
                    "into": { "type": "string", "description": "Target branch the main checkout must already be on (optional)." }
                }),
                &["lane_id"],
            ),
        },
        ToolDef {
            name: "lane_diff",
            description: "See what a lane actually produced: commits ahead of the repo's base \
                branch (with diffstat, via commits/committed_stat), plus uncommitted changes \
                (uncommitted_stat). Use to verify a worker's claim of 'done' before merge_lane, \
                or to check for work worth keeping before stop_agent/delete_lane. Set \
                include_patch for the actual diff text, capped — but that patch covers \
                uncommitted changes only ('git diff HEAD'); committed work is visible via \
                commits/committed_stat, not in the patch text.",
            input_schema: obj(
                json!({
                    "lane_id": { "type": "integer", "description": "The lane to inspect." },
                    "include_patch": { "type": "boolean", "description": "Include the diff text for uncommitted changes only ('git diff HEAD'), capped at max_patch_chars (default false). Committed work is NOT in this text — see commits/committed_stat." },
                    "max_patch_chars": { "type": "integer", "description": "Cap on the patch text in characters, up to 20000 (default 8000)." }
                }),
                &["lane_id"],
            ),
        },
        ToolDef {
            name: "list_repos",
            description: "List the repositories registered with repomon (id, name, path).",
            input_schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "wait_for_change",
            description: "Block until the fleet meaningfully changes (a status/attention edge) or \
                the timeout elapses, then return the deltas. This is how you watch agents without \
                busy-polling: announce you'll watch, call this, report what changed. until: \
                'any' (default) or 'needs_you' to wake only when an agent needs you.",
            input_schema: obj(
                json!({
                    "timeout_secs": { "type": "integer", "description": "Max wait, 1-120 (default 60)." },
                    "until": { "type": "string", "enum": ["any", "needs_you"], "description": "Wake condition (default any)." },
                    "lanes": { "type": "array", "items": { "type": "integer" }, "description": "Only watch these lane ids (optional)." }
                }),
                &[],
            ),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::AgentDigest;

    fn lane(id: i64, repo: &str, status: &str, attention: Attention) -> LaneDigest {
        LaneDigest {
            lane_id: id,
            repo: repo.into(),
            branch: "b".into(),
            dirty: "clean".into(),
            pinned: false,
            agent: Some(AgentDigest {
                kind: "claude".into(),
                status: status.into(),
                attention,
                headline: None,
                idle_secs: 0,
                external: false,
                inferred: false,
                window: Some("lane-1".into()),
                pending_prompt: None,
            }),
            extra_agents: 0,
            active_agents: 1,
        }
    }

    fn agent_sess(external: bool, window: Option<&str>) -> AgentSession {
        AgentSession {
            id: 1,
            agent: repomon_core::model::AgentKind::ClaudeCode,
            repo_id: 1,
            worktree_id: Some(1),
            started_at: Utc::now(),
            last_activity_at: Utc::now(),
            ended_at: None,
            manifest_path: std::path::PathBuf::from("/tmp/x.jsonl"),
            tool_call_count: 0,
            title: None,
            status: repomon_core::model::AgentStatus::Waiting,
            external,
            session_id: None,
            resume_at: None,
            inferred: false,
            tmux_window: window.map(str::to_string),
            last_message: None,
            pending_prompt: None,
            config_dir: None,
            custom_label: None,
        }
    }

    #[test]
    fn target_window_picks_primary_and_refuses_unmanaged() {
        // An explicit override always wins.
        assert_eq!(
            target_window(None, Some("lane-2-3".into())).unwrap(),
            "lane-2-3"
        );
        // Otherwise default to the primary's window.
        let managed = agent_sess(false, Some("lane-7-2"));
        assert_eq!(target_window(Some(&managed), None).unwrap(), "lane-7-2");
        // Refuse an external session (do not auto-act on the user's own claude).
        assert!(target_window(Some(&agent_sess(true, Some("lane-7"))), None).is_err());
        // Refuse a windowless session, and a lane with no session at all.
        assert!(target_window(Some(&agent_sess(false, None)), None).is_err());
        assert!(target_window(None, None).is_err());
    }

    #[test]
    fn diff_detects_a_needs_you_transition() {
        let base = vec![lane(1, "r", "running", Attention::None)];
        let cur = vec![lane(1, "r", "waiting", Attention::Permission)];
        let d = diff(&base, &cur, None);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].from, "running");
        assert_eq!(d[0].to, "waiting");
        assert_eq!(d[0].attention, Attention::Permission);
        assert!(d[0].needs_you);
    }

    #[test]
    fn diff_reports_added_and_removed_lanes() {
        let base = vec![lane(1, "r", "running", Attention::None)];
        let cur = vec![lane(2, "r", "running", Attention::None)];
        let d = diff(&base, &cur, None);
        assert!(d.iter().any(|x| x.lane_id == 2 && x.from == "absent"));
        assert!(d.iter().any(|x| x.lane_id == 1 && x.to == "gone"));
    }

    #[test]
    fn diff_honors_the_lane_filter() {
        let base = vec![lane(1, "r", "running", Attention::None)];
        let cur = vec![lane(1, "r", "waiting", Attention::Decision)];
        assert!(diff(&base, &cur, Some(&[2])).is_empty());
        assert_eq!(diff(&base, &cur, Some(&[1])).len(), 1);
    }

    #[test]
    fn approve_key_decoding() {
        assert_eq!(approve_key(None).unwrap().0, "Enter");
        assert_eq!(approve_key(Some(&json!("yes"))).unwrap().0, "Enter");
        assert_eq!(approve_key(Some(&json!("no"))).unwrap().0, "Escape");
        assert_eq!(approve_key(Some(&json!(2))).unwrap().0, "2");
        assert_eq!(approve_key(Some(&json!("3"))).unwrap().0, "3");
        assert!(approve_key(Some(&json!("maybe"))).is_err());
    }

    #[test]
    fn tally_counts_by_attention() {
        let lanes = vec![
            lane(1, "r", "running", Attention::None),
            lane(2, "r", "waiting", Attention::Permission),
            lane(3, "r", "waiting", Attention::Decision),
        ];
        let t = tally(&lanes);
        assert_eq!(t["running"], 1);
        assert_eq!(t["needs_you"], 2);
        assert_eq!(t["permission"], 1);
        assert_eq!(t["decision"], 1);
    }

    #[test]
    fn transcript_limit_clamps_to_1_and_50() {
        assert_eq!(clamp_transcript_limit(None), 12); // default
        assert_eq!(clamp_transcript_limit(Some(0)), 1);
        assert_eq!(clamp_transcript_limit(Some(999)), 50);
        assert_eq!(clamp_transcript_limit(Some(30)), 30);
    }

    #[test]
    fn max_chars_clamps_to_100_and_2000() {
        assert_eq!(clamp_max_chars(None), 500); // default
        assert_eq!(clamp_max_chars(Some(1)), 100);
        assert_eq!(clamp_max_chars(Some(50_000)), 2000);
        assert_eq!(clamp_max_chars(Some(750)), 750);
    }

    #[test]
    fn budget_drop_keeps_everything_under_budget() {
        assert_eq!(drop_oldest_for_budget(&[100, 200, 300], 24_000), 0);
        assert_eq!(drop_oldest_for_budget(&[], 24_000), 0);
    }

    #[test]
    fn budget_drop_removes_oldest_items_first() {
        // Three 10_000-char items (oldest first) exceed a 24_000 budget; the oldest one item
        // must go, leaving the two newest (20_000 <= 24_000).
        let lens = [10_000, 10_000, 10_000];
        assert_eq!(drop_oldest_for_budget(&lens, 24_000), 1);
    }

    #[test]
    fn budget_drop_never_underflows_past_the_last_item() {
        // Even if every item is oversized, we stop once nothing is left to drop rather than
        // underflowing the running total.
        let lens = [50_000, 50_000];
        assert_eq!(drop_oldest_for_budget(&lens, 24_000), 2);
    }

    #[test]
    fn last_lines_keeps_only_the_tail() {
        let s = "1\n2\n3\n4\n5";
        assert_eq!(last_lines(s, 3, 100), "3\n4\n5");
        // Fewer lines than requested: keep them all.
        assert_eq!(last_lines(s, 100, 100), s);
    }

    #[test]
    fn last_lines_caps_chars_keeping_the_most_recent_tail() {
        let s = "aaaa\nbbbb\ncccc";
        let capped = last_lines(s, 10, 5);
        assert_eq!(capped.chars().count(), 5);
        assert!(capped.ends_with("cccc"));
    }

    #[test]
    fn shape_pane_returns_text_with_no_error_on_success() {
        let (pane, pane_error) = shape_pane(Ok("last 40 lines".into()));
        assert_eq!(pane, Some("last 40 lines".into()));
        assert_eq!(pane_error, None);
    }

    #[test]
    fn shape_pane_degrades_to_null_pane_and_a_reason_on_failure() {
        // The crash-debugging case this finding is about: the window is gone, but the caller
        // should still get pane: null + why, not lose the whole read_agent response over it.
        let (pane, pane_error) = shape_pane(Err(
            "the lane's active session has no tmux window to target".into(),
        ));
        assert_eq!(pane, None);
        assert_eq!(
            pane_error,
            Some("the lane's active session has no tmux window to target".into())
        );
    }
}
