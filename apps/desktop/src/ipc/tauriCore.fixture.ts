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
import type { AgentSession, Lane, Repo, TranscriptItem, PendingDialog } from "../bindings";
// The native Channel constructor needs window.__TAURI_INTERNALS__. Screenshot callbacks
// remain local and use the same onmessage interface; no native bridge is installed.
export class Channel<T> { onmessage: (message: T) => void = () => undefined; }
export const convertFileSrc = (path: string) => path;

function repo(id: number, name: string, label: string | null = null): Repo {
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

function agentSession(overrides: Partial<AgentSession>): AgentSession {
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

function lane(overrides: { id: number; repo: Repo; branch?: string; agent_sessions?: AgentSession[]; last_activity_at: string; view_mode?: string }): Lane {
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
    view_mode: overrides.view_mode ?? null,
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
        // The strip's second line: a real dialog/prompt question, not the status_reason
        // field (that's a description of *why* it's waiting, e.g. "no output for 4m" - see
        // lane 16 below for that case - never a question, and the daemon never quotes it).
        pending_prompt: "Allow Bash: bun run build?",
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
        pending_prompt: "Which currency should the estimate use?",
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
  // Needs-you with no pending prompt at all: the strip falls back to the daemon's
  // status_reason, in its own idiom (crates/repomon-daemon/src/rpc.rs), shown unquoted since
  // it is a description of why the agent is waiting, not a question it asked.
  lane({
    id: 16,
    repo: REPOS[3],
    branch: "claude/stalled-migration",
    last_activity_at: ago(6),
    agent_sessions: [
      agentSession({
        id: 161,
        agent: "claude-code",
        status: "running",
        stale: true,
        status_reason: "no output for 6m",
        session_id: "s16",
        tmux_window: "lane-16",
      }),
    ],
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

// Synthetic ordinary fleet, selected with ?fleet=ordinary. No transcript headlines or PRs,
// repeated main branches, mostly quiet lanes. Wire sources: AgentStatus::as_str in
// core/model.rs; rpc.rs::status_reason (idle = "no output for Nm", ended = "process gone");
// usage_query.rs::lane_headline returns None when no ledger session supplies a headline.
// No private database or live fleet was copied to construct this fixture.
const ORDINARY_REPOS = [
  repo(21, "repomon"), repo(22, "deneblondon"), repo(23, "SAAS"), repo(24, "Mira"),
  repo(25, "portfolio"), repo(26, "TinyAgent"), repo(27, "customer-portal", "Customer portal"),
];
const ORDINARY_LANES = Array.from({ length: 16 }, (_, index) => {
  const target = ORDINARY_REPOS[index % ORDINARY_REPOS.length];
  const id = 200 + index;
  const branch = index < 7 ? "main" : ["fix/login", "main", "chore/dependencies", "feat/account-settings-and-notification-preferences"][index % 4];
  const minutes = [3, 18, 42, 70, 120, 180, 240, 300, 420, 600, 720, 900, 1440, 2880, 4320, 5760][index];
  return lane({
    id, repo: target, branch, last_activity_at: ago(minutes),
    agent_sessions: index % 3 === 2 ? [] : [agentSession({
      id: id * 10, repo_id: target.id, worktree_id: id,
      agent: index % 2 ? "claude-code" : "codex",
      session_id: `s${id}`, tmux_window: `lane-${id}`,
      last_activity_at: ago(minutes),
      status: index === 0 ? "running" : index === 6 ? "ended" : "idle",
      ended_at: index === 6 ? ago(minutes) : null,
      status_reason: index === 0 ? "transcript still being written" : index === 6 ? "process gone" : `no output for ${minutes}m`,
    })],
  });
});
// Verbatim operator evidence. `real` models Part A's rejected candidates; `raw` deliberately
// replays the bad pre-filter output to test clipping and full native tooltips. No UI parser.
export const REAL_HEADLINES = [
  "Reviewed Codex session id: 01a07869-e392-7601-8020-11d9b1a66816",
  "Reviewed Codex session id: 01a02e77-3f96-76f3-ae8a-9ad63cde5576",
  "Reviewed Codex session id: 01a08a8c-c41f-7c42-a50d-9037db875036",
  "Reviewed Codex session id: 01a08a92-2f29-7c31-b9b9-29e073599a24",
  "[REPOMAIL id=729f61afaff89056f8297aa087aa8f75 from=lane-81/4...",
  "Effect logic is correct: no fire on mount, fires once on the true-to-false...",
  "I am working on the Avenith website found in the Avenith folder in this...",
];
const REAL_BRANCHES = ["shopify-app", "woocommerce", "feat/seo-lcp-preload", "feat/seo-crawl-ux", "feat/seo-structured-data", "main"];
const REAL_LANES = ORDINARY_LANES.map((item, index) => ({ ...item,
  worktree: { ...item.worktree, branch: REAL_BRANCHES[index % REAL_BRANCHES.length], name: REAL_BRANCHES[index % REAL_BRANCHES.length] },
}));
REAL_LANES[7] = { ...REAL_LANES[7], repo: REAL_LANES[0].repo, worktree: { ...REAL_LANES[7].worktree, branch: "shopify-app", name: "shopify-app" } };
const REAL_PRS = ["docs: add Spanish README", "fix: make spawned agents immediately talkable", "fix: usage tracker and agent names follow the agent, not its sidebar slot"].map((title, index) => ({ ...PULL_REQUESTS[0], number: [87, 78, 53][index], title }));
// Identity and visible-state replay transcribed from rejected-home-bdb56bd.png.
// Git change counts are deliberately empty fixture values, not a live fleet query.
const OPERATOR_REPOS = [repo(501, "AliHamzaAzam"), repo(502, "aventhi-voice"), repo(503, "repomon"), repo(504, "deneblondon-theme")];
const OPERATOR_LANES = [
  [0, "main", "running"], [3, "feat/voice-loader-tag", "idle"], [2, "c1-design-candidate", null],
  [2, "feat/conversation-view-ui", "running"], [1, "main", null], [2, "feat/conversation-view-daemon", "running"],
  [1, "shopify-app", null], [1, "woocommerce", null], [3, "feat/seo-lcp-preload", null],
  [3, "feat/seo-crawl-ux", null], [3, "feat/seo-structured-data", null], [0, "claude/aventh-hero-redesign-638ea3", null],
].map(([repoIndex, branch, state], index) => lane({id:500+index, repo:OPERATOR_REPOS[Number(repoIndex)], branch:String(branch), last_activity_at:ago(index < 6 ? 29 + index * 3 : index < 8 ? 5760 : 93600), agent_sessions:state ? [agentSession({id:500+index,agent:index === 3 || index === 5 ? "codex" : "claude-code", status:state === "running" ? "running" : "idle",tmux_window:`lane-${500+index}`})] : []}));
const query = new URLSearchParams(location.search);
const fleetMode = query.get("fleet");
const ordinary = fleetMode === "ordinary";
const real = fleetMode === "real" || fleetMode === "raw";
const surface = query.get("surface");
const scenario = query.get("case") ?? "rich";
const conversationLane = lane({ id: 10, repo: scenario === "operator" ? OPERATOR_REPOS[0] : REPOS[0], branch: ["dull", "attachments", "no-source", "operator"].includes(scenario) ? "main" : "codex/charms-collections", last_activity_at: ago(4), view_mode: "conversation", agent_sessions: [agentSession({ id:101, agent: scenario === "no-source" ? "opencode" : scenario === "operator" ? "claude-code" : "codex", session_id: scenario === "no-source" ? null : "s10", tmux_window:"lane-10", status: scenario === "dull" ? "idle" : "running" })] });
const fixtureRepos = fleetMode === "operator" || scenario === "operator" ? OPERATOR_REPOS : ordinary || real ? ORDINARY_REPOS : REPOS;
const fixtureLanes = surface === "conversation" ? [conversationLane] : fleetMode === "operator" ? OPERATOR_LANES : ordinary ? ORDINARY_LANES : real ? REAL_LANES : LANES;

// TranscriptItem::new keeps legacy roles user|assistant|tools. conversation.rs emits running
// tools, partial assistant output, working status and live:dialog; transcript.rs emits pane:<window>
// terminal_block when there is no source. All fixture rows satisfy the merged generated binding.
const fixtureDialog: PendingDialog = { title: "Bash command", question: "Do you want to proceed?", body: ["bun run build"], options: [{ number:1, text:"Yes" }, { number:2, text:"No" }], selected:0 };
const item = (id: string, kind: string, text: string, extra: Partial<TranscriptItem> = {}): TranscriptItem => ({ id, kind, role: kind === "user" ? "user" : kind === "assistant" ? "assistant" : "tools", text, at:"2026-09-10T09:41:00Z", ...extra });
const richItems: TranscriptItem[] = [
  item("u1", "user", "Add collections to the charms page. Keep the existing grid and let customers filter by collection."),
  item("a1", "assistant", "I'll check how the charms are loaded and add collection filters using the existing product data.", { model:"gpt-5-codex" }),
  item("t1", "tool_call", "Found the charms grid and collection query.", { name:"exec_command", input_summary:"rg collections src/routes/charms", result_summary:"2 matching files", status:"ok" }),
  item("t2", "tool_call", "Added a collection filter and kept the grid unchanged.", { name:"apply_patch", input_summary:"src/routes/charms.tsx", result_summary:"1 file changed", status:"ok", diff:"diff --git a/src/routes/charms.tsx b/src/routes/charms.tsx\n--- a/src/routes/charms.tsx\n+++ b/src/routes/charms.tsx\n@@ -1,2 +1,3 @@\n const charms = await getCharms();\n-return <CharmGrid items={charms} />;\n+const collections = await getCollections();\n+return <CharmGrid items={charms} collections={collections} />;" }),
  item("a2", "assistant", "The filter is in place. Selecting a collection updates the grid, and **All charms** clears the selection. Next I'll check the production build.", { model:"gpt-5-codex" }),
  item("t3", "tool_call", "bun run build", { name:"exec_command", input_summary:"bun run build", status:"running" }),
  item("live:working", "status", "Working", { status_kind:"working", at:null }),
  item("stream:1", "assistant", "Checking the collection", { partial:true, at:null }),
];
const brokenItems = [
  item("u1", "user", "Check the collection filter before this ships."),
  item("t-error", "tool_call", "Error: Cannot find module './collections'\nBuild exited with code 1.", { name:"exec_command", input_summary:"bun run build", status:"error" }),
  // durable.rs retains malformed JSON lines as terminal_block; exercise the same wire shape.
  item("broken:1", "terminal_block", '{"type":"response_item","payload": { malformed transcript line'),
  item("a1", "assistant", "The build failed because the collection module is missing. I'll fix the import before trying again."),
];
// The readable reply from the operator's rejected conversation capture. Injected-only
// user frames are absent, as promised by the daemon round-four cleaning contract.
const operatorItems: TranscriptItem[] = [
  item("operator-tool", "tool_call", "Controller inbox checked.", {name:"Bash", input_summary:"Read the controller reply", status:"ok"}),
  item("operator-cost1", "status", "Turn cost $0.0511", {status_kind:"turn_cost",cost_usd:0.0511}),
  item("operator-reply", "assistant", "The controller confirmed. Its reply to you, in short: it read the manual and named its own failures (narrative mails, praise essays to workers, em-dashes, task files in the wrong folder, proposing home-screen rounds on worker fixture screenshots without ever building and looking at the app itself). It says both workers are running and it is applying the manual from this turn.\n\nWhere things stand:\n\n- `~/repomind/REPOMIND.md` is the new operating manual, loaded into every future controller boot. Boot context regenerated.\n- `~/repomind/AGENTS.md` now defines `fleet/<repo>/tasks/` for worker briefs.\n- The live controller (`lane-48355624/1`) has acknowledged and switched behaviour. Its next operator message should be the first under the manual: under fifteen lines, outcome first, one decision, and live screenshots before any more proposal.\n\nIf it slips again, point it at the manual section it broke, since it now treats that as the bar.", {model:"claude-fable-5-1"}),
  item("operator-finished1", "status", "Turn finished", {status_kind:"turn_finished"}),
  item("operator-cost2", "status", "Turn cost $0.0540", {status_kind:"turn_cost",cost_usd:0.054}),
  item("operator-finished2", "status", "Turn finished", {status_kind:"turn_finished"}),
];
let promptAnswered = false;
const eventChannels = new Set<{ onmessage: (event: unknown) => void }>();
const watchTimers = new Map<string, ReturnType<typeof setTimeout>[]>();
function transcriptItems(): TranscriptItem[] {
  if (scenario === "operator") return operatorItems;
  if (scenario === "no-source") return [item("pane:lane-10", "terminal_block", "$ opencode\nWaiting for input.\n› ", { at:null })];
  if (scenario === "dull" || scenario === "attachments") return [item("u1", "user", "Check the README."), item("a1", "assistant", "The README matches the current setup. No changes needed.")];
  if (scenario === "broken") return brokenItems;
  if (scenario === "dialog") return richItems.slice(0, -2);
  if (scenario === "notices") return [richItems[0],
    item("started", "status", "Turn started", { status_kind:"turn_started" }),
    ...richItems.slice(1).filter((row) => !row.partial && row.id !== "live:working").map((row) => row.status === "running" ? {...row,status:"ok" as const} : row),
    item("finished", "status", "Turn finished", { status_kind:"turn_finished" }),
    item("cost", "status", "Turn cost $0.0511", { status_kind:"turn_cost", cost_usd:0.0511 }),
    item("rate", "status", "Rate limit reached. Resets at 14:00.", { status_kind:"rate_limit" }),
    item("usage", "status", "Usage limit reached for this account.", { status_kind:"usage_limit" })];
  return richItems;
}
const initialPage = () => ({ items:transcriptItems(), next_before: scenario === "rich" ? 120 : null });
let fixtureConfig = { sort_repos_by_activity:false, sort_mode:"default", tab_sort_mode:"manual", agent_views:{codex:"conversation", "claude-code":"terminal"}, agent_status_rows:query.has("notices") ? {codex:["rate_limit", "usage_limit"]} : {} };
if (query.get("focus") === "reply") {
  const focusReply = setInterval(() => {
    const field = document.querySelector<HTMLTextAreaElement>('textarea[aria-label^="Reply to"]');
    if (field) { field.focus(); clearInterval(focusReply); }
  }, 100);
}
if (scenario === "diff" || scenario === "notices") {
  const expand = setInterval(() => {
    const option = [...document.querySelectorAll<HTMLElement>('[role="option"]')].find((node) => node.textContent?.includes("Verbose detail"));
    if (option) { option.click(); clearInterval(expand); }
    else document.querySelector<HTMLButtonElement>('button[aria-label="Transcript detail"]')?.click();
  }, 100);
}
if (scenario === "attachments") {
  const timer = setInterval(() => {
    const field = document.querySelector<HTMLTextAreaElement>('textarea[aria-label^="Reply to"]');
    if (!field) return;
    const data = new DataTransfer();
    data.items.add(new File(["fixture image bytes"], "layout-reference.png", { type:"image/png" }));
    data.items.add(new File(["fixture review notes"], "review-notes.md", { type:"text/markdown" }));
    field.dispatchEvent(new ClipboardEvent("paste", { bubbles:true, clipboardData:data }));
    field.focus(); clearInterval(timer);
  }, 100);
}
if (query.has("notices")) {
  const timer = setInterval(() => {
    const section = document.querySelector<HTMLElement>('[aria-label="Conversation detail"]');
    if (!section) return;
    section.querySelectorAll("details").forEach((node) => node.open = true);
    section.scrollIntoView({ block:"start" }); clearInterval(timer);
  }, 200);
}
if (surface === "settings") {
  // Drive the real app's accessible controls. Navigation stays inside the dev-only fixture.
  const navigate = setInterval(() => {
    const tabs = [...document.querySelectorAll<HTMLButtonElement>('[role="tab"]')];
    const agents = tabs.find((button) => button.textContent?.trim() === "Agents");
    if (agents) { agents.click(); clearInterval(navigate); }
    else document.querySelector<HTMLButtonElement>('button[aria-label="Settings"]')?.click();
  }, 100);
}

const DAEMON_CALL_FIXTURES: Record<string, (params: unknown) => unknown> = {
  "repo.list": () => fixtureRepos,
  "lane.list": () => fixtureLanes,
  "usage.get": () => [],
  "terminal.list_all": () => [],
  // Carries no `theme`/`accent` so App.tsx's config.get handler leaves the `?theme=` query
  // param (read by index.html's boot script) in charge of the screenshot's theme.
  "config.get": () => fixtureConfig,
  "config.set": (params) => (fixtureConfig = params as typeof fixtureConfig),
  "lane.set_view": () => null,
  "agent.capture": () => ({ content: scenario === "broken" ? "Error: Cannot find module './collections'\nBuild exited with code 1.\n› " : scenario === "no-source" ? "$ opencode\nWaiting for input.\n› " : scenario === "streaming" ? "Checking collection filters\nBuild running\n› " : "Build completed in 1.4s\nReady for your review\n› " }),
  "agent.prompt": () => ({ dialog: scenario === "dialog" && !promptAnswered ? fixtureDialog : null }),
  "agent.answer": () => { promptAnswered = true; return { answered:"Yes", sent:["Enter"] }; },
  "agent.send_input": () => null,
  "agent.transcript_page": () => ({ items:[item("older:1", "user", "Use the existing brand styles for the collection filter.")], next_before:null }),
  "agent.transcript_watch": (params) => {
    const target = params as { lane_id:number; window:string; on:boolean };
    watchTimers.get(target.window)?.forEach(clearTimeout);
    if (!target.on) { watchTimers.delete(target.window); return null; }
    if (scenario === "rich" || scenario === "streaming" || scenario === "diff") {
      const emit = (text: string, partial: boolean) => eventChannels.forEach((channel) => channel.onmessage({ jsonrpc:"2.0", method:"event.agent.transcript", params:{ lane_id:target.lane_id, window:target.window, subscription_id:1, items:[item("stream:1", "assistant", text, { partial, at:null }), ...(partial ? [] : [{...richItems[5],status:"ok"}])], removed_ids:partial ? [] : ["live:working"], next_before:120 } }));
      watchTimers.set(target.window, [setTimeout(() => emit("Checking the collection filter at narrow widths. The grid keeps its existing spacing", true), 650), setTimeout(() => emit("The collection filter works at both widths. The grid keeps its existing spacing and the build passes.", false), scenario === "streaming" ? 15000 : 1600)]);
    }
    return initialPage();
  },
  "usage.summary": () => ({
    from: ago(1440),
    to: ago(0),
    groups: [],
    totals: { cost_usd: 0, input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0 },
    unpriced_models: [],
  }),
  "agent.detect": () => AGENT_CHOICES,
  "lane.headline": (params) => {
    const id = (params as { lane_id:number }).lane_id;
    if (surface === "conversation" && scenario === "operator") return "Theme-swap watch for aventhi-voice";
    if (fleetMode === "operator" || ordinary || (surface === "conversation" && ["dull", "attachments", "no-source"].includes(scenario))) return null;
    if (real) return fleetMode === "raw" ? REAL_HEADLINES[id - 200] ?? null : id === 206 ? REAL_HEADLINES[6] : null;
    return HEADLINES[id] ?? null;
  },
  "repo.pull_requests": () => ordinary || fleetMode === "operator" ? [] : real ? REAL_PRS : PULL_REQUESTS,
  "repomind.status": () => null,
  "message.list": () => ({ messages: [], next_before: null }),
};

const CONNECTED_STATUS = {
  phase: "connected",
  endpoint: "fixture:///screenshot",
  message: null,
  hint: null,
  log_path: null,
  daemon: { uptime_secs: 3600, repos: fixtureRepos.length, lanes: fixtureLanes.length, db_size_bytes: 0, version: "fixture" },
};

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (cmd === "save_chat_attachment") return `/fixture/attachments/${args?.name}` as T;
  if (cmd === "plugin:dialog|open") return ["/fixture/notes.md"] as T;
  if (cmd === "daemon_subscribe") { eventChannels.add(args?.onEvent as { onmessage: (event: unknown) => void }); return null as T; }
  if (cmd === "term_watch") return { cols:120, rows:32, generation:1, sequence:1 } as T;
  if (cmd === "connection_status") return CONNECTED_STATUS as T;
  if (cmd === "daemon_call") {
    const { method, params } = (args ?? {}) as { method: string; params: unknown };
    const handler = DAEMON_CALL_FIXTURES[method];
    return (handler ? handler(params) : null) as T;
  }
  return null as T;
}
