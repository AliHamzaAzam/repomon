/// Dev-only screenshot fixture, aliased in for `vite.screenshot.config.ts` over the real
/// `@tauri-apps/api/core`. Never imported by the app's own entry points, `tauri dev`, `vite
/// build`, or any test.
///
/// The real IPC seam turned out to be `@tauri-apps/api/core`'s `invoke`, not `ipc/rpc.ts`
/// alone: `ipc/rpc.ts`'s `daemonCall` calls `invoke("daemon_call", ...)`, but
/// `ipc/connection.ts`'s connection probe, `ipc/boot.ts`, `ipc/assets.ts`, `ipc/cli.ts`,
/// `ipc/daemonControl.ts`, `ipc/updater.ts` and `ipc/term.ts` all call `invoke` directly for
/// their own commands. Shimming `invoke` itself, one level lower, covers every one of those
/// without touching each file, and lets the real `daemonCall`/`getConnectionStatus`/etc. run
/// unmodified against fixture data.
export { Channel, convertFileSrc } from "@tauri-apps/api/core";

function repo(id: number, name: string, label: string | null = null) {
  return {
    id,
    path: `/code/${name}`,
    name,
    added_at: "2026-07-20T00:00:00Z",
    worktree_root_template: null,
    hidden: false,
    position: null,
    label,
  };
}

function agentSession(overrides: Record<string, unknown>) {
  return {
    id: 1,
    agent: "claude-code",
    repo_id: 1,
    worktree_id: 1,
    started_at: "2026-09-10T00:00:00Z",
    last_activity_at: "2026-09-10T00:00:00Z",
    ended_at: null,
    manifest_path: "",
    tool_call_count: 4,
    title: null,
    status: "running",
    external: false,
    session_id: "s1",
    resume_at: null,
    inferred: false,
    tmux_window: "lane-1",
    last_message: null,
    pending_prompt: null,
    pending_dialog: null,
    stale: false,
    stalled_since: null,
    subagent_running: null,
    gate: null,
    config_dir: null,
    custom_label: null,
    generated_label: null,
    status_reason: null,
    ...overrides,
  };
}

function lane(overrides: Record<string, unknown>) {
  const id = overrides.id as number;
  const repoRef = overrides.repo as ReturnType<typeof repo>;
  const branch = (overrides.branch as string) ?? `lane-${id}`;
  return {
    id,
    repo: repoRef,
    worktree: {
      id,
      repo_id: repoRef.id,
      path: `${repoRef.path}-wt/${branch}`,
      branch,
      head: "abc1234",
      is_main: false,
      name: branch,
    },
    state: {
      worktree_id: id,
      head: "abc1234",
      branch,
      upstream: null,
      ahead: 0,
      behind: 0,
      dirty: { staged: 0, unstaged: 0, untracked: 0 },
      last_commit_at: null,
      locked: false,
      prunable: false,
      last_change_at: null,
    },
    agent_sessions: overrides.agent_sessions ?? [],
    last_activity_at: overrides.last_activity_at,
    pinned: false,
    role: null,
  };
}

const ago = (minutes: number) => new Date(Date.now() - minutes * 60_000).toISOString();

const REPOS = [repo(1, "deneblondon"), repo(2, "SAAS"), repo(3, "portfolio"), repo(4, "Mira")];

const HEADLINES: Record<number, string> = {
  10: "Add collections to charms page",
  11: "Assess developer rate for POS system work",
  12: "Fix flaky login test",
  13: "Portfolio redesign with 3D globe",
  14: "Audit documentation",
};

const LANES = [
  lane({
    id: 10,
    repo: REPOS[0],
    branch: "codex/charms-collections",
    last_activity_at: ago(4),
    agent_sessions: [
      agentSession({
        id: 101,
        agent: "codex",
        status: "waiting",
        status_reason: `"Allow Bash: bun run build?"`,
        session_id: "s10",
        tmux_window: "lane-10",
      }),
    ],
  }),
  lane({
    id: 11,
    repo: REPOS[1],
    branch: "antigravity/pos-rates",
    last_activity_at: ago(9),
    agent_sessions: [
      agentSession({
        id: 111,
        agent: "antigravity",
        status: "waiting",
        status_reason: `"Which currency should the estimate use?"`,
        session_id: "s11",
        tmux_window: "lane-11",
      }),
    ],
  }),
  lane({
    id: 12,
    repo: REPOS[3],
    branch: "opencode/fix-login-test",
    last_activity_at: ago(20),
    agent_sessions: [
      agentSession({ id: 121, agent: "opencode", status: "running", session_id: "s12", tmux_window: "lane-12" }),
    ],
  }),
  lane({
    id: 13,
    repo: REPOS[2],
    branch: "claude/portfolio-globe",
    last_activity_at: ago(120),
    agent_sessions: [
      agentSession({ id: 131, agent: "claude-code", status: "running", session_id: "s13", tmux_window: "lane-13" }),
    ],
  }),
  lane({
    id: 14,
    repo: REPOS[0],
    branch: "codex/audit-docs",
    last_activity_at: ago(300),
    agent_sessions: [
      agentSession({ id: 141, agent: "codex", status: "ended", session_id: "s14", tmux_window: "lane-14" }),
    ],
  }),
  lane({
    id: 15,
    repo: REPOS[1],
    branch: "claude/wire-usage-ledger",
    last_activity_at: ago(360),
    agent_sessions: [],
  }),
];

const AGENT_CHOICES = [
  { name: "claude-code", command: "claude", detected: true, default: true, custom: false },
  { name: "codex", command: "codex", detected: true, default: false, custom: false },
  { name: "opencode", command: "opencode", detected: true, default: false, custom: false },
];

const PULL_REQUESTS = [
  {
    repo_id: 1,
    repo_name: "deneblondon",
    number: 7,
    title: "Phase 6 voice transcription",
    url: "https://github.com/example/deneblondon/pull/7",
    is_draft: false,
    updated_at: ago(600),
  },
];

const DAEMON_CALL_FIXTURES: Record<string, (params: unknown) => unknown> = {
  "repo.list": () => REPOS,
  "lane.list": () => LANES,
  "usage.get": () => [],
  "terminal.list_all": () => [],
  // Carries no `theme`/`accent` so App.tsx's config.get handler leaves the `?theme=` query
  // param (read by index.html's boot script) in charge of the screenshot's theme.
  "config.get": () => ({ sort_repos_by_activity: false, sort_mode: "default", tab_sort_mode: "manual" }),
  "usage.summary": () => ({
    from: ago(1440),
    to: ago(0),
    groups: [],
    totals: { cost_usd: 0, input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0 },
    unpriced_models: [],
  }),
  "agent.detect": () => AGENT_CHOICES,
  "lane.headline": (params) => HEADLINES[(params as { lane_id: number }).lane_id] ?? null,
  "repo.pull_requests": () => PULL_REQUESTS,
  "repomind.status": () => null,
  "message.list": () => ({ messages: [], next_before: null }),
};

const CONNECTED_STATUS = {
  phase: "connected",
  endpoint: "fixture:///screenshot",
  message: null,
  hint: null,
  log_path: null,
  daemon: { uptime_secs: 3600, repos: REPOS.length, lanes: LANES.length, db_size_bytes: 0, version: "fixture" },
};

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (cmd === "connection_status") return CONNECTED_STATUS as T;
  if (cmd === "daemon_call") {
    const { method, params } = (args ?? {}) as { method: string; params: unknown };
    const handler = DAEMON_CALL_FIXTURES[method];
    return (handler ? handler(params) : null) as T;
  }
  return null as T;
}
