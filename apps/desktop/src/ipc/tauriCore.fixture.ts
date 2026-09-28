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
import type { AgentSession, CommandCatalog, Lane, Repo, SystemDoctorResult, TranscriptItem, PendingDialog } from "../bindings";
// The native Channel constructor needs window.__TAURI_INTERNALS__. Screenshot callbacks
// remain local and use the same onmessage interface; no native bridge is installed.
export class Channel<T> { onmessage: (message: T) => void = () => undefined; }
export const convertFileSrc = (path: string) => path.startsWith("/fixture/attachments/") ? new URLSearchParams(location.search).get("preview") ?? "/src-tauri/icons/128x128.png" : path;

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
    accent: null,
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

// The operator's own eight, two of them off PATH, for the keyboard-navigation screenshots: the
// arrow grid and the unselectable-but-focusable missing tile only exist at this length.
const AGENT_CHOICES_EIGHT = [
  { name: "claude-code", command: "claude", detected: true, default: true, custom: false },
  { name: "claude-work", command: "claude-work", detected: true, default: false, custom: true },
  { name: "codex", command: "codex", detected: true, default: false, custom: false },
  { name: "hermes", command: "hermes", detected: true, default: false, custom: false },
  { name: "opencode", command: "opencode", detected: true, default: false, custom: false },
  { name: "antigravity", command: "antigravity", detected: true, default: false, custom: false },
  { name: "aider", command: "aider", detected: false, default: false, custom: false },
  { name: "cursor", command: "cursor-agent", detected: false, default: false, custom: false },
];

// The `system.doctor` probe behind Settings > System, carrying the operator's own eight-agent
// split: six CLIs on PATH and two absent, so the panel's six-versus-two balance is what gets
// screenshotted rather than a flattering all-detected row. The two custom Claude entries keep
// their env-prefixed commands, which share a long boilerplate head and differ only in the tail.
const DOCTOR_AGENTS = [
  { kind: "claude-code", name: "Claude Code", command: "claude", detected: true },
  { kind: "claude-code", name: "Claude Work", command: "env -u CLAUDE_CONFIG_DIR claude", detected: true },
  { kind: "claude-code", name: "Claude Alt Config", command: "CLAUDE_CONFIG_DIR='/Users/azaleas/.claude-alt' claude", detected: true },
  { kind: "codex", name: "Codex", command: "codex", detected: true },
  { kind: "hermes", name: "Hermes", command: "hermes", detected: true },
  { kind: "opencode", name: "OpenCode", command: "opencode", detected: true },
  { kind: "aider", name: "Aider", command: "aider", detected: false },
  { kind: "cursor", name: "Cursor", command: "cursor-agent", detected: false },
];
const DOCTOR: SystemDoctorResult = {
  platform: "macos",
  tmux: { available: true, version: "tmux 3.5a", source: "system", path: "/opt/homebrew/bin/tmux", not_applicable: false },
  git: { available: true, version: "git version 2.51.0", path: "/opt/homebrew/bin/git" },
  agent_host: null,
  agents: DOCTOR_AGENTS,
};

