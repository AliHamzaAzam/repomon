import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";

import type { AgentSession, ApprovalRule, BrowseResult, Commit, JournalEntry, PendingDialog, Playbook, Schedule, TimelineData, WorkSession } from "../bindings";
import { DaemonRpcError, daemonCall } from "../ipc/rpc";
import { isMac } from "../keymap";
import { agentLabel } from "./agentLabel";
import { laneIndicator, type FleetStore } from "../stores/fleet";
import type { NotificationStore } from "../stores/notifications";
import type { MessageStore } from "../stores/messages";
import type { ActionsStore } from "../stores/actions";
import {
  IconBot,
  IconCheck,
  IconClose,
  IconCommand,
  IconLayers,
  IconPin,
  IconRefresh,
  IconSparkles,
  IconTerminal,
} from "./icons";

const JOURNAL_LIMIT = 200;

type ControlTab = "actions" | "triage" | "history" | "journal" | "playbooks" | "schedules" | "approvals" | "feed";

/// Params for a journal fetch. A blank box is not a search for the empty string: it asks for the
/// recent tail, which is what the tab shows on open. Sending `query: ""` instead would run a
/// substring search that matches everything, ordered by relevance rather than recency.
export function journalQueryParams(query: string): { query?: string; limit: number } {
  const trimmed = query.trim();
  return trimmed ? { query: trimmed, limit: JOURNAL_LIMIT } : { limit: JOURNAL_LIMIT };
}

/// How a playbook stands with respect to the approval gate.
///
/// Three states, not two: a playbook that was approved and then re-drafted by the orchestrator is
/// live under its *old* approved text while the revision waits. Collapsing that into "approved"
/// would hide a pending change, and into "draft" would imply nothing is in force.
export function playbookState(book: Playbook): { label: string; awaitingApproval: boolean } {
  if (book.status !== "approved") return { label: "draft", awaitingApproval: true };
  if (book.draft_content !== null) return { label: "approved · revision pending", awaitingApproval: true };
  return { label: "approved", awaitingApproval: false };
}

/// Params for `schedule.add`. A blank cap is omitted rather than sent as 0: the daemon picks a
/// deliberately conservative default for unattended runs, and 0 would pin the run to no actions
/// at all, which looks like a schedule that silently does nothing.
export function scheduleAddParams(
  spec: string,
  prompt: string,
  cap: string,
): { spec: string; prompt: string; max_actions?: number } {
  const parsed = Number.parseInt(cap.trim(), 10);
  const base = { spec: spec.trim(), prompt: prompt.trim() };
  return Number.isFinite(parsed) && parsed > 0 ? { ...base, max_actions: parsed } : base;
}

/// Approval rules grouped by repo, repos alphabetical and patterns alphabetical within each.
///
/// Grouped because the question a person asks here is "what can run unattended in *this* repo",
/// and a rule is scoped to one repo: the same `cargo test` approved in two repos is two rules, and
/// a flat list makes that look like a duplicate.
export function groupApprovalRules(rules: ApprovalRule[]): Array<{ repo: string; rules: ApprovalRule[] }> {
  const byRepo = new Map<string, ApprovalRule[]>();
  for (const rule of rules) {
    const bucket = byRepo.get(rule.repo);
    if (bucket) bucket.push(rule);
    else byRepo.set(rule.repo, [rule]);
  }
  return [...byRepo.entries()]
    .map(([repo, group]) => ({ repo, rules: [...group].sort((a, b) => a.pattern.localeCompare(b.pattern)) }))
    .sort((a, b) => a.repo.localeCompare(b.repo));
}

interface ControlCenterProps {
  fleet: FleetStore;
  notifications: NotificationStore;
  messages: MessageStore;
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
  const [journal, setJournal] = createSignal<JournalEntry[]>([]);
  const [journalSearch, setJournalSearch] = createSignal("");
  const [playbooks, setPlaybooks] = createSignal<Playbook[]>([]);
  const [openPlaybook, setOpenPlaybook] = createSignal<string | null>(null);
  const [schedules, setSchedules] = createSignal<Schedule[]>([]);
  const [specDraft, setSpecDraft] = createSignal("");
  const [promptDraft, setPromptDraft] = createSignal("");
  const [capDraft, setCapDraft] = createSignal("");
  const [approvals, setApprovals] = createSignal<ApprovalRule[]>([]);
  const [browser, setBrowser] = createSignal<BrowseResult | null>(null);
  const [selectedAgentKey, setSelectedAgentKey] = createSignal<string | null>(null);
  let trigger!: HTMLButtonElement;
  let dialogElement!: HTMLElement;
  let previouslyFocused: HTMLElement | null = null;

