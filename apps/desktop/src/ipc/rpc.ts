import { Channel, invoke } from "@tauri-apps/api/core";

import type {
  AccountUsage,
  AgentChoice,
  ApprovalRule,
  BrowseResult,
  Commit,
  CommitShow,
  DialogClass,
  ExtSnapshot,
  FanoutSummary,
  FileCreateResult,
  FileDeleteResult,
  FileDiffBaseResult,
  FileIndexResult,
  FileListResult,
  FileReadRawResult,
  FileReadResult,
  FileRenameResult,
  FileSearchResult,
  FileWriteResult,
  FleetMessage,
  JournalEntry,
  Lane,
  MessagePage,
  ModelRateRow,
  PendingDialog,
  Playbook,
  PolicyAction,
  RatesStatus,
  Repo,
  RepomindStatus,
  Schedule,
  SupervisionConfig,
  SupervisionEntry,
  SupervisionOverrides,
  SupervisionPolicy,
  SystemDoctorResult,
  TimelineData,
  TranscriptItem,
  UsageBucket,
  UsageFinding,
  UsageGroupBy,
  UsageRange,
  UsageSessionRow,
  UsageStatus,
  UsageRefreshResult,
  UsageSummary,
  UsageTimeline,
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

/** The window every ledger read takes: a named range, or `custom` with explicit RFC 3339 bounds. */
export interface UsageWindowParams {
  range: UsageRange;
  since?: string | null;
  until?: string | null;
}

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
  message_hop_refresh_senders?: string[];
  notify_show_why: boolean;
  notify_coalesce: boolean;
  notify_click_focus: boolean;
  notify_desktop_fallback: boolean;
  notify_subagents: boolean;
  usage_probe: boolean;
  expand_agents: boolean;
  sort_repos_by_activity: boolean;
  /** "default" | "activity" | "manual" - resolved daemon-side from the setting + legacy boolean. */
  sort_mode?: string;
  /** "activity" | "manual" - how the per-lane agent tabs are ordered. */
  tab_sort_mode?: string;
  /** Whether the companion-app WebSocket bridge is enabled at daemon startup. */
  remote_enabled?: boolean;
  /** Configured WebSocket bind address for the companion-app bridge. */
  remote_bind?: string | null;
  /** Masked legacy shared token; the raw secret never crosses the RPC boundary. */
  remote_token_masked?: string | null;
  embedded_pty: boolean;
  orchestrator_agent?: string | null;
  orchestrator_model?: string | null;
  agent_icons?: Record<string, string>;
  supervision: SupervisionConfig;
  /** The `[repomind]` table: the home repo and its controller lane. */
  repomind?: RepomindConfigView;
  /** Whether the daemon ingests agent transcripts into the usage ledger at all. */
  usage_enabled: boolean;
  /** Whether the daily LiteLLM price refresh is on ([usage] refresh_prices). */
  usage_refresh_prices: boolean;
  [key: string]: unknown;
}

/** Carries only edited price fields so omitted values neither replace nor clear existing overrides. */
export interface UsagePriceOverrideUpsert {
  model: string;
  input_per_mtok?: number;
  output_per_mtok?: number;
  cache_read_per_mtok?: number;
  cache_write_per_mtok?: number;
}

/** `config.get`'s `repomind` block. `home` is the raw setting, `home_path` its expanded form. */
export interface RepomindConfigView {
  home: string;
  home_path: string;
  primary_agent?: string | null;
  max_controllers: number;
}

