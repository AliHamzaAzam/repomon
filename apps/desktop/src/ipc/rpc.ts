import { Channel, invoke } from "@tauri-apps/api/core";

import type {
  AccountUsage,
  ApprovalRule,
  AgentChoice,
  BrowseResult,
  Commit,
  ExtSnapshot,
  FanoutSummary,
  JournalEntry,
  Lane,
  FleetMessage,
  MessagePage,
  Playbook,
  Schedule,
  PendingDialog,
  Repo,
  SystemDoctorResult,
  TimelineData,
  TranscriptItem,
  WorkSession,
} from "../bindings";

export interface RpcFailure {
  code: number;
  message: string;
  data: unknown | null;
}

export class DaemonRpcError extends Error implements RpcFailure {
  readonly code: number;
  readonly data: unknown | null;

  constructor(error: RpcFailure) {
    super(error.message);
    this.name = "DaemonRpcError";
    this.code = error.code;
    this.data = error.data;
  }
}

export interface DaemonEvent<T = unknown> {
  jsonrpc: "2.0";
  method: `event.${string}`;
  params: T;
}

export type ExtScope = { scope: "global" } | { scope: "repo"; repo_id: number };
/** Ext RPC params: a scope plus the Claude account (config dir) to target. Omitted = "default" (~/.claude). */
export type ExtScopeParams = ExtScope & { account?: string };

export interface ConfigView {
  accent?: string | null;
  theme?: string | null;
  worktree_template: string;
  default_agent?: string | null;
  auto_continue: boolean;
  auto_continue_message: string;
  spawn_prompt: boolean;
  notify_enabled: boolean;
  notify_needs_you: boolean;
  notify_rate_limited: boolean;
  notify_resumed: boolean;
  notify_idle: boolean;
  notify_sound: boolean;
  notify_sound_volume: number;
  notify_sound_unfocused_only: boolean;
  notify_sound_agent_needs_you: boolean;
  notify_sound_agent_finished: boolean;
  notify_sound_repomind_needs_you: boolean;
  notify_sound_error_or_stall: boolean;
  notify_sound_incoming_message: boolean;
  notify_sound_update_ready: boolean;
  message_inject_agents?: boolean;
  message_inject_operator?: boolean;
  notify_show_why: boolean;
  notify_coalesce: boolean;
  notify_click_focus: boolean;
  notify_desktop_fallback: boolean;
  notify_subagents: boolean;
  usage_probe: boolean;
  expand_agents: boolean;
  sort_repos_by_activity: boolean;
  embedded_pty: boolean;
  orchestrator_agent?: string | null;
  orchestrator_model?: string | null;
  agent_icons?: Record<string, string>;
  [key: string]: unknown;
}

export interface OrchestratorStatus {
  running: boolean;
  agent?: string | null;
  model?: string | null;
  backend?: string | null;
  window?: string | null;
  attention?: string | null;
  headline?: string | null;
}

/**
 * `lane.diff`'s result: a lane's branch compared against the repo's base branch, plus its own
 * uncommitted state. Mirrors `LaneDiff` in crates/repomon-core/src/git/diff.rs — `commits` is the
 * raw `git log --oneline <merge_base>..HEAD` text (newline-separated "oid summary" lines), not a
 * structured array, so callers split it themselves (see GitExplorerPanel's `parseCommits`).
 */
export interface LaneDiff {
  base: string;
  merge_base: string;
  commits: string;
  commits_truncated?: boolean;
  committed_stat: string;
  uncommitted_stat: string;
  untracked: number;
  patch?: string;
  patch_truncated?: boolean;
}