// Quota pressure worth looking at: one quiet window, one tight, one at the limit, and one the
// probe could not read at all. `?usage=` opts a screenshot in; every other scenario keeps the
// empty report, because most accounts have nothing probed.
const USAGE_WINDOWS = [
  { label: "5h", pct_used: 12, reset_at: "2026-09-14T18:30:00Z" },
  { label: "wk", pct_used: 88, reset_at: null },
  { label: "sonnet", pct_used: 100, reset_at: null },
  { label: "mo", pct_used: undefined as unknown as number, reset_at: null },
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
const defects = scenario.startsWith("defect-");
if (defects) localStorage.setItem("repomon.workspace.layout", "focused");
const defectRepo = repo(505, "repomind");
// "no-source" models Hermes: no SourceKind scanner exists for it, so the UI's per-kind fallback
// note and the daemon's deliberate terminal_block-from-capture item are what's on screen.
// "antigravity" models one of the daemon's four scanned kinds rendering an ordinary transcript,
// the state the operator's broken capture should reach once C1 round 5's daemon half lands.
// ?agent= overrides the scenario's own default kind, for evidence gathering across every
// spawnable kind (e.g. chat command routing) without a scenario per kind.
const scenarioAgent = query.get("agent") ?? (scenario === "no-source" ? "hermes" : scenario === "antigravity" || scenario === "agy-cleared" ? "antigravity" : scenario === "mail-pinned" ? "claude-code" : scenario === "operator" || defects ? "claude-code" : "codex");
// "agentsDemo" adds a second, non-tmux (external) session beside the primary one, so a single
// conversation screenshot can show both the sidebar's interactive row and its inert counterpart
// side by side. "spawn-loading" strips the lane down to no agent at all: that is the real,
// unforced way to reach the Spawn agent dialog, whose Select Runtime list this then leaves
// loading forever (agent.detect never resolves below) for the loading-skeleton screenshot.
const agentsDemo = query.has("agentsDemo");
const spawnLoading = scenario === "spawn-loading";
const spawnKeys = scenario === "spawn-keys";
const usageProbed = query.has("usage");
const agyState = (query.get("states") ?? "sent") as "sent" | "queued" | "consumed" | "delivered";
const conversationLane = lane({ id: 10, repo: defects ? defectRepo : scenario === "operator" ? OPERATOR_REPOS[0] : REPOS[0], branch: defects || ["dull", "attachments", "no-source", "antigravity", "agy-cleared", "mail-pinned", "long-history", "operator"].includes(scenario) ? "main" : "codex/charms-collections", last_activity_at: ago(4), view_mode: "conversation", agent_sessions: spawnLoading || spawnKeys ? [] : [agentSession({ id:101, agent: scenarioAgent, session_id: scenario === "no-source" ? null : "s10", tmux_window:"lane-10", status: scenario === "dull" ? "idle" : "running" }), ...(defects ? [agentSession({id:102,agent:"claude-code",session_id:"s11",tmux_window:"lane-10/2",status:"idle",custom_label:"ai-chatbot-development"})] : []), ...(agentsDemo ? [agentSession({id:103,agent:"claude-code",session_id:null,tmux_window:null,external:true,status:"idle",custom_label:"design-review"})] : [])] });
const fixtureRepos = defects ? [defectRepo] : fleetMode === "operator" || scenario === "operator" || defects ? OPERATOR_REPOS : ordinary || real ? ORDINARY_REPOS : REPOS;
const fixtureLanes = surface === "conversation" ? [conversationLane] : fleetMode === "operator" ? OPERATOR_LANES : ordinary ? ORDINARY_LANES : real ? REAL_LANES : LANES;

// TranscriptItem::new keeps legacy roles user|assistant|tools. conversation.rs emits running
// tools, partial assistant output, working status and live:dialog; transcript.rs emits pane:<window>
// terminal_block when there is no source. All fixture rows satisfy the merged generated binding.
const fixtureDialog: PendingDialog = { title: "Bash command", question: "Do you want to proceed?", body: ["bun run build"], options: [{ number:1, text:"Yes" }, { number:2, text:"No" }], selected:0 };
const fixtureDialogLong: PendingDialog = { title: "Migration cleanup", question: "The migration touched three tables that still have unindexed foreign keys. How should this proceed?", body: Array.from({ length: 12 }, (_, i) => `Table ${i + 1}: fk_table_${i + 1}_parent references table_${i + 1}(id), no index found.`), options: [
  { number:1, text:"Add the missing indexes", description:"Runs CREATE INDEX CONCURRENTLY for each of the three foreign keys before continuing the migration. Adds a few minutes but avoids locking writes on the affected tables." },
  { number:2, text:"Leave it", description:"No deletion. Everything currently works, but future joins on these foreign keys will do a full table scan until they are indexed later." },
  { number:3, text:"Cancel the migration", description:"Rolls back the schema changes made so far and leaves the tables exactly as they were before this run started." },
], selected:0 };
const item = (id: string, kind: string, text: string, extra: Partial<TranscriptItem> = {}): TranscriptItem => ({ id, kind, role: kind === "user" ? "user" : kind === "assistant" ? "assistant" : "tools", text, at:"2026-09-10T09:41:00Z", ...extra });
const LONG_MAIL = "Round 15 is on main. Ordered-chain fix landed: cached lane.get off the scan path, narrowed the ordering key from per-connection to per-window, added an in-flight chain-head watchdog. Chose Session over per-window release, because a per-window release drops the guarantee that two panes on one connection stay ordered relative to each other, and the watchdog already bounds the worst case.";
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
const commandTerminals = new Set<Channel<ArrayBuffer>>();
let commandSelection = 0;
let commandFinished = false;
function drawCommandFixture() {
  const body = commandFinished ? `Model changed to fixture-${commandSelection + 1}.\r\nReady for your next message.`
    : `Select a model\r\n\r\n${commandSelection === 0 ? ">" : " "} 1. fixture-1\r\n${commandSelection === 1 ? ">" : " "} 2. fixture-2\r\n\r\nUse arrow keys and Enter to select. Escape goes back.`;
  const bytes = new TextEncoder().encode(`\x1b[2J\x1b[H${body}`);
  const frame = new Uint8Array(bytes.length + 1); frame.set(bytes, 1);
  for (const channel of commandTerminals) channel.onmessage(frame.buffer);
  Object.assign(window, {__REPOMON_COMMAND_FIXTURE__:{selection:commandSelection,finished:commandFinished}});
}
const eventChannels = new Set<{ onmessage: (event: unknown) => void }>();
const watchTimers = new Map<string, ReturnType<typeof setTimeout>[]>();
const defectPane = "Looking at the two that matter most, at your window width.\n\nBoth surfaces now match the reference patterns.\n\nMerge to main, yes or no?\n\nCogitated for 10m 39s\n› yes merge it\nauto mode on (shift+tab to cycle)";
function transcriptItems(): TranscriptItem[] {
  if (defects) return [
    item("real-user", "user", "Check the installed conversation."),
    item("real-answer", "assistant", "The conversation has a readable reply. The terminal remains available when you need it.", {model:"claude-opus-5"}),
    item("live:2", "terminal_block", defectPane, {partial:true,at:null}),
    ...(scenario === "defect-images" ? [item("echo", "assistant", 'Attached file: "/fixture/attachments/screen.png"\n\nI can see the layout in this image.'), item("attached-user", "user", 'It is doing this sometimes. Please check the input too.\n\nAttached file: "/fixture/attachments/screen.png"\n\nAttached file: "/fixture/attachments/notes.md"\n\nAttached file: "/fixture/attachments/missing.png"')] : []),
  ];
  if (scenario === "operator") return operatorItems;
  if (scenario === "no-source") return [item("pane:lane-10", "terminal_block", "$ hermes\nWaiting for input.\n› ", { at:null })];
  if (scenario === "antigravity") return [
    item("u1", "user", "Update the regional pricing rules to include the new EU tax bands."),
    item("t1", "tool_call", "Found the pricing table and its existing tax mapping.", { name:"exec_command", input_summary:"rg tax_bands src/pricing", result_summary:"1 matching file", status:"ok" }),
    item("a1", "assistant", "The EU tax bands are in place, using the existing pricing table so no schema change was needed. Checkout totals now include the new bands for EU carts.", { model:"gemini-3-pro" }),
  ];
  // The operator's report: an antigravity chat he had cleared, holding nothing but the two
  // messages he typed into it. `?states=` supplies exactly what the daemon reports for them, so
  // the same fixture shows the regression and the fix without either being drawn by hand.
  // The operator's controller pane: two inbound mails from a worker, which travel the input path
  // and so arrive carrying role "user" with a ticket of their own.
  if (scenario === "mail-pinned") return [
    item("m1", "mail", "Ordered-chain work is committed on the branch; gates are green and the report is in qa/.", { role:"user", mail:{ id:"2dd190b818e0323bb958c845ab7d2f0b", sender:"lane-48358676/1", reply_to:null } }),
    item("m2", "mail", LONG_MAIL, { role:"user", mail:{ id:"7af6dadef35efab8a7c239ddb73cf4a6", sender:"lane-48358676/1", reply_to:null } }),
  ];
  if (scenario === "agy-cleared") return [
    item("u1", "user", "Hi which model are you?", { partial:true }),
    item("u2", "user", "hhh", { partial:true }),
  ];
  if (scenario === "long-history") return Array.from({ length: 40 }, (_, i) => item(`h${i}`, i % 6 === 0 ? "user" : "assistant", i % 6 === 0 ? `Round ${i / 6 + 1}: keep going on the migration.` : `Batch ${i} of the migration is done; the existing schema stayed untouched.`));
  if (scenario === "queued") return [
    item("u1", "user", "Attached file: \"/fixture/attachments/screen.png\"\n\nCheck this layout before you continue."),
    item("a1", "assistant", "The layout matches the reference. Continuing with the migration.", { model:"claude-opus-5" }),
    item("u2", "user", "Also check the mobile breakpoint.", { partial:true }),
    item("u3", "user", "And the tablet one too.", { partial:true }),
  ];
  // A single long queued turn: the pending row must size to its own content up to a bounded
  // share of the pane and offer an explicit "Show more" rather than a nested scrollbar.
  if (scenario === "pending-long") return [item("u1", "user", "Please review the onboarding flow end to end and note every place the copy disagrees with the design doc, especially the empty states, the error toasts, and the confirmation step right before publishing - I want a full pass, not a skim.\n\nAlso check the mobile breakpoint at 375px and 414px, since the last screenshot round only covered desktop widths, and the tablet layout at 768px while you're at it.\n\nOne more thing: the settings drawer still shows the old plan name in two places even though billing already renamed it, so sweep the whole settings surface for stale copy too.", { partial:true })];
  // The DULL case is deliberately the thinnest fixture on offer: one idle agent, nothing said -
  // the quiet lane the busy "rich" fixture never shows on its own.
  if (scenario === "dull") return [];
  if (scenario === "attachments") return [item("u1", "user", "Check the README."), item("a1", "assistant", "The README matches the current setup. No changes needed.")];
  if (scenario === "broken") return brokenItems;
  if (scenario === "dialog" || scenario === "dialog-long") return richItems.slice(0, -2);
  if (scenario === "notices") return [richItems[0],
    item("started", "status", "Turn started", { status_kind:"turn_started" }),
    ...richItems.slice(1).filter((row) => !row.partial && row.id !== "live:working").map((row) => row.status === "running" ? {...row,status:"ok" as const} : row),
    item("finished", "status", "Turn finished", { status_kind:"turn_finished" }),
    item("cost", "status", "Turn cost $0.0511", { status_kind:"turn_cost", cost_usd:0.0511 }),
    item("rate", "status", "Rate limit reached. Resets at 14:00.", { status_kind:"rate_limit" }),
    item("usage", "status", "Usage limit reached for this account.", { status_kind:"usage_limit" })];
  return richItems;
}
const initialPage = () => ({
  items:transcriptItems(),
  next_before: scenario === "rich" ? 120 : scenario === "long-history" ? 40 : null,
  older_message_count: scenario === "long-history" ? 57 : undefined,
  order: scenario === "queued" ? ["u1", "a1", "u2", "u3"] : scenario === "pending-long" ? ["u1"] : scenario === "agy-cleared" ? ["u1", "u2"] : scenario === "mail-pinned" ? ["m1", "m2"] : undefined,
  input_states: scenario === "queued" ? { u2:"consumed", u3:"queued" } : scenario === "pending-long" ? { u1:"queued" } : scenario === "agy-cleared" ? { u1:agyState, u2:agyState } : scenario === "mail-pinned" ? { m1:"sent", m2:"sent" } : undefined,
});
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
if (scenario === "attachments" || scenario.startsWith("defect-composer")) {
  const timer = setInterval(() => {
    const field = document.querySelector<HTMLTextAreaElement>('textarea[aria-label^="Reply to"]');
    if (!field) return;
    const data = new DataTransfer();
    data.items.add(new File(["fixture image bytes"], "layout-reference.png", { type:"image/png" }));
    data.items.add(new File(["fixture review notes"], "review-notes.md", { type:"text/markdown" }));
    field.dispatchEvent(new ClipboardEvent("paste", { bubbles:true, clipboardData:data }));
    if (scenario.startsWith("defect-composer")) {
      field.value = scenario === "defect-composer-long" ? "Please check this layout.\n".repeat(12) : "it is doing this sometimes?";
      field.dispatchEvent(new Event("input", {bubbles:true}));
      if (scenario === "defect-composer-cleared") {
        setTimeout(() => document.querySelector<HTMLButtonElement>('[aria-label="Send reply"]')?.click(), 200);
      }
    }
    field.focus(); clearInterval(timer);
  }, 100);
}
if (scenario.startsWith("native-model")) {
  const timer = setInterval(() => {
    const chip = document.querySelector<HTMLButtonElement>(".composer-model");
    if (!chip) return;
    clearInterval(timer);
    chip.click();
  }, 100);
}
if (scenario === "native-palette" || scenario === "native-empty") {
  const timer = setInterval(() => {
    const field = document.querySelector<HTMLTextAreaElement>('textarea[aria-label^="Reply to"]');
    if (!field) return;
    clearInterval(timer);
    field.focus();
    field.value = scenario === "native-palette" ? "/re" : "/";
    field.dispatchEvent(new Event("input", { bubbles:true }));
  }, 100);
}
if (query.has("drag")) {
  const timer = setInterval(() => {
    const dropZone = document.querySelector<HTMLElement>(".conversation-reply");
    if (!dropZone) return;
    clearInterval(timer);
    const data = new DataTransfer();
    data.items.add(new File(["fixture image bytes"], "screenshot.png", { type:"image/png" }));
    dropZone.dispatchEvent(new DragEvent("dragenter", { bubbles:true, dataTransfer:data }));
  }, 100);
}
if (scenario === "defect-excerpt-open" || scenario === "broken") {
  const timer = setInterval(() => {
    const button = document.querySelector<HTMLButtonElement>(".conversation-excerpt-toggle");
    if (button) { button.click(); clearInterval(timer); }
  }, 100);
}
if (scenario === "long-history" && query.has("load")) {
  const timer = setInterval(() => {
    const button = document.querySelector<HTMLButtonElement>(".conversation-older");
    if (!button) return;
    button.click();
    clearInterval(timer);
    // The loaded evidence (the older rows, the beginning-of-conversation marker) lives at the
    // top of the ledger, not wherever "following the latest output" left the scrollTop - wait
    // for the fixture's single-page load to actually exhaust history (the button's own removal)
    // before scrolling there, so a screenshot shows the settled state, not a mid-flight one.
    const settle = setInterval(() => {
      const stillLoading = document.querySelector(".conversation-older");
      const scroll = document.querySelector<HTMLElement>(".conversation-scroll");
      if (stillLoading || !scroll) return;
      clearInterval(settle);
      // older()'s own scroll-preservation runs in a requestAnimationFrame right after the load
      // resolves; force scrollTop after that frame has had a chance to run, not before, or the
      // two writes race and whichever lands last wins unpredictably.
      requestAnimationFrame(() => requestAnimationFrame(() => { scroll.scrollTop = 0; }));
    }, 50);
  }, 50);
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
  const wanted = query.get("tab") ?? "Agents";
  const navigate = setInterval(() => {
    const tabs = [...document.querySelectorAll<HTMLButtonElement>('[role="tab"]')];
    const target = tabs.find((button) => button.textContent?.trim() === wanted);
    if (target) { target.click(); clearInterval(navigate); }
    else document.querySelector<HTMLButtonElement>('button[aria-label="Settings"]')?.click();
  }, 100);
  if (wanted === "System") {
    // The agents grid sits below two cards inside the modal's own scroller, so the shot has to
    // be scrolled to it: a viewport capture of the panel top proves nothing about eight rows.
    const reveal = setInterval(() => {
      const label = [...document.querySelectorAll<HTMLElement>(".section-label")]
        .find((node) => node.textContent?.trim() === "Coding Agents & Tooling");
      const card = label?.closest("div.rounded-xl");
      if (!card) return;
      clearInterval(reveal);
      card.scrollIntoView({ block: "end" });
    }, 100);
  }
}
if (spawnLoading || spawnKeys) {
  // The real, unforced route to the Spawn dialog: a lane with no agent yet shows this button
  // instead of a mounted pane, so no keyboard-shortcut simulation is needed to reach it.
  const openSpawn = setInterval(() => {
    const button = [...document.querySelectorAll<HTMLButtonElement>("button")].find((node) => node.textContent?.includes("Spawn agent"));
    if (!button) return;
    clearInterval(openSpawn);
    button.click();
  }, 100);
}
if (query.has("pill")) {
  // Scrolls away from the bottom once the ledger has content to scroll, so the Latest output
  // pill is on screen for its screenshot; scenario "rich" keeps streaming a reply in behind it
  // (see agent.transcript_watch below), landing while scrolled up for the unread badge too.
  const scrollUp = setInterval(() => {
    const scroll = document.querySelector<HTMLElement>(".conversation-scroll");
    if (!scroll || scroll.scrollHeight <= scroll.clientHeight) return;
    clearInterval(scrollUp);
    scroll.scrollTop = 0;
  }, 50);
}
if (agentsDemo) {
  // A visible focus ring is the proof this row is a real control now, not decoration.
  const focusRow = setInterval(() => {
    const row = document.querySelector<HTMLElement>(".context-agent-interactive");
    if (!row) return;
    clearInterval(focusRow);
    row.focus();
  }, 100);
}

// The daemon's agent.command_catalog RPC has not shipped yet (see stores/commandCatalog.ts);
// this stands in for it, shaped exactly like the contract, for the native model picker and
// slash palette screenshots. An empty catalog is deliberately its own scenario, not a missing
// case - the whole point of the real RPC is that a fabricated command is never an option.
const NATIVE_CATALOGS: Record<string, CommandCatalog> = {
  "native-model": {
    commands: [{ name:"model", description:"Change the active model", source:"builtin", one_shot:true }],
    models: [
      { id:"claude-opus-5", label:"Opus 5", current:false },
      { id:"claude-sonnet-5", label:"Sonnet 5", current:false },
      { id:"claude-haiku-4-5", label:"Haiku 4.5", current:false },
      { id:"gpt-5-codex", label:"GPT-5 Codex", current:true },
      { id:"gemini-3-pro", label:"Gemini 3 Pro", current:false },
      { id:"gemini-3-flash", label:"Gemini 3 Flash", current:false },
      { id:"grok-4", label:"Grok 4", current:false },
    ],
    model_command: "/model",
    efforts: [],
    effort_command: null,
  },
  "native-palette": {
    commands: [
      { name:"model", description:"Change the active model", source:"builtin", one_shot:true },
      { name:"review", description:"Review the current diff for bugs and style issues", source:"builtin", one_shot:true },
      { name:"clear", description:"Clear the conversation and start a fresh context", source:"builtin", one_shot:true },
      { name:"resume", description:"Resume a previous session", source:"builtin", one_shot:false },
      { name:"repomind:review-plan", description:"Check the active plan against repo conventions", source:"plugin", one_shot:true },
      { name:"repomind:release-notes", description:"Draft release notes from recent commits", source:"plugin", one_shot:true },
      { name:"vim", description:"Toggle vim keybindings", source:"user", one_shot:false },
    ],
    models: [
      { id:"claude-opus-5", label:"Opus 5", current:false },
      { id:"claude-sonnet-5", label:"Sonnet 5", current:true },
    ],
    model_command: "/model",
    efforts: [
      { id:"low", label:"Low", current:false },
      { id:"medium", label:"Medium", current:false },
      { id:"high", label:"High", current:true },
      { id:"xhigh", label:"Xhigh", current:false },
      { id:"max", label:"Max", current:false },
    ],
    effort_command: "/effort",
  },
  "native-empty": { commands: [], models: [], model_command: null, efforts: [], effort_command: null },
  "native-model-dull": {
    commands: [{ name:"model", description:"Change the active model", source:"builtin", one_shot:true }],
    models: [
      { id:"z-ai/glm-5.2", label:"GLM 5.2", current:true },
      { id:"anthropic/claude-fable-5.1", label:"Claude Fable 5.1", current:false },
    ],
    model_command: "/model",
    efforts: [],
    effort_command: null,
  },
  // Defect ONE: a kind with models but no confirmed one-shot switch form (codex, opencode) must
  // never fall back to the terminal - the panel says so instead of offering a selection it can't
  // actually drive.
  "native-model-unconfirmed": {
    commands: [],
    models: [
      { id:"gpt-6-astra", label:"GPT-6-Astra", current:true },
      { id:"gpt-5.6-luna", label:"gpt-5.6-luna", current:false },
    ],
    model_command: null,
    efforts: [],
    effort_command: null,
  },
};
const nativeCatalog = NATIVE_CATALOGS[scenario];

const DAEMON_CALL_FIXTURES: Record<string, (params: unknown) => unknown> = {
  "agent.command_catalog": () => nativeCatalog ?? { commands: [], models: [], model_command: null, efforts: [], effort_command: null },
  "repo.list": () => fixtureRepos,
  "lane.list": () => fixtureLanes,
  "usage.get": () => (usageProbed
    ? [{ key: "default", label: "main", age_secs: 42, report: { windows: USAGE_WINDOWS } },
       { key: "codex", label: "codex", age_secs: 42, report: { windows: USAGE_WINDOWS } }]
    : []),
  "terminal.list_all": () => [],
  // Carries no `theme`/`accent` so App.tsx's config.get handler leaves the `?theme=` query
  // param (read by index.html's boot script) in charge of the screenshot's theme.
  "config.get": () => fixtureConfig,
  "config.set": (params) => (fixtureConfig = params as typeof fixtureConfig),
  "lane.set_view": () => null,
  "agent.capture": () => ({ content: scenario === "broken" ? "Error: Cannot find module './collections'\nBuild exited with code 1.\n› " : scenario === "no-source" ? "$ hermes\nWaiting for input.\n› " : scenario === "streaming" ? "Checking collection filters\nBuild running\n› " : "Build completed in 1.4s\nReady for your review\n› " }),
  "agent.prompt": () => ({ dialog: promptAnswered ? null : scenario === "dialog" ? fixtureDialog : scenario === "dialog-long" ? fixtureDialogLong : null }),
  "agent.answer": () => { promptAnswered = true; return { answered:"Yes", sent:["Enter"] }; },
  "agent.send_input": () => { if (scenario === "commands") { commandFinished = false; drawCommandFixture(); } return null; },
  "agent.key": (params) => {
    if (scenario === "commands") {
      const key = (params as {key:string}).key;
      if (key === "Down" || key === "Up") commandSelection = 1 - commandSelection;
      if (key === "Enter") commandFinished = true;
      drawCommandFixture();
    }
    return null;
  },
  "agent.fit": (params) => ({cols:(params as {cols:number}).cols,rows:(params as {rows:number}).rows}),
  "agent.transcript_page": () => scenario === "long-history"
    ? { items: Array.from({ length: 57 }, (_, i) => item(`older${i}`, i % 6 === 0 ? "user" : "assistant", i === 0 ? "Round 0: start the migration off the legacy schema." : `Setup step ${i} of the migration.`)), next_before: null, older_message_count: null }
    : { items:[item("older:1", "user", "Use the existing brand styles for the collection filter.")], next_before:null },
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
    // `?usage=` screenshots carry the operator's own day of spend, so the sidebar's Today row is
    // as wide in a capture as it is on his screen. Every other scenario keeps the quiet zero.
    totals: { cost_usd: usageProbed ? 124.7 : 0, input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0 },
    unpriced_models: [],
  }),
  // Never resolves for spawn-loading: the daemon serialises this behind the chat's own first
  // page in the reported bug, so the fixture holds it open indefinitely for the loading-skeleton
  // screenshot rather than approximating the delay with a timer.
  "agent.detect": () => (spawnLoading ? new Promise(() => undefined) : spawnKeys ? AGENT_CHOICES_EIGHT : AGENT_CHOICES),
  "lane.headline": (params) => {
    const id = (params as { lane_id:number }).lane_id;
    if (surface === "conversation" && scenario === "operator") return "Theme-swap watch for aventhi-voice";
    if (defects || fleetMode === "operator" || ordinary || (surface === "conversation" && ["dull", "attachments", "no-source"].includes(scenario))) return null;
    if (real) return fleetMode === "raw" ? REAL_HEADLINES[id - 200] ?? null : id === 206 ? REAL_HEADLINES[6] : null;
    return HEADLINES[id] ?? null;
  },
  "repo.pull_requests": () => ordinary || fleetMode === "operator" ? [] : real ? REAL_PRS : PULL_REQUESTS,
  "system.doctor": () => DOCTOR,
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
  if (cmd === "allow_chat_attachment_preview") {
    if (String(args?.path).includes("missing")) throw new Error("Attachment unavailable");
    return args?.path as T;
  }
  if (cmd === "save_chat_attachment") return `/fixture/attachments/${args?.name}` as T;
  if (cmd === "plugin:dialog|open") return ["/fixture/notes.md"] as T;
  if (cmd === "daemon_subscribe") { eventChannels.add(args?.onEvent as { onmessage: (event: unknown) => void }); return null as T; }
  if (cmd === "term_watch") {
    if (scenario === "commands") commandTerminals.add(args?.onBytes as Channel<ArrayBuffer>);
    return { cols:120, rows:32, generation:1, sequence:1 } as T;
  }
  if (cmd === "connection_status") return CONNECTED_STATUS as T;
  if (cmd === "daemon_call") {
    const { method, params } = (args ?? {}) as { method: string; params: unknown };
    const handler = DAEMON_CALL_FIXTURES[method];
    return (handler ? handler(params) : null) as T;
  }
  return null as T;
}