  const selectedLane = () => props.fleet.selectedLane();
  const selectedAgents = createMemo(() => selectedLane()?.agent_sessions ?? []);
  const selectedAgent = createMemo(() => selectedAgents().find((agent, index) => agentKey(agent, index) === selectedAgentKey()) ?? selectedAgents()[0] ?? null);
  const pendingAgent = createMemo(() => selectedLane()?.agent_sessions.find((agent) => agent.pending_dialog) ?? null);
  const mailLaneBadges = createMemo(() => [...props.messages.unreadByLane().entries()]
    .map(([laneId, count]) => ({
      laneId,
      count,
      label: props.fleet.lanes().find((lane) => lane.id === laneId)?.worktree.name ?? `lane-${laneId}`,
    }))
    .sort((left, right) => left.laneId - right.laneId));

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
    // Same platform gate as keymap.ts: Cmd is mod on macOS and a held Ctrl there is a terminal
    // chord meant for the agent, not this shortcut. Elsewhere mod is Ctrl.
    const mod = isMac() ? event.metaKey && !event.ctrlKey : event.ctrlKey;
    if (mod && event.key.toLowerCase() === "k") {
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
    const repoId = selectedLane()?.repo.id ?? props.fleet.visibleRepos()[0]?.id;
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

  async function loadJournal(query = "") {
    try {
      const result = await daemonCall("journal.query", journalQueryParams(query));
      setJournal(result.entries);
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function loadPlaybooks() {
    try {
      const result = await daemonCall("playbook.list");
      setPlaybooks(result.playbooks);
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function actOnPlaybook(method: "playbook.approve" | "playbook.delete", name: string) {
    setBusy(name);
    try {
      await daemonCall(method, { name });
      setError(null);
      await loadPlaybooks();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }

  async function loadSchedules() {
    try {
      const result = await daemonCall("schedule.list");
      setSchedules(result.schedules);
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function addSchedule() {
    setBusy("schedule-add");
    try {
      await daemonCall("schedule.add", scheduleAddParams(specDraft(), promptDraft(), capDraft()));
      setSpecDraft("");
      setPromptDraft("");
      setCapDraft("");
      setError(null);
      await loadSchedules();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }

  async function removeSchedule(id: number) {
    try {
      await daemonCall("schedule.remove", { id });
      setError(null);
      await loadSchedules();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function loadApprovals() {
    try {
      const result = await daemonCall("approval.list");
      setApprovals(result.rules);
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function revokeApproval(rule: ApprovalRule) {
    try {
      await daemonCall("approval.remove", { repo: rule.repo, pattern: rule.pattern });
      setError(null);
      await loadApprovals();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function chooseTab(next: ControlTab) {
    setTab(next);
    if (next === "history") await loadHistory();
    if (next === "journal") await loadJournal();
    if (next === "playbooks") await loadPlaybooks();
    if (next === "schedules") await loadSchedules();
    if (next === "approvals") await loadApprovals();
    if (next === "feed") {
      props.notifications.markAllRead();
      await props.messages.refresh();
    }
  }

  const currentDialog = () => dialog() ?? pendingAgent()?.pending_dialog ?? null;

  const tabIcon = (t: ControlTab) => {
    switch (t) {
      case "actions": return <IconCommand size={14} />;
      case "triage": return <IconSparkles size={14} />;
      case "history": return <IconRefresh size={14} />;
      case "journal": return <IconLayers size={14} />;
      case "playbooks": return <IconTerminal size={14} />;
      case "schedules": return <IconPin size={14} />;
      case "approvals": return <IconCheck size={14} />;
      case "feed": return <IconBot size={14} />;
    }
  };

  return (
    <>
        <button
          ref={trigger}
          type="button"
          class="focus-ring relative flex h-7 items-center gap-1.5 rounded-lg border border-line bg-raised/70 px-2.5 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground"
          onClick={openControl}
          aria-haspopup="dialog"
        >
          <IconCommand size={13} />
          <span>Control</span>
          <kbd class="ml-0.5 rounded border border-line bg-surface px-1 py-0.2 font-mono text-[9px] text-muted">
            ⌘K
          </kbd>
          <Show when={props.notifications.unread() + props.messages.unread()}>
            <span class="absolute -right-1 -top-1 grid size-4 place-items-center rounded-full bg-attention font-mono text-[9px] font-bold text-background">
              {props.notifications.unread() + props.messages.unread()}
            </span>
          </Show>
        </button>

        <Show when={open()}>
          <div class="fixed inset-0 z-50 flex items-center justify-center bg-background/80 p-6 backdrop-blur-md" onPointerDown={(event) => {
            if (event.target === event.currentTarget) closeControl();
          }}>
            <section ref={dialogElement} role="dialog" aria-modal="true" aria-label="Control center" tabIndex={-1} class="flex h-[min(48rem,90vh)] w-[min(64rem,95vw)] overflow-hidden rounded-2xl border border-line bg-surface shadow-[0_28px_90px_var(--shadow)]">
              <nav aria-label="Control sections" class="flex w-44 flex-col justify-between border-r border-line bg-surface/50 p-2.5">
                <div>
                  <p class="section-label px-2.5 pb-2.5 pt-1.5">Control Center</p>
                  <div class="space-y-0.5">
                    <For each={["actions", "triage", "history", "journal", "playbooks", "schedules", "approvals", "feed"] as ControlTab[]}>
                      {(item) => (
                        <button
                          type="button"
                          class={`focus-ring flex w-full items-center justify-between rounded-lg px-2.5 py-1.5 text-left text-xs font-medium capitalize transition-colors ${tab() === item ? "bg-raised text-foreground shadow-xs" : "text-muted hover:bg-raised/60 hover:text-foreground"}`}
                          onClick={() => void chooseTab(item)}
                        >
                          <span class="flex items-center gap-2">
                            <span class={tab() === item ? "text-signal" : "text-muted"}>{tabIcon(item)}</span>
                            <span>{item}</span>
                          </span>
                          <Show when={item === "feed" && props.notifications.unread() + props.messages.unread()}>
                            <span class="rounded-full bg-attention/15 px-1.5 font-mono text-[10px] font-semibold text-attention">
                              {props.notifications.unread() + props.messages.unread()}
                            </span>
                          </Show>
                        </button>
                      )}
                    </For>
                  </div>
                </div>
                <div class="border-t border-line/60 px-2 pt-2">
                  <span class="font-mono text-[10px] text-muted">Press Esc to close</span>
                </div>
              </nav>

              <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
                <div class="flex items-center justify-between border-b border-line px-6 py-4">
                  <div>
                    <span class="section-label">{tab()}</span>
                    <h2 class="text-base font-semibold text-foreground">
                      {selectedLane()?.repo.name ?? "Fleet"} <span class="font-normal text-muted">/ {selectedLane()?.worktree.branch ?? "no lane"}</span>
                    </h2>
                  </div>
                  <button
                    type="button"
                    class="focus-ring flex size-7 items-center justify-center rounded-lg text-muted transition-colors hover:bg-raised hover:text-foreground"
                    aria-label="Close Control Center"
                    onClick={() => closeControl()}
                  >
                    <IconClose size={15} />
                  </button>
                </div>

                <div class="min-h-0 flex-1 overflow-y-auto p-6">
                  <Show when={error()}>
                    <p class="mb-4 rounded-xl border border-fault/30 bg-fault/8 p-3 text-xs text-fault">{error()}</p>
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
                              {agentLabel(agent)}
                            </button>
                          )}
                        </For>
                      </div>
                    </Show>
                    <div class="action-grid">
                      <button onClick={() => spawnAgent()} disabled={!selectedLane()}>Spawn agent</button>
                      <button onClick={() => void run("adopt", () => props.actions.adoptAgent(selectedLane()!, selectedAgent()))} disabled={!selectedLane() || !selectedAgent()?.external}>Adopt external</button>
                      <button onClick={() => renameSession()} disabled={!selectedAgent()?.session_id}>Rename session</button>
                      <button onClick={() => void run("pin", () => props.actions.pinLane(selectedLane()!))} disabled={!selectedLane()}>{selectedLane()?.pinned ? "Unpin lane" : "Pin lane"}</button>
                      <button onClick={() => void run("continue", () => daemonCall("agent.auto_continue", { lane_id: selectedLane()!.id, enabled: true }))} disabled={!selectedLane()}>Arm auto-continue</button>
                      <button class="is-danger" onClick={() => {
                        const lane = selectedLane();
                        if (!lane) return;
                        props.actions.stopAgent(lane, selectedAgent());
                        closeControl(false);
                      }} disabled={!selectedLane() || !selectedAgent()?.tmux_window}>Stop agent</button>
                    </div>
                  </section>

                  <section>
                    <p class="section-label mb-2">Lane and repository</p>
                    <div class="action-grid">
                      <button onClick={() => createLane()} disabled={!props.fleet.visibleRepos().length}>New lane</button>
                      <button onClick={() => void run("diff", () => daemonCall("lane.diff", { lane_id: selectedLane()!.id, include_patch: true }))} disabled={!selectedLane()}>Review diff</button>
                      <button onClick={() => {
                        const lane = selectedLane();
                        if (!lane) return;
                        props.actions.mergeLane(lane);
                        closeControl(false);
                      }} disabled={!selectedLane() || selectedLane()?.worktree.is_main}>Merge lane</button>
                      <button onClick={() => addRepo()}>Add repository</button>
                      <button onClick={() => void browse(browser()?.path)}>Browse filesystem</button>
                      <button class="is-danger" onClick={() => {
                        const lane = selectedLane();
                        if (!lane || lane.worktree.is_main) return;
                        props.actions.deleteLane(lane);
                        closeControl(false);
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

              <Show when={tab() === "journal"}>
                <form
                  class="mb-4 flex gap-2"
                  onSubmit={(event) => { event.preventDefault(); void loadJournal(journalSearch()); }}
                >
                  <input
                    class="focus-ring h-8 flex-1 rounded border border-line bg-background px-2 text-xs outline-none"
                    placeholder="Search what repomind did"
                    value={journalSearch()}
                    onInput={(event) => setJournalSearch(event.currentTarget.value)}
                    aria-label="Search the orchestration journal"
                  />
                  <button class="focus-ring rounded bg-signal px-3 font-mono text-[0.58rem] uppercase text-background" type="submit">Search</button>
                </form>
                <For
                  each={journal()}
                  fallback={<p class="text-xs text-muted">Nothing journalled yet. repomind writes here as it works.</p>}
                >
                  {(entry) => (
                    <button
                      type="button"
                      class="focus-ring mb-1.5 block w-full rounded-lg border border-line p-2.5 text-left disabled:cursor-default"
                      disabled={entry.lane_id === null}
                      onClick={() => {
                        if (entry.lane_id === null) return;
                        props.fleet.setSelectedLaneId(entry.lane_id);
                        setTab("actions");
                      }}
                    >
                      <span class="flex items-center justify-between gap-2">
                        <b class="truncate font-mono text-[0.66rem]">{entry.action}</b>
                        <span class={`shrink-0 font-mono text-[0.5rem] uppercase ${entry.outcome === "ok" ? "text-signal" : "text-fault"}`}>
                          {entry.outcome}
                        </span>
                      </span>
                      <span class="mt-1 flex items-center gap-1.5 font-mono text-[0.55rem] text-muted">
                        <span>{formatTime(entry.at)}</span>
                        <Show when={entry.repo}>{(repo) => <span class="truncate">· {repo()}</span>}</Show>
                      </span>
                      <Show when={entry.detail ?? entry.params}>
                        {(text) => <span class="mt-1 block truncate font-mono text-[0.55rem] text-muted/70">{text()}</span>}
                      </Show>
                    </button>
                  )}
                </For>
              </Show>

              <Show when={tab() === "playbooks"}>
                <p class="mb-3 text-xs leading-relaxed text-muted">
                  Procedures repomind drafted from work it finished. A draft is inert: it is only
                  offered back to the orchestrator once you approve it. Open one to read it before
                  you do.
                </p>
                <For each={playbooks()} fallback={<p class="text-xs text-muted">No playbooks yet.</p>}>
                  {(book) => {
                    const state = () => playbookState(book);
                    const isOpen = () => openPlaybook() === book.name;
                    return (
                      <div class="mb-2 rounded-lg border border-line">
                        <button
                          type="button"
                          class="focus-ring flex w-full items-center justify-between gap-2 p-2.5 text-left"
                          onClick={() => setOpenPlaybook(isOpen() ? null : book.name)}
                          aria-expanded={isOpen()}
                        >
                          <b class="truncate text-xs">{book.name}</b>
                          <span class={`shrink-0 font-mono text-[0.5rem] uppercase ${state().awaitingApproval ? "text-attention" : "text-signal"}`}>
                            {state().label}
                          </span>
                        </button>
                        <Show when={isOpen()}>
                          <pre class="max-h-64 overflow-auto border-t border-line px-2.5 py-2 font-mono text-[0.6rem] leading-relaxed whitespace-pre-wrap text-muted">
                            {book.draft_content ?? book.content}
                          </pre>
                          <div class="flex justify-end gap-2 border-t border-line p-2">
                            <button
                              type="button"
                              class="focus-ring rounded border border-line px-2 py-1 font-mono text-[0.55rem] uppercase text-muted hover:text-fault"
                              disabled={busy() === book.name}
                              onClick={() => props.actions.confirmPlaybookDelete(book.name, () => actOnPlaybook("playbook.delete", book.name))}
                            >Delete</button>
                            <Show when={state().awaitingApproval}>
                              <button
                                type="button"
                                class="focus-ring rounded border border-signal/40 bg-signal/10 px-2 py-1 font-mono text-[0.55rem] uppercase text-signal"
                                disabled={busy() === book.name}
                                onClick={() => void actOnPlaybook("playbook.approve", book.name)}
                              >Approve</button>
                            </Show>
                          </div>
                        </Show>
                      </div>
                    );
                  }}
                </For>
              </Show>

              <Show when={tab() === "schedules"}>
                <p class="mb-3 text-xs leading-relaxed text-muted">
                  Standing orchestrations: repomind runs headless on a timer under a lower action
                  cap, and never merges or deletes a lane unattended. Results arrive as
                  notifications and land in the journal.
                </p>
                <form
                  class="mb-4 grid gap-2 rounded-lg border border-line p-2.5"
                  onSubmit={(event) => { event.preventDefault(); void addSchedule(); }}
                >
                  <div class="flex gap-2">
                    <input
                      class="focus-ring h-8 flex-1 rounded border border-line bg-background px-2 text-xs outline-none"
                      placeholder="weekdays 09:00"
                      value={specDraft()}
                      onInput={(event) => setSpecDraft(event.currentTarget.value)}
                      aria-label="Schedule"
                    />
                    <input
                      class="focus-ring h-8 w-24 rounded border border-line bg-background px-2 text-xs outline-none"
                      placeholder="cap"
                      inputMode="numeric"
                      value={capDraft()}
                      onInput={(event) => setCapDraft(event.currentTarget.value)}
                      aria-label="Action cap"
                    />
                  </div>
                  <input
                    class="focus-ring h-8 rounded border border-line bg-background px-2 text-xs outline-none"
                    placeholder="morning fleet briefing"
                    value={promptDraft()}
                    onInput={(event) => setPromptDraft(event.currentTarget.value)}
                    aria-label="Goal"
                  />
                  <div class="flex items-center justify-between gap-2">
                    <span class="font-mono text-[0.55rem] text-muted/70">
                      daily HH:MM · weekdays HH:MM · weekends HH:MM · every Nm · every Nh
                    </span>
                    <button
                      class="focus-ring rounded bg-signal px-3 py-1 font-mono text-[0.58rem] uppercase text-background disabled:opacity-40"
                      type="submit"
                      disabled={busy() === "schedule-add" || !specDraft().trim() || !promptDraft().trim()}
                    >Add</button>
                  </div>
                </form>
                <For each={schedules()} fallback={<p class="text-xs text-muted">Nothing scheduled.</p>}>
                  {(entry) => (
                    <div class="mb-2 flex items-start justify-between gap-2 rounded-lg border border-line p-2.5">
                      <span class="min-w-0">
                        <b class="block truncate font-mono text-[0.66rem]">{entry.spec}</b>
                        <span class="mt-0.5 block truncate text-xs text-muted">{entry.prompt}</span>
                        <span class="mt-1 block font-mono text-[0.55rem] text-muted/70">
                          cap {entry.max_actions} · {entry.last_run_at ? `last run ${formatTime(entry.last_run_at)}` : "never run"}
                        </span>
                      </span>
                      <button
                        type="button"
                        class="focus-ring shrink-0 rounded border border-line px-2 py-1 font-mono text-[0.55rem] uppercase text-muted hover:text-fault"
                        onClick={() => void removeSchedule(entry.id)}
                      >Remove</button>
                    </div>
                  )}
                </For>
              </Show>

              <Show when={tab() === "approvals"}>
                <p class="mb-3 text-xs leading-relaxed text-muted">
                  Command patterns repomind may approve for you, learned from verdicts you gave it
                  and confirmed by you. Destructive commands (force-push, <code>rm -rf</code>,
                  <code>reset --hard</code>) always reach you regardless of any rule here, and a
                  denial is never generalised into an auto-deny.
                </p>
                <For each={groupApprovalRules(approvals())} fallback={<p class="text-xs text-muted">Nothing is auto-approved.</p>}>
                  {(group) => (
                    <section class="mb-3">
                      <p class="section-label mb-1.5">{group.repo}</p>
                      <For each={group.rules}>
                        {(rule) => (
                          <div class="mb-1 flex items-center justify-between gap-2 rounded border border-line px-2.5 py-1.5">
                            <code class="truncate font-mono text-[0.64rem]">{rule.pattern}</code>
                            <button
                              type="button"
                              class="focus-ring shrink-0 rounded border border-line px-2 py-0.5 font-mono text-[0.55rem] uppercase text-muted hover:text-fault"
                              onClick={() => void revokeApproval(rule)}
                            >Revoke</button>
                          </div>
                        )}
                      </For>
                    </section>
                  )}
                </For>
              </Show>

              <Show when={tab() === "feed"}>
                <div class="mb-3 flex justify-end gap-2">
                  <button class="focus-ring rounded border border-line px-2 py-1 text-xs text-muted" onClick={() => void props.notifications.enableNative()}>{props.notifications.nativeEnabled() ? "Native alerts enabled" : "Enable native alerts"}</button>
                  <button class="focus-ring rounded border border-line px-2 py-1 text-xs text-muted" onClick={props.notifications.clear}>Clear</button>
                </div>
                <div class="grid gap-5 lg:grid-cols-2">
                  <section>
                    <p class="section-label mb-2">Fleet mail <Show when={props.messages.unread()}><span class="text-attention">· {props.messages.unread()} unread</span></Show></p>
                    <Show when={mailLaneBadges().length}>
                      <div class="mb-2 flex flex-wrap gap-1" aria-label="Unread mail by lane">
                        <For each={mailLaneBadges()}>{(entry) => <button type="button" class="focus-ring rounded-full bg-attention/10 px-2 py-0.5 font-mono text-[0.52rem] text-attention" onClick={() => props.fleet.setSelectedLaneId(entry.laneId)}>{entry.label} · {entry.count}</button>}</For>
                      </div>
                    </Show>
                    <For each={props.messages.items()} fallback={<p class="text-sm text-muted">No fleet mail yet.</p>}>
                      {(message) => (
                        <button
                          type="button"
                          class={`focus-ring mb-2 block w-full rounded-lg border p-3 text-left ${message.read_state === "unread" ? "border-attention/40 bg-attention/5" : "border-line"}`}
                          onClick={() => { void props.messages.open(message).then(() => setTab("actions")); }}
                        >
                          <span class="flex items-center justify-between gap-2">
                            <b class="truncate text-xs">{message.sender.address} <span class="font-normal text-muted">to {message.recipient.address}</span></b>
                            <span class="shrink-0 font-mono text-[0.5rem] uppercase text-muted">{message.read_state}</span>
                          </span>
                          <span class="mt-1 block text-xs leading-relaxed text-muted">{message.body}</span>
                          <span class="mt-2 flex justify-between font-mono text-[0.5rem] text-muted/70"><span>{formatTime(message.created_at)}</span><span>{message.delivery_state}</span></span>
                        </button>
                      )}
                    </For>
                  </section>
                  <section>
                    <p class="section-label mb-2">Agent notifications</p>
                    <For each={props.notifications.items()} fallback={<p class="text-sm text-muted">No notifications yet.</p>}>
                      {(item) => <button type="button" class="focus-ring mb-2 block w-full rounded-lg border border-line p-3 text-left" onClick={() => { props.fleet.setSelectedLaneId(item.lane_id); setTab("actions"); }}><span class="flex items-center justify-between"><b class="text-xs">{item.title}</b><span class="font-mono text-[0.5rem] uppercase text-muted">{item.kind.replace(/_/g, " ")}</span></span><span class="mt-1 block text-xs leading-relaxed text-muted">{item.body}</span></button>}
                    </For>
                  </section>
                </div>
              </Show>
            </div>
          </div>
        </section>
      </div>
    </Show>
  </>
);
}

export { replacementDialog };
