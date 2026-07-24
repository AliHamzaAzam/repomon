import { Channel, invoke } from "@tauri-apps/api/core";

import type {
  AccountUsage,
  AgentChoice,
  BrowseResult,
  Commit,
  ExtSnapshot,
  FanoutSummary,
  Lane,
  PendingDialog,
  Repo,
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

export type ExtScopeParams = { scope: "global" } | { scope: "repo"; repo_id: number };

export interface ConfigView {
  accent?: string | null;
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
  notify_show_why: boolean;
  notify_coalesce: boolean;
  notify_click_focus: boolean;
  notify_subagents: boolean;
  usage_probe: boolean;
  expand_agents: boolean;
  embedded_pty: boolean;
  orchestrator_agent?: string | null;
  orchestrator_model?: string | null;
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

interface RpcMap {
  "repo.list": { params: undefined; result: Repo[] };
  "repo.add": { params: { path: string }; result: Repo };
  "repo.remove": { params: { repo_id: number }; result: null };
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
  "lane.diff": { params: { lane_id: number; include_patch?: boolean }; result: unknown };
  "agent.detect": { params: undefined; result: AgentChoice[] };
  "agent.spawn": { params: { lane_id: number; agent: string; task?: string }; result: { lane_id: number; window: string } };
  "agent.adopt": { params: { lane_id: number; session_id?: string }; result: { lane_id: number; window: string } };
  "agent.stop": { params: { lane_id: number; window?: string }; result: null };
  "agent.capture": { params: { lane_id: number; window?: string; lines?: number }; result: { content: string } };
  "agent.prompt": { params: { lane_id: number; window?: string }; result: { dialog: PendingDialog | null } };
  "agent.answer": { params: { lane_id: number; window?: string; choice: number; expect_summary?: string }; result: null };
  "agent.pin": { params: { lane_id: number; pinned: boolean }; result: null };
  "agent.auto_continue": { params: { lane_id: number; enabled: boolean }; result: null };
  "agent.send_input": { params: { lane_id: number; window?: string; text: string; enter?: boolean }; result: null };
  "agent.key": { params: { lane_id: number; window?: string; key: string; literal?: boolean }; result: null };
  "agent.scroll": {
    params: { lane_id: number; window?: string; up: boolean; ticks: number };
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
  "plugin.update": { params: { id?: string }; result: { ok: boolean; stdout: string } };
  "plugin.details": { params: { id: string }; result: { text: string } };
  "marketplace.add": { params: { source: string }; result: { ok: boolean; stdout: string } };
  "marketplace.remove": { params: { name: string }; result: { ok: boolean; stdout: string } };
  "marketplace.refresh": { params: { name?: string }; result: { ok: boolean; stdout: string } };
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
