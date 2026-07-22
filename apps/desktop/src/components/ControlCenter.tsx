import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";

import type { AgentSession, BrowseResult, Commit, PendingDialog, TimelineData, WorkSession } from "../bindings";
import { DaemonRpcError, daemonCall } from "../ipc/rpc";
import { laneIndicator, type FleetStore } from "../stores/fleet";
import type { NotificationStore } from "../stores/notifications";
import type { ActionsStore } from "../stores/actions";

type ControlTab = "actions" | "triage" | "history" | "feed";

interface ControlCenterProps {
  fleet: FleetStore;
  notifications: NotificationStore;
  actions: ActionsStore;
}

function replacementDialog(error: unknown): PendingDialog | null | undefined {
  if (!(error instanceof DaemonRpcError) || error.code !== -32010) return undefined;
  const data = error.data as { dialog?: PendingDialog | null } | null;
  return data?.dialog;
}

function formatTime(value: string): string {
  return new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }).format(new Date(value));
}

function agentKey(agent: AgentSession, index: number): string {
  return agent.tmux_window ?? agent.session_id ?? `${agent.agent}-${index}`;
}

export default function ControlCenter(props: ControlCenterProps) {
  const [open, setOpen] = createSignal(false);
  const [tab, setTab] = createSignal<ControlTab>("actions");
  const [busy, setBusy] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [output, setOutput] = createSignal<unknown>(null);
  const [dialog, setDialog] = createSignal<PendingDialog | null>(null);
  const [commits, setCommits] = createSignal<Commit[]>([]);
  const [sessions, setSessions] = createSignal<WorkSession[]>([]);
  const [timeline, setTimeline] = createSignal<TimelineData | null>(null);
  const [search, setSearch] = createSignal("");
  const [browser, setBrowser] = createSignal<BrowseResult | null>(null);
  const [selectedAgentKey, setSelectedAgentKey] = createSignal<string | null>(null);
  let trigger!: HTMLButtonElement;
  let dialogElement!: HTMLElement;
  let previouslyFocused: HTMLElement | null = null;

  const selectedLane = () => props.fleet.selectedLane();
  const selectedAgents = createMemo(() => selectedLane()?.agent_sessions ?? []);
  const selectedAgent = createMemo(() => selectedAgents().find((agent, index) => agentKey(agent, index) === selectedAgentKey()) ?? selectedAgents()[0] ?? null);
  const pendingAgent = createMemo(() => selectedLane()?.agent_sessions.find((agent) => agent.pending_dialog) ?? null);

  createEffect(() => {
    const agents = selectedAgents();
    if (!agents.some((agent, index) => agentKey(agent, index) === selectedAgentKey())) {
      setSelectedAgentKey(agents[0] ? agentKey(agents[0], 0) : null);
    }
  });

  function focusableElements() {
    return [...dialogElement.querySelectorAll<HTMLElement>(
      'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
    )];
  }

  function openControl() {
    previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : trigger;
    setOpen(true);
    queueMicrotask(() => (focusableElements()[0] ?? dialogElement).focus());
  }

  function closeControl(restoreFocus = true) {
    setOpen(false);
    if (restoreFocus) queueMicrotask(() => (previouslyFocused?.isConnected ? previouslyFocused : trigger)?.focus());
  }

  const onKey = (event: KeyboardEvent) => {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      if (open()) closeControl();
      else openControl();
    } else if (event.key === "Escape" && open()) {
      closeControl();
    } else if (event.key === "Tab" && open()) {
      const focusable = focusableElements();
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (!first || !last) {
        event.preventDefault();
        dialogElement.focus();
      } else if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }
  };

  onMount(() => window.addEventListener("keydown", onKey));
  onCleanup(() => window.removeEventListener("keydown", onKey));

  async function run(label: string, task: () => Promise<unknown>) {
    setBusy(label);
    setError(null);
    try {
      const result = await task();
      if (result !== undefined && result !== null) setOutput(result);
      await props.fleet.refresh();
      return result;
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      return undefined;
    } finally {
      setBusy(null);
    }
  }

  function spawnAgent() {
    const lane = selectedLane();
    if (!lane) return;
    props.actions.spawn(lane);
    closeControl(false);
  }

  function addRepo() {
    void props.actions.addRepo();
    closeControl(false);
  }

  async function browse(path?: string) {
    const result = await run("browse", () => daemonCall("fs.browse", path ? { path } : {}));
    if (result) setBrowser(result as BrowseResult);
  }

  function createLane() {
    const repoId = selectedLane()?.repo.id ?? props.fleet.repos()[0]?.id;
    if (repoId === undefined) return;
    props.actions.newLane(repoId);
    closeControl(false);
  }

  function renameSession() {
    const agent = selectedAgent();
    if (!agent?.session_id) return;
    props.actions.rename({ sessionId: agent.session_id, current: agent.custom_label ?? "" });
    closeControl(false);
  }

  function confirmAction(options: Parameters<ActionsStore["confirm"]>[0]) {
    props.actions.confirm(options);
    closeControl(false);
  }

  async function answer(choice: number) {
    const lane = selectedLane();
    const agent = pendingAgent();
    if (!lane || !agent) return;
    const expect = agent.pending_prompt ?? undefined;
    setBusy("answer");
    setError(null);
    try {
      await daemonCall("agent.answer", {
        lane_id: lane.id,
        window: agent.tmux_window ?? undefined,
        choice,
        expect_summary: expect,
      });
      setDialog(null);
      await props.fleet.refresh();
    } catch (cause) {
      const replacement = replacementDialog(cause);
      if (replacement !== undefined) {
        setDialog(replacement);
        setError(replacement ? "The dialog changed. Review the current options before answering." : "The dialog was already closed.");
      } else {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
    } finally {
      setBusy(null);
    }
  }

  async function loadHistory(query?: string) {
    const lane = selectedLane();
    const now = new Date();
    const from = new Date(now.getTime() - 24 * 60 * 60 * 1000);
    setBusy("history");
    setError(null);
    try {
      const [nextCommits, nextSessions, nextTimeline] = await Promise.all([
        query
          ? daemonCall("commit.search", { query, limit: 100 })
          : lane
            ? daemonCall("commit.recent", { lane_id: lane.id, limit: 100 })
            : Promise.resolve([]),
        daemonCall("sessions", { from_iso: from.toISOString(), to_iso: now.toISOString() }),
        daemonCall("timeline", { from_iso: from.toISOString(), to_iso: now.toISOString(), bucket_secs: 1800 }),
      ]);
      setCommits(nextCommits);
      setSessions(nextSessions);
      setTimeline(nextTimeline);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }

  async function chooseTab(next: ControlTab) {
    setTab(next);
    if (next === "history") await loadHistory();
    if (next === "feed") props.notifications.markAllRead();
  }

  const currentDialog = () => dialog() ?? pendingAgent()?.pending_dialog ?? null;

  return (
    <>
      <button
        ref={trigger}
        type="button"
        class="focus-ring relative rounded-md border border-line bg-raised px-2.5 py-1.5 font-mono text-[0.58rem] uppercase tracking-[0.1em] text-muted hover:text-foreground"
        onClick={openControl}
        aria-haspopup="dialog"
      >
        Control <span class="ml-1 text-[0.5rem] opacity-60">⌘K</span>
        <Show when={props.notifications.unread()}>
          <span class="absolute -right-1.5 -top-1.5 grid size-4 place-items-center rounded-full bg-attention text-[0.5rem] font-bold text-background">
            {props.notifications.unread()}
          </span>
        </Show>
      </button>

      <Show when={open()}>
        <div class="fixed inset-0 z-50 flex items-center justify-center bg-background/70 p-6 backdrop-blur-sm" onPointerDown={(event) => {
          if (event.target === event.currentTarget) closeControl();
        }}>
          <section ref={dialogElement} role="dialog" aria-modal="true" aria-label="Control center" tabIndex={-1} class="grid h-[min(46rem,88vh)] w-[min(62rem,94vw)] grid-cols-[11rem_minmax(0,1fr)] overflow-hidden rounded-xl border border-line bg-surface shadow-[0_28px_90px_var(--shadow)]">
            <nav aria-label="Control sections" class="border-r border-line bg-raised/50 p-2">
              <p class="section-label px-2 pb-3 pt-2">Control center</p>
              <For each={["actions", "triage", "history", "feed"] as ControlTab[]}>
                {(item) => (
                  <button type="button" class={`focus-ring mb-1 flex w-full items-center justify-between rounded-md px-2 py-2 text-left text-xs capitalize ${tab() === item ? "bg-signal/10 text-signal" : "text-muted hover:bg-raised hover:text-foreground"}`} onClick={() => void chooseTab(item)}>
                    <span>{item}</span>
                    <Show when={item === "feed" && props.notifications.unread()}>
                      <span class="rounded-full bg-attention/15 px-1.5 font-mono text-[0.52rem] text-attention">{props.notifications.unread()}</span>
                    </Show>
                  </button>
                )}
              </For>
              <button type="button" class="focus-ring absolute bottom-[8%] ml-2 font-mono text-[0.55rem] uppercase text-muted" onClick={() => closeControl()}>Esc close</button>
            </nav>

            <div class="min-h-0 overflow-y-auto p-5">
              <div class="mb-5 flex items-start justify-between border-b border-line pb-4">
                <div>
                  <p class="section-label">{tab()}</p>
                  <h2 class="mt-1 text-lg font-semibold">{selectedLane()?.repo.name ?? "Fleet"} <span class="font-normal text-muted">/ {selectedLane()?.worktree.branch ?? "no lane"}</span></h2>
                </div>
                <button type="button" class="focus-ring rounded border border-line px-2 py-1 text-xs text-muted" onClick={() => closeControl()}>Close</button>
              </div>

              <Show when={error()}>
                <p class="mb-4 rounded-md border border-fault/40 bg-fault/8 p-2 text-xs text-fault">{error()}</p>
              </Show>

              <Show when={tab() === "actions"}>
                <div class="space-y-5">
                  <Show when={currentDialog()}>
                    {(prompt) => (
                      <section class="rounded-lg border border-attention/40 bg-attention/8 p-4">
                        <p class="section-label text-attention">Needs your answer</p>
                        <h3 class="mt-2 text-sm font-semibold">{prompt().title ?? "Agent question"}</h3>
                        <p class="mt-1 text-sm">{prompt().question}</p>
                        <For each={prompt().body}>{(line) => <p class="mt-1 font-mono text-[0.65rem] text-muted">{line}</p>}</For>
                        <div class="mt-3 grid gap-2">
                          <For each={prompt().options}>
                            {(option, index) => (
                              <button type="button" class="focus-ring rounded-md border border-line bg-surface px-3 py-2 text-left text-xs hover:border-attention/50" onClick={() => void answer(index())} disabled={busy() === "answer"}>
                                <span class="mr-2 font-mono text-attention">{option.number ?? index() + 1}</span>{option.text}
                              </button>
                            )}
                          </For>
                        </div>
                      </section>
                    )}
                  </Show>

                  <section>
                    <p class="section-label mb-2">Agent</p>
                    <Show when={selectedAgents().length > 1}>
                      <div class="mb-2 flex flex-wrap gap-1" role="group" aria-label="Agent session">
                        <For each={selectedAgents()}>
                          {(agent, index) => (
                            <button
                              type="button"
                              class={`focus-ring rounded border px-2 py-1 text-xs ${selectedAgent() === agent ? "border-signal/40 bg-signal/10 text-signal" : "border-line text-muted"}`}
                              aria-pressed={selectedAgent() === agent}
                              onClick={() => setSelectedAgentKey(agentKey(agent, index()))}
                            >
                              {agent.custom_label ?? agent.title ?? `${agent.agent} ${index() + 1}`}
                            </button>
                          )}
                        </For>
                      </div>
                    </Show>
                    <div class="action-grid">
                      <button onClick={() => spawnAgent()} disabled={!selectedLane()}>Spawn agent</button>
                      <button onClick={() => void run("adopt", () => daemonCall("agent.adopt", { lane_id: selectedLane()!.id, session_id: selectedAgent()?.session_id ?? undefined }))} disabled={!selectedLane() || !selectedAgent()?.external}>Adopt external</button>
                      <button onClick={() => renameSession()} disabled={!selectedAgent()?.session_id}>Rename session</button>
                      <button onClick={() => void run("pin", () => daemonCall("agent.pin", { lane_id: selectedLane()!.id, pinned: !selectedLane()!.pinned }))} disabled={!selectedLane()}>{selectedLane()?.pinned ? "Unpin lane" : "Pin lane"}</button>
                      <button onClick={() => void run("continue", () => daemonCall("agent.auto_continue", { lane_id: selectedLane()!.id, enabled: true }))} disabled={!selectedLane()}>Arm auto-continue</button>
                      <button class="is-danger" onClick={() => {
                        const lane = selectedLane();
                        const agent = selectedAgent();
                        if (!lane) return;
                        confirmAction({ title: "Stop agent?", message: "Stop this managed agent. Its terminal session ends.", confirmLabel: "Stop", danger: true, onConfirm: async () => { await daemonCall("agent.stop", { lane_id: lane.id, window: agent?.tmux_window ?? undefined }); await props.fleet.refresh(); } });
                      }} disabled={!selectedLane() || !selectedAgent()?.tmux_window}>Stop agent</button>
                    </div>
                  </section>

                  <section>
                    <p class="section-label mb-2">Lane and repository</p>
                    <div class="action-grid">
                      <button onClick={() => createLane()} disabled={!props.fleet.repos().length}>New lane</button>
                      <button onClick={() => void run("diff", () => daemonCall("lane.diff", { lane_id: selectedLane()!.id, include_patch: true }))} disabled={!selectedLane()}>Review diff</button>
                      <button onClick={() => {
                        const lane = selectedLane();
                        if (!lane) return;
                        confirmAction({ title: "Merge lane?", message: `Merge ${lane.worktree.branch ?? lane.worktree.name} into the repository base branch.`, confirmLabel: "Merge", onConfirm: async () => { await daemonCall("lane.merge", { lane_id: lane.id }); await props.fleet.refresh(); } });
                      }} disabled={!selectedLane() || selectedLane()?.worktree.is_main}>Merge lane</button>
                      <button onClick={() => addRepo()}>Add repository</button>
                      <button onClick={() => void browse(browser()?.path)}>Browse filesystem</button>
                      <button class="is-danger" onClick={() => {
                        const lane = selectedLane();
                        if (!lane || lane.worktree.is_main) return;
                        confirmAction({ title: "Delete lane?", message: `Delete the worktree lane ${lane.worktree.branch ?? lane.worktree.name}. The branch is kept.`, confirmLabel: "Delete", danger: true, onConfirm: async () => { await daemonCall("lane.delete", { lane_id: lane.id, also_delete_branch: false }); await props.fleet.refresh(); } });
                      }} disabled={!selectedLane() || selectedLane()?.worktree.is_main}>Delete lane</button>
                      <button class="is-danger" onClick={() => {
                        const repo = selectedLane()?.repo;
                        if (repo) props.actions.removeRepo(repo);
                        closeControl(false);
                      }} disabled={!selectedLane()}>Remove repository</button>
                    </div>
                  </section>

                  <Show when={browser()}>
                    {(listing) => (
                      <section class="rounded-lg border border-line bg-background p-3">
                        <div class="mb-2 flex items-center justify-between gap-2"><p class="truncate font-mono text-[0.62rem] text-muted">{listing().path}</p><button class="focus-ring rounded border border-line px-2 py-1 text-xs text-muted" onClick={() => void run("discover", () => daemonCall("repo.discover", { root: listing().path, max_depth: 4 }))}>Discover</button></div>
                        <div class="grid max-h-52 gap-1 overflow-y-auto">
                          <Show when={listing().parent}><button class="focus-ring rounded px-2 py-1 text-left text-xs text-muted hover:bg-raised" onClick={() => void browse(listing().parent!)}>../</button></Show>
                          <For each={listing().entries}>{(entry) => <div class="flex items-center gap-2 rounded px-2 py-1 hover:bg-raised"><button class="focus-ring min-w-0 flex-1 truncate text-left text-xs" onClick={() => void browse(entry.path)}>{entry.name}/</button><Show when={entry.is_repo && !entry.added}><button class="focus-ring rounded border border-signal/40 px-2 py-0.5 font-mono text-[0.52rem] uppercase text-signal" onClick={() => void run("repo.add", () => daemonCall("repo.add", { path: entry.path }))}>Add</button></Show><Show when={entry.added}><span class="font-mono text-[0.5rem] uppercase text-muted">added</span></Show></div>}</For>
                        </div>
                      </section>
                    )}
                  </Show>

                  <Show when={output()}>
                    <pre class="max-h-64 overflow-auto rounded-lg border border-line bg-background p-3 font-mono text-[0.65rem] leading-relaxed text-muted">{JSON.stringify(output(), null, 2)}</pre>
                  </Show>
                </div>
              </Show>

              <Show when={tab() === "triage"}>
                <div class="space-y-2">
                  <For each={props.fleet.lanes().filter((lane) => laneIndicator(lane).urgent)} fallback={<p class="text-sm text-muted">No lane currently needs attention.</p>}>
                    {(lane) => (
                      <button type="button" class="focus-ring flex w-full items-center justify-between rounded-lg border border-line p-3 text-left hover:border-attention/50" onClick={() => { props.fleet.setSelectedLaneId(lane.id); setTab("actions"); }}>
                        <span><b class="text-sm">{lane.repo.name} / {lane.worktree.name}</b><span class="mt-1 block text-xs text-muted">{lane.agent_sessions[0]?.last_message ?? lane.worktree.branch}</span></span>
                        <span class="lane-badge is-attention">{laneIndicator(lane).label}</span>
                      </button>
                    )}
                  </For>
                </div>
              </Show>

              <Show when={tab() === "history"}>
                <form class="mb-4 flex gap-2" onSubmit={(event) => { event.preventDefault(); void loadHistory(search()); }}>
                  <input class="focus-ring h-8 flex-1 rounded border border-line bg-background px-2 text-xs outline-none" placeholder="Search commits" value={search()} onInput={(event) => setSearch(event.currentTarget.value)} />
                  <button class="focus-ring rounded bg-signal px-3 font-mono text-[0.58rem] uppercase text-background" type="submit">Search</button>
                </form>
                <div class="grid gap-5 lg:grid-cols-2">
                  <section><p class="section-label mb-2">Commits</p><For each={commits()} fallback={<p class="text-xs text-muted">No commits in this view.</p>}>{(commit) => <div class="border-b border-line py-2"><p class="text-xs font-medium">{commit.summary}</p><p class="mt-1 font-mono text-[0.55rem] text-muted">{commit.oid.slice(0, 8)} · {formatTime(commit.time)}</p></div>}</For></section>
                  <section><p class="section-label mb-2">Sessions, last 24h</p><For each={sessions()} fallback={<p class="text-xs text-muted">No work sessions detected.</p>}>{(session) => <div class="border-b border-line py-2"><p class="text-xs font-medium">{session.repo_names.join(" + ")}</p><p class="mt-1 font-mono text-[0.55rem] text-muted">{session.kind} · {session.commit_count} commits · {Math.round((Date.parse(session.to) - Date.parse(session.from)) / 60000)}m</p></div>}</For></section>
                </div>
                <Show when={timeline()?.rows.length}>
                  <section class="mt-5"><p class="section-label mb-2">Activity timeline</p><For each={timeline()!.rows}>{(row) => <div class="mb-2 grid grid-cols-[7rem_minmax(0,1fr)] items-center gap-2"><span class="truncate text-xs text-muted">{row.repo_name}</span><div class="flex h-5 items-end gap-px">{row.density.map((level) => <i class="flex-1 bg-signal" style={{ height: `${Math.max(8, level * 18)}%`, opacity: `${0.18 + level * 0.14}` }} />)}</div></div>}</For></section>
                </Show>
              </Show>

              <Show when={tab() === "feed"}>
                <div class="mb-3 flex justify-end gap-2">
                  <button class="focus-ring rounded border border-line px-2 py-1 text-xs text-muted" onClick={() => void props.notifications.enableNative()}>{props.notifications.nativeEnabled() ? "Native alerts enabled" : "Enable native alerts"}</button>
                  <button class="focus-ring rounded border border-line px-2 py-1 text-xs text-muted" onClick={props.notifications.clear}>Clear</button>
                </div>
                <For each={props.notifications.items()} fallback={<p class="text-sm text-muted">No notifications yet.</p>}>
                  {(item) => <button type="button" class="focus-ring mb-2 block w-full rounded-lg border border-line p-3 text-left" onClick={() => { props.fleet.setSelectedLaneId(item.lane_id); setTab("actions"); }}><span class="flex items-center justify-between"><b class="text-xs">{item.title}</b><span class="font-mono text-[0.5rem] uppercase text-muted">{item.kind.replace(/_/g, " ")}</span></span><span class="mt-1 block text-xs leading-relaxed text-muted">{item.body}</span></button>}
                </For>
              </Show>
            </div>
          </section>
        </div>
      </Show>
    </>
  );
}

export { replacementDialog };