interface RpcMap {
  "repo.list": { params: undefined; result: Repo[] };
  "repo.add": { params: { path: string }; result: Repo };
  "repo.remove": { params: { repo_id: number }; result: null };
  "repo.set_hidden": { params: { repo_id: number; hidden: boolean }; result: null };
  "approval.record": {
    params: { repo: string; command: string; verdict: string };
    result: { pattern: string | null; approvals: number; rule_exists: boolean; propose: boolean };
  };
  "approval.allow": { params: { repo: string; pattern: string }; result: null };
  "approval.remove": { params: { repo: string; pattern: string }; result: null };
  "approval.list": { params: undefined; result: { rules: ApprovalRule[] } };
  "schedule.add": {
    params: { spec: string; prompt: string; max_actions?: number };
    result: Schedule & { next_run?: string };
  };
  "schedule.list": { params: undefined; result: { schedules: Array<Schedule & { next_run?: string }> } };
  "schedule.remove": { params: { id: number }; result: null };
  "playbook.save": { params: { name: string; content: string }; result: Playbook };
  "playbook.search": { params: { query: string; limit?: number }; result: { playbooks: Playbook[] } };
  "playbook.list": { params: undefined; result: { playbooks: Playbook[] } };
  "playbook.approve": { params: { name: string }; result: Playbook };
  "playbook.delete": { params: { name: string }; result: null };
  "journal.append": {
    params: {
      session: string;
      action: string;
      lane_id?: number | null;
      repo?: string | null;
      params?: string | null;
      outcome?: string;
      detail?: string | null;
    };
    result: { id: number };
  };
  "journal.query": {
    params: { query?: string; since_last_session?: boolean; limit?: number };
    result: { entries: JournalEntry[] };
  };
  "repo.notes.get": {
    params: { repo_id: number };
    result: { repo_id: number; name: string; exists: boolean; content: string; path: string };
  };
  "repo.notes.set": {
    params: { repo_id: number; content: string };
    result: { repo_id: number; bytes: number; path: string };
  };
  "repo.discover": { params: { root: string; max_depth?: number }; result: string[] };
  "lane.list": { params: undefined; result: Lane[] };
  "lane.create": {
    params: {
      repo_id: number;
      branch: string;
      source_branch?: string;
      path?: string;
      copy_files?: string[];
    };
    result: Lane;
  };
  "lane.delete": { params: { lane_id: number; also_delete_branch?: boolean }; result: null };
  "lane.focus": { params: { lane_id: number }; result: { path: string } };
  "lane.merge": { params: { lane_id: number; into?: string }; result: { message: string } };
  "lane.diff": { params: { lane_id: number; include_patch?: boolean }; result: LaneDiff };
  // `to` also accepts a list of addresses, "lane-2/*", or "*" (A6 broadcast/multi-recipient
  // mail). A single plain address still returns a bare `FleetMessage`; anything else returns a
  // fan-out summary instead (`{ recipient_count, sent_count, results: { to, status, ... }[] }`).
  "message.send": {
    params: { to: string | string[]; body: string; reply_to?: string };
    result: FleetMessage | { recipient_count: number; sent_count: number; results: Array<{ to: string; status: "sent" | "no_such_session" | "delivery_error"; message_id?: string; thread_id?: string; error?: string }> };
  };
  "message.inbox": { params: { unread_only?: boolean; limit?: number; before?: string }; result: MessagePage };
  "message.mark_read": { params: { id: string }; result: FleetMessage };
  "message.list": { params: { lane_id?: number; unread_only?: boolean; limit?: number; before?: string }; result: MessagePage };
  "agent.detect": { params: undefined; result: AgentChoice[] };
  "agent.add": { params: { name: string; command: string }; result: null };
  "agent.remove": { params: { name: string }; result: null };
  "agent.set_default": { params: { name: string | null }; result: null };
  "agent.spawn": { params: { lane_id: number; agent: string; task?: string }; result: { lane_id: number; window: string } };
  "agent.adopt": { params: { lane_id: number; session_id?: string; agent?: string }; result: { lane_id: number; window: string } };
  "agent.stop": { params: { lane_id: number; window?: string }; result: null };
  "agent.capture": { params: { lane_id: number; window?: string; lines?: number }; result: { content: string } };
  "agent.transcript_page": {
    params: { lane_id: number; session_id?: string; before?: number };
    result: { items: TranscriptItem[]; next_before: number | null };
  };
  "agent.prompt": { params: { lane_id: number; window?: string }; result: { dialog: PendingDialog | null } };
  "agent.answer": { params: { lane_id: number; window?: string; choice: number; expect_summary?: string }; result: null };
  "agent.pin": { params: { lane_id: number; pinned: boolean }; result: null };
  "agent.auto_continue": { params: { lane_id: number; enabled: boolean }; result: null };
  "agent.send_input": { params: { lane_id: number; window?: string; text: string; enter?: boolean }; result: null };
  "agent.key": { params: { lane_id: number; window?: string; key: string; literal?: boolean }; result: null };
  "agent.scroll": {
    params: { lane_id: number; window?: string; up: boolean; ticks: number; col: number; row: number };
    result: { forwarded: boolean };
  };
  "agent.resize": { params: { lane_id: number; window?: string; cols: number; rows: number }; result: null };
  "agent.fit": {
    params: { lane_id: number; window?: string; cols: number; rows: number };
    result: { applied: boolean; cols: number | null; rows: number | null };
  };
  "session.rename": { params: { session_id: string; label?: string }; result: null };
  "terminal.open": { params: { lane_id: number }; result: { id: string; target: string } };
  "terminal.list": { params: { lane_id: number }; result: string[] };
  "terminal.list_all": { params: undefined; result: Array<{ lane_id: number; id: string }> };
  "terminal.close": { params: { id: string }; result: null };
  "fs.browse": { params: { path?: string }; result: BrowseResult };
  "viewport.set": { params: { lane_ids: number[]; focus_lane?: number; focus_window?: string; windows?: string[] }; result: null };
  "commit.recent": { params: { lane_id?: number; repo_id?: number; limit?: number }; result: Commit[] };
  "commit.search": { params: { query: string; limit?: number }; result: Commit[] };
  timeline: { params: { from_iso: string; to_iso: string; bucket_secs: number }; result: TimelineData };
  sessions: { params: { from_iso: string; to_iso: string }; result: WorkSession[] };
  "config.get": { params: undefined; result: ConfigView };
  "config.set": { params: Partial<ConfigView>; result: ConfigView };
  "system.doctor": { params: undefined; result: SystemDoctorResult };
  "usage.get": { params: undefined; result: AccountUsage[] };
  "orchestrator.status": { params: undefined; result: OrchestratorStatus };
  "orchestrator.transcript": { params: { limit?: number }; result: TranscriptItem[] };
  "orchestrator.start": { params: { agent?: string; model?: string }; result: OrchestratorStatus };
  "orchestrator.stop": { params: undefined; result: null };
  "orchestrator.send_input": { params: { text: string; enter?: boolean }; result: null };
  "orchestrator.key": { params: { key: string; literal?: boolean }; result: null };
  "orchestrator.watch": { params: { on: boolean }; result: null };
  "orchestrator.resize": { params: { cols: number; rows: number }; result: null };
  "ext.list": { params: ExtScopeParams; result: ExtSnapshot };
  "plugin.enable": { params: { id: string } & ExtScopeParams; result: { ok: boolean; fanout: FanoutSummary | null } };
  "plugin.disable": { params: { id: string } & ExtScopeParams; result: { ok: boolean; fanout: FanoutSummary | null } };
  "plugin.install": { params: { ref: string } & ExtScopeParams; result: { ok: boolean; stdout: string; fanout: FanoutSummary | null } };
  "plugin.remove": { params: { id: string } & ExtScopeParams; result: { ok: boolean; stdout: string } };
  "plugin.update": { params: { id?: string; account?: string }; result: { ok: boolean; stdout: string } };
  "plugin.details": { params: { id: string; account?: string }; result: { text: string } };
  "marketplace.add": { params: { source: string; account?: string }; result: { ok: boolean; stdout: string } };
  "marketplace.remove": { params: { name: string; account?: string }; result: { ok: boolean; stdout: string } };
  "marketplace.refresh": { params: { name?: string; account?: string }; result: { ok: boolean; stdout: string } };
  "skill.create": { params: { name: string; description?: string } & ExtScopeParams; result: { path: string } };
  "skill.read": { params: { path: string }; result: { content: string } };
  "skill.write": { params: { path: string; content: string }; result: { ok: boolean; fanout: FanoutSummary | null } };
  "skill.delete": { params: { name: string } & ExtScopeParams; result: { ok: boolean; fanout: FanoutSummary | null } };
}

export type RpcMethod = keyof RpcMap;
export type RpcParams<M extends RpcMethod> = RpcMap[M]["params"];
export type RpcResult<M extends RpcMethod> = RpcMap[M]["result"];

export function isRpcFailure(value: unknown): value is RpcFailure {
  return typeof value === "object" && value !== null && "code" in value && "message" in value;
}

export async function daemonCall<M extends RpcMethod>(
  method: M,
  ...args: RpcParams<M> extends undefined ? [] | [undefined] : [RpcParams<M>]
): Promise<RpcResult<M>> {
  try {
    return await invoke<RpcResult<M>>("daemon_call", {
      method,
      params: args[0] ?? null,
    });
  } catch (error) {
    if (isRpcFailure(error)) throw new DaemonRpcError(error);
    throw error;
  }
}

export async function subscribeDaemon(
  onEvent: (event: DaemonEvent) => void,
): Promise<() => void> {
  const channel = new Channel<DaemonEvent>();
  let active = true;
  channel.onmessage = (event) => {
    if (active) onEvent(event);
  };
  await invoke("daemon_subscribe", { onEvent: channel });
  return () => {
    active = false;
  };
}