export interface RemoteDeviceSummary {
  name: string;
  role: string;
  created_at: string;
  last_seen_at: string | null;
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

/** Mirrors lane.diff, whose commit list is raw git log --oneline text rather than structured rows. */
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
  "repo.rename": { params: { repo_id: number; label: string }; result: Repo };
  "repo.reorder": { params: { ordered_ids: number[] }; result: Repo[] };
  "agent.set_tab_order": {
    params: { lane_id: number; ordered_ids: string[] };
    result: null;
  };
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
  // The other half of the approval gate: moves the draft into `playbooks/rejected/` with
  // `status: rejected`. Never deletes, so a rejected idea stays readable in the home's history.
  "playbook.reject": {
    params: { name: string };
    result: { name: string; path: string; status: string };
  };
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
  // Worktree file operations are local-only.
  "file.list": { params: { lane_id: number; path?: string }; result: FileListResult };
  "file.read": { params: { lane_id: number; path: string }; result: FileReadResult };
  "file.read_raw": { params: { lane_id: number; path: string }; result: FileReadRawResult };
  // `expected_mtime_ms` omitted = last-write-wins; given and stale, the daemon rejects with
  // DaemonRpcError.code === -32011 ("conflict: file changed on disk") and
  // `data: { expected_mtime_ms, actual_mtime_ms }` (actual is null if the file was deleted).
  "file.write": {
    params: { lane_id: number; path: string; content: string; expected_mtime_ms?: number };
    result: FileWriteResult;
  };
  "file.index": {
    params: { lane_id: number };
    result: FileIndexResult;
  };
  "file.create": {
    params: { lane_id: number; path: string; is_dir?: boolean };
    result: FileCreateResult;
  };
  "file.rename": {
    params: { lane_id: number; from: string; to: string };
    result: FileRenameResult;
  };
  "file.delete": {
    params: { lane_id: number; path: string; recursive?: boolean };
    result: FileDeleteResult;
  };
  "file.search": {
    params: {
      lane_id: number;
      query: string;
      regex?: boolean;
      case_sensitive?: boolean;
      glob?: string;
      max_results?: number;
    };
    result: FileSearchResult;
  };
  "file.diff_base": {
    params: { lane_id: number; path: string };
    result: FileDiffBaseResult;
  };
  // A single plain destination returns FleetMessage; lists and wildcards return per-recipient
  // fan-out results.
  "message.send": {
    params: { to: string | string[]; body: string; reply_to?: string };
    result: FleetMessage | { recipient_count: number; sent_count: number; results: Array<{ to: string; status: "sent" | "no_such_session" | "delivery_error"; message_id?: string; thread_id?: string; error?: string }> };
  };
  "message.inbox": { params: { unread_only?: boolean; limit?: number; before?: string }; result: MessagePage };
  "message.mark_read": { params: { id: string }; result: FleetMessage };
  "message.list": { params: { lane_id?: number; unread_only?: boolean; limit?: number; before?: string }; result: MessagePage };
  "message.force_send": { params: { id: string }; result: FleetMessage };
  "message.delete": { params: { id: string }; result: null };
  "agent.detect": { params: undefined; result: AgentChoice[] };
  "agent.add": { params: { name: string; command: string }; result: null };
  "agent.remove": { params: { name: string }; result: null };
  "agent.set_default": { params: { name: string | null }; result: null };
  "agent.spawn": { params: { lane_id: number; agent: string; task?: string }; result: { lane_id: number; window: string; spawn_warnings?: string[] } };
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
  "session.rename": { params: { session_id: string; fallback_session_id?: string; label?: string }; result: null };
  "terminal.open": { params: { lane_id: number }; result: { id: string; target: string } };
  "terminal.list": { params: { lane_id: number }; result: string[] };
  "terminal.list_all": { params: undefined; result: Array<{ lane_id: number; id: string }> };
  "terminal.close": { params: { id: string }; result: null };
  "fs.browse": { params: { path?: string }; result: BrowseResult };
  "viewport.set": { params: { lane_ids: number[]; focus_lane?: number; focus_window?: string; fit_windows?: string[]; windows?: string[] }; result: null };
  "commit.recent": { params: { lane_id?: number; repo_id?: number; limit?: number }; result: Commit[] };
  "commit.search": { params: { query: string; limit?: number }; result: Commit[] };
  // Local-only (see remote.rs's remote_method_allowed) - same reasoning as the worktree
  // file-editor RPCs above: a caller-chosen oid can walk the entire repo history one commit at a
  // time, broader than lane.diff's current-diff-only scope.
  "commit.show": { params: { lane_id: number; oid: string; max_patch_chars?: number }; result: CommitShow };
  timeline: { params: { from_iso: string; to_iso: string; bucket_secs: number }; result: TimelineData };
  sessions: { params: { from_iso: string; to_iso: string }; result: WorkSession[] };
  "config.get": { params: undefined; result: ConfigView };
  "config.set": {
    params: Partial<ConfigView> & {
      /** Settings > Usage's inline editor / `repomon usage rates set`. */
      usage_price_override_upsert?: UsagePriceOverrideUpsert;
      /** Settings > Usage's Reset action / `repomon usage rates reset`: a model id to drop. */
      usage_price_override_reset?: string;
    };
    result: ConfigView;
  };
  "remote.pair": {
    params: { name: string };
    result: { name: string; token: string; url: string };
  };
  "remote.devices": { params: undefined; result: RemoteDeviceSummary[] };
  "remote.revoke": { params: { name: string }; result: { revoked: boolean } };
  "system.doctor": { params: undefined; result: SystemDoctorResult };
  "usage.get": { params: undefined; result: AccountUsage[] };
  "usage.refresh": { params: undefined; result: UsageRefreshResult };
  "usage.summary": { params: UsageWindowParams & { group_by: UsageGroupBy }; result: UsageSummary };
  "usage.timeline": {
    params: UsageWindowParams & { group_by: UsageGroupBy; bucket: UsageBucket };
    result: UsageTimeline;
  };
  "usage.sessions": {
    params: UsageWindowParams & { lane_id?: number | null; limit?: number };
    result: UsageSessionRow[];
  };
  "usage.findings": { params: UsageWindowParams; result: UsageFinding[] };
  "usage.status": { params: undefined; result: UsageStatus };
  "usage.rates": { params: undefined; result: RatesStatus };
  "usage.refresh_rates": { params: undefined; result: RatesStatus };
  "usage.models": { params: undefined; result: ModelRateRow[] };
  "usage.export": {
    params: UsageWindowParams & { format: "csv" | "json" };
    result: { path: string; events: number; bytes: number };
  };
  "usage.ingest_now": {
    params: undefined;
    result: {
      listed: number;
      scanned: number;
      events: number;
      failed: number;
      redigested: number;
    };
  };
  "orchestrator.status": { params: undefined; result: OrchestratorStatus };
  "orchestrator.transcript": { params: { limit?: number }; result: TranscriptItem[] };
  "orchestrator.start": { params: { agent?: string; model?: string }; result: OrchestratorStatus };
  "orchestrator.stop": { params: undefined; result: null };
  "orchestrator.send_input": { params: { text: string; enter?: boolean }; result: null };
  "orchestrator.key": { params: { key: string; literal?: boolean }; result: null };
  "orchestrator.watch": { params: { on: boolean }; result: null };
  "orchestrator.resize": { params: { cols: number; rows: number }; result: null };
  // The repomind home. `status` is read-only and remote-allowed; `boot` and `export` write files
  // in the home and are local-only (see the daemon's `remote_method_allowed`).
  "repomind.status": { params: undefined; result: RepomindStatus };
  "repomind.boot": {
    params: undefined;
    result: { path: string; bytes: number; tokens_estimate: number; trimmed: string[] };
  };
  "repomind.export": { params: undefined; result: { files: string[]; kinds: string[] } };
  // Local-only: types one instruction into the primary controller's composer through the
  // daemon's verified injection. `outcome` is "sent", "skipped" (the composer was busy, `reason`
  // says which) or "failed"; it rejects outright when no controller is running.
  "repomind.instruct": {
    params: { text: string };
    result: { outcome: string; window: string; entry_id?: number | null; reason?: string };
  };
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
  "supervision.get": {
    params: { lane_id?: number };
    result: {
      defaults: SupervisionConfig;
      lane: SupervisionOverrides | null;
      effective: SupervisionPolicy | null;
    };
  };
  "supervision.set": {
    params: {
      lane_id: number;
      enabled?: boolean;
      classes?: Partial<Record<DialogClass, PolicyAction>>;
      nudge_text?: string;
      stall_mins?: number;
      nudge_retries?: number;
      expect_work?: boolean;
    };
    result: { effective: SupervisionPolicy };
  };
  "supervision.audit": {
    params: { lane_id?: number; limit?: number; before_id?: number };
    result: { entries: SupervisionEntry[] };
  };
  "supervision.status": {
    params: undefined;
    result: {
      master: boolean;
      lanes: Array<{ lane_id: number; enabled: boolean; last: SupervisionEntry | null }>;
    };
  };
  "supervision.nudge": {
    params: { lane_id: number; window?: string; text?: string };
    result: { outcome: string; entry_id: number; keys: string[] | null };
  };
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
