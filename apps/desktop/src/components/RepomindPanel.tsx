import { For, Show, createEffect, createSignal, onCleanup, onMount, type JSX } from "solid-js";

import type { AgentSession, TranscriptItem } from "../bindings";
import { stripAnsi, trimBlankEdges } from "./ansi";
import { daemonCall, subscribeDaemon, type OrchestratorStatus } from "../ipc/rpc";
import {
  agentState,
  agentStateReason,
  stateIndicator,
  type ControllerSummary,
  type FleetStore,
} from "../stores/fleet";
import type { ActionsStore } from "../stores/actions";
import type { EditorStore } from "../stores/editor";
import type { RepomindStore } from "../stores/repomind";
import { agentLabel } from "./agentLabel";
import { journalPathFor, journalTail, readActivePlan, type ActivePlan } from "./repomindDocs";
import {
  AgentIcon,
  IconClose,
  IconLayers,
  IconPlay,
  IconPlus,
  IconSparkles,
  IconStop,
} from "./icons";

/// The panel's three readings of the controller lane. "Home" is what the home itself holds; "Live"
/// and "Transcript" are the primary controller's own session, unchanged from before.
type RepomindView = "home" | "live" | "transcript";

/// The daemon-owned boot document, opened from the Boot context section.
const BOOT_PATH = ".repomind/boot.md";

interface RepomindPanelProps {
  fullscreen?: boolean;
  onToggleFullscreen?: () => void;
  /// Supplies the controller lane. Everything the Home view reads goes through `file.read` on it,
  /// because the home is a normal lane. Optional so the fullscreen host can mount the panel before
  /// the fleet has synced.
  fleet?: FleetStore;
  repomind?: RepomindStore;
  /// Opens the spawn modal on the controller lane ("Spawn controller").
  actions?: ActionsStore;
  editor?: EditorStore;
  /// Puts the center in Editor mode, so a clicked plan actually becomes visible.
  onEnsureEditorOpen?: () => void;
}

const AWAITING_ANSWER = ["permission", "decision"];

/// An empty summary, for a mount that has no fleet store behind it yet.
const NO_CONTROLLERS: ControllerSummary = { lane: null, agents: 0, state: null, urgent: 0 };

/// How long ago something happened, or "never" for something that has not. Coarse on purpose: the
/// question these lines answer is "is this keeping up?", never "what time exactly".
export function sinceLabel(iso: string | null | undefined, now = Date.now()): string {
  if (!iso) return "never";
  const at = Date.parse(iso);
  if (Number.isNaN(at)) return "never";
  const seconds = Math.max(0, Math.round((now - at) / 1000));
  if (seconds < 60) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

/// A section of the Home view: one heading, one optional trailing control, and its body. Flat by
/// design, separated by rules rather than boxed, so the view reads as one column of readings.
function Section(props: {
  title: string;
  detail?: string;
  action?: JSX.Element;
  children?: JSX.Element;
}) {
  return (
    <section class="border-b border-line/70 px-3.5 py-3 last:border-b-0" aria-label={props.title}>
      <div class="mb-2 flex items-baseline justify-between gap-2">
        <h2 class="section-label">{props.title}</h2>
        <div class="flex shrink-0 items-center gap-1.5">
          <Show when={props.detail}>
            <span class="font-mono text-[10px] text-muted/80">{props.detail}</span>
          </Show>
          {props.action}
        </div>
      </div>
      {props.children}
    </section>
  );
}

/// The quiet control a Home section uses for its one action.
function SectionButton(props: { label: string; busy?: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      class="focus-ring rounded border border-line bg-raised/50 px-1.5 py-0.5 font-mono text-[10px] text-muted transition-colors hover:bg-raised hover:text-foreground disabled:opacity-40"
      disabled={props.busy}
      onClick={props.onClick}
    >
      {props.busy ? "Working…" : props.label}
    </button>
  );
}

export default function RepomindPanel(props: RepomindPanelProps) {
  const [status, setStatus] = createSignal<OrchestratorStatus>({ running: false });
  const [items, setItems] = createSignal<TranscriptItem[]>([]);
  const [liveOutput, setLiveOutput] = createSignal("");
  const [message, setMessage] = createSignal("");
  const [view, setView] = createSignal<RepomindView>("home");
  const [busy, setBusy] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [plans, setPlans] = createSignal<ActivePlan[]>([]);
  const [journal, setJournal] = createSignal<string[]>([]);
  const [docsError, setDocsError] = createSignal<string | null>(null);
  let active = true;
  let timer: ReturnType<typeof setInterval> | undefined;
  let unsubscribe: (() => void) | undefined;
  // Newest read wins: a slow `file.read` must never overwrite a later one's results.
  let docsToken = 0;

  const pane = () => trimBlankEdges(stripAnsi(liveOutput()));
  const awaitingAnswer = () => AWAITING_ANSWER.includes(status().attention ?? "");
  const controller = () => props.fleet?.controller() ?? NO_CONTROLLERS;
  const home = () => props.repomind?.status() ?? null;
  const laneId = () => controller().lane?.id ?? home()?.lane_id ?? null;
  const running = () => controller().agents > 0 || status().running;
  const indicator = () => stateIndicator(controller().agents ? controller().state : null);
  const controllers = () => controller().lane?.agent_sessions ?? [];
  const reasoned = () => controllers().filter((session) => agentStateReason(session));
  const atCap = () => {
    const max = home()?.max_controllers;
    return max !== undefined && controller().agents >= max;
  };

  function errorMessage(cause: unknown) {
    return cause instanceof Error ? cause.message : String(cause);
  }

  async function refresh() {
    try {
      const next = await daemonCall("orchestrator.status");
      if (!active) return;
      setStatus(next);
      if (next.running) setItems(await daemonCall("orchestrator.transcript", { limit: 60 }));
      else setItems([]);
    } catch (cause) {
      if (active) setError(errorMessage(cause));
    }
  }

  /// Read the home's plans and today's journal digest through the ordinary lane file RPCs. The
  /// home is a normal lane, so this needs no repomind-specific RPC of its own.
  async function loadDocs(lane: number) {
    const mine = ++docsToken;
    try {
      const listing = await daemonCall("file.list", { lane_id: lane, path: "plans/active" });
      const files = listing.entries.filter(
        (entry) =>
          !entry.is_dir &&
          entry.name.toLowerCase().endsWith(".md") &&
          // The daemon's own counts skip a directory's README, so the panel does too.
          entry.name.toLowerCase() !== "readme.md",
      );
      const read = await Promise.all(
        files.map(async (entry) => {
          const file = await daemonCall("file.read", { lane_id: lane, path: entry.path });
          return readActivePlan(entry.path, file.content);
        }),
      );
      if (!active || mine !== docsToken) return;
      setPlans([...read].sort((a, b) => a.title.localeCompare(b.title)));
      setDocsError(null);
    } catch (cause) {
      if (!active || mine !== docsToken) return;
      setPlans([]);
      setDocsError(errorMessage(cause));
    }

    try {
      const digest = await daemonCall("file.read", {
        lane_id: lane,
        path: journalPathFor(new Date()),
      });
      if (!active || mine !== docsToken) return;
      setJournal(journalTail(digest.content));
    } catch {
      // No digest for today is the ordinary case before the first journal row lands, not a fault.
      if (active && mine === docsToken) setJournal([]);
    }
  }

  // Re-read the home when its shape changes rather than on every heartbeat: the plan count moving,
  // an export landing, the lane appearing, or the operator switching back to this view.
  createEffect(() => {
    const lane = laneId();
    const shown = view() === "home";
    home()?.counts.active_plans;
    home()?.export.last_run;
    if (lane === null || !shown) return;
    void loadDocs(lane);
  });

  onMount(() => {
    void daemonCall("orchestrator.watch", { on: true }).catch((cause: unknown) => setError(errorMessage(cause)));
    void subscribeDaemon((event) => {
      if (event.method === "event.orchestrator.output") {
        const content = (event.params as { content?: unknown }).content;
        if (typeof content === "string") setLiveOutput(content);
      } else if (event.method === "event.orchestrator.status") {
        setStatus(event.params as OrchestratorStatus);
      }
    }).then((stop) => {
      if (active) unsubscribe = stop;
      else stop();
    }).catch((cause: unknown) => setError(errorMessage(cause)));
    void refresh();
    timer = setInterval(() => void refresh(), 1500);
  });

  onCleanup(() => {
    active = false;
    if (timer) clearInterval(timer);
    unsubscribe?.();
    void daemonCall("orchestrator.watch", { on: false }).catch(() => undefined);
  });

  async function lifecycle(action: "start" | "stop") {
    setBusy(action);
    setError(null);
    try {
      if (action === "stop") await daemonCall("orchestrator.stop");
      else await daemonCall("orchestrator.start", {});
      if (action === "start") setView("live");
      else setLiveOutput("");
      await refresh();
      await props.fleet?.refresh();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  }

  async function sendKey(key: string, label = key) {
    if (!status().running) return;
    setBusy(`key:${label}`);
    setError(null);
    try {
      await daemonCall("orchestrator.key", { key });
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  }

  async function send() {
    const text = message().trim();
    if (!text || !status().running) return;
    setBusy("send");
    setError(null);
    try {
      await daemonCall("orchestrator.send_input", { text });
      setMessage("");
      setView("live");
      await refresh();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  }

  /// Open a home-relative file in the editor. The home is an ordinary worktree, so this selects its
  /// lane first and then opens the path exactly as any lane's file is opened.
  function openInEditor(path: string) {
    const lane = laneId();
    if (lane === null) return;
    props.fleet?.setSelectedLaneId(lane);
    props.onEnsureEditorOpen?.();
    void props.editor?.openFile(path);
  }

  function spawnController() {
    const lane = controller().lane;
    if (lane) props.actions?.spawn(lane);
  }

  const tab = (id: RepomindView, label: string) => (
    <button
      type="button"
      role="tab"
      aria-selected={view() === id}
      class={`focus-ring rounded-md px-2.5 py-0.5 text-[11px] font-medium transition-colors ${
        view() === id ? "bg-surface text-foreground shadow-xs font-semibold" : "text-muted hover:text-foreground"
      }`}
      onClick={() => setView(id)}
    >
      {label}
    </button>
  );

  return (
    <div class="flex h-full flex-col bg-surface">
      <div class="flex h-10 shrink-0 items-center justify-between gap-2 border-b border-line bg-surface/95 px-3.5">
        <div class="flex min-w-0 items-center gap-2">
          <span class={`lane-pulse ${running() ? `is-${indicator().tone}` : ""}`} />
          <span class="shrink-0 text-xs font-semibold text-foreground">Repomind</span>
          <span class={`lane-status is-${indicator().tone}`}>{indicator().label}</span>
          <Show when={controller().agents > 0}>
            <span
              class="inline-flex shrink-0 items-center gap-0.5 font-mono text-[10px] leading-none text-muted"
              title={`${controller().agents} controller${controller().agents === 1 ? "" : "s"} in the home lane`}
            >
              <IconLayers size={9} class="text-muted/70" />
              {controller().agents}
            </span>
          </Show>
        </div>
        <div class="flex shrink-0 items-center gap-1.5">
          <Show when={props.onToggleFullscreen}>
            <button
              type="button"
              class="focus-ring flex h-6 items-center rounded border border-line bg-raised/50 px-2 text-[10px] font-medium text-muted hover:text-foreground"
              onClick={props.onToggleFullscreen}
            >
              {props.fullscreen ? "Collapse" : "Expand"}
            </button>
          </Show>
          <Show when={running() && controller().lane}>
            <button
              type="button"
              class="focus-ring flex size-6 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
              onClick={spawnController}
              disabled={atCap()}
              title={
                atCap()
                  ? `The home already runs its maximum of ${home()?.max_controllers} controllers`
                  : "Spawn another controller in the home lane"
              }
              aria-label="Spawn controller"
            >
              <IconPlus size={12} />
            </button>
          </Show>
          <button
            type="button"
            class={`focus-ring flex h-6 items-center gap-1 rounded-md border px-2 text-[11px] font-medium transition-colors ${
              running()
                ? "border-fault/30 bg-fault/10 text-fault hover:bg-fault/20"
                : "border-signal/40 bg-signal/10 text-signal hover:bg-signal/20"
            }`}
            onClick={() => void lifecycle(running() ? "stop" : "start")}
            disabled={Boolean(busy())}
          >
            {running() ? <IconStop size={11} /> : <IconPlay size={11} />}
            <span>
              {busy() === "start"
                ? "Starting…"
                : busy() === "stop"
                  ? "Stopping…"
                  : running()
                    ? "Stop"
                    : "Start"}
            </span>
          </button>
        </div>
      </div>

      <Show when={error()}>
        {(err) => (
          <div role="alert" class="m-3 mb-0 flex items-start justify-between gap-2 rounded-xl border border-fault/30 bg-fault/8 p-2.5 text-xs text-fault">
            <span>{err()}</span>
            <button type="button" class="focus-ring text-muted hover:text-foreground" aria-label="Dismiss repomind error" onClick={() => setError(null)}>
              <IconClose size={12} />
            </button>
          </div>
        )}
      </Show>

      <div class="flex h-10 shrink-0 items-center justify-between border-b border-line px-3.5" role="tablist" aria-label="Repomind views">
        <div class="flex items-center rounded-lg border border-line bg-raised/50 p-0.5">
          {tab("home", "Home")}
          {tab("live", "Live Feed")}
          {tab("transcript", "Transcript")}
        </div>
        <Show when={status().attention && status().attention !== "none"}>
          <span class="rounded-full border border-attention/40 bg-attention/10 px-2 py-0.5 font-mono text-[9px] font-semibold uppercase text-attention">
            {status().attention}
          </span>
        </Show>
      </div>

      <Show when={status().running && awaitingAnswer()}>
        <div class="flex items-center gap-1.5 border-b border-line bg-attention/5 px-3 py-2">
          <span class="mr-1 font-mono text-[10px] font-semibold uppercase text-attention">Answer:</span>
          <For each={["1", "2", "3"]}>
            {(digit) => (
              <button
                type="button"
                class="focus-ring flex size-6 items-center justify-center rounded-md border border-line bg-surface font-mono text-xs font-medium text-foreground hover:bg-raised"
                disabled={Boolean(busy())}
                onClick={() => void sendKey(digit)}
                title={`Pick option ${digit}`}
              >{digit}</button>
            )}
          </For>
          <button
            type="button"
            class="focus-ring ml-1 rounded-md border border-signal/40 bg-signal/10 px-2.5 py-1 font-mono text-[11px] font-medium text-signal hover:bg-signal/20"
            disabled={Boolean(busy())}
            onClick={() => void sendKey("Enter")}
            title="Confirm selected option"
          >{busy() === "key:Enter" ? "…" : "Enter"}</button>
          <button
            type="button"
            class="focus-ring rounded-md border border-line bg-surface px-2 py-1 font-mono text-[11px] font-medium text-muted hover:text-fault"
            disabled={Boolean(busy())}
            onClick={() => void sendKey("Escape")}
            title="Cancel prompt"
          >Esc</button>
        </div>
      </Show>

      <div class="min-h-0 flex-1 overflow-y-auto">
        <Show when={view() === "home"}>
          <Show
            when={laneId() !== null}
            fallback={
              <p class="p-4 text-xs leading-relaxed text-muted">
                The repomind home has no lane yet. The daemon creates it the first time it starts
                with a reachable home directory.
              </p>
            }
          >
            <Section title="Active plans" detail={String(plans().length)}>
              <Show
                when={plans().length}
                fallback={
                  <p class="text-xs leading-relaxed text-muted">
                    {docsError() ?? "No goals in flight. Repomind writes one file per goal into plans/active."}
                  </p>
                }
              >
                <ul class="space-y-0.5">
                  <For each={plans()}>
                    {(plan) => (
                      <li>
                        <button
                          type="button"
                          class="focus-ring w-full rounded-md px-1.5 py-1 text-left transition-colors hover:bg-raised/60"
                          onClick={() => openInEditor(plan.path)}
                          title={`Open ${plan.path}`}
                        >
                          <span class="block truncate text-xs font-medium text-foreground">{plan.title}</span>
                          <Show when={plan.nextStep}>
                            <span class="mt-0.5 block truncate text-[11px] text-muted">Next: {plan.nextStep}</span>
                          </Show>
                        </button>
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
            </Section>

            <Section title="Journal" detail={journal().length ? "today" : undefined}>
              <Show
                when={journal().length}
                fallback={<p class="text-xs text-muted">Nothing exported into today's digest yet.</p>}
              >
                <ul class="space-y-1.5">
                  <For each={journal()}>
                    {(entry) => (
                      <li class="whitespace-pre-wrap break-words border-l border-line pl-2 font-mono text-[10px] leading-relaxed text-muted">
                        {entry}
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
            </Section>

            <Section
              title="Boot context"
              detail={home()?.boot.generated_at ? `${home()?.boot.tokens_estimate} tokens` : undefined}
              action={
                <>
                  <SectionButton
                    label="Regenerate"
                    busy={props.repomind?.busy() === "boot"}
                    onClick={() => void props.repomind?.regenerateBoot()}
                  />
                  <SectionButton label="Open boot.md" onClick={() => openInEditor(BOOT_PATH)} />
                </>
              }
            >
              <p class="text-xs text-muted">Assembled {sinceLabel(home()?.boot.generated_at)}</p>
              <Show when={home()?.boot.trimmed.length}>
                <p class="mt-1.5 text-[11px] leading-relaxed text-attention">
                  The token budget left out: {home()?.boot.trimmed.join(", ")}
                </p>
              </Show>
            </Section>

            <Section
              title="Export"
              detail={home()?.export.pending ? "pending" : undefined}
              action={
                <SectionButton
                  label="Export now"
                  busy={props.repomind?.busy() === "export"}
                  onClick={() => void props.repomind?.runExport()}
                />
              }
            >
              <p class="text-xs text-muted">Last run {sinceLabel(home()?.export.last_run)}</p>
              <Show when={home()?.export.last_error}>
                <p class="mt-1.5 text-[11px] leading-relaxed text-fault">{home()?.export.last_error}</p>
              </Show>
            </Section>

            <Section title="Controllers" detail={String(controller().agents)}>
              <Show
                when={controllers().length}
                fallback={
                  <p class="text-xs leading-relaxed text-muted">
                    No controller is running. Start one to coordinate work across the fleet.
                  </p>
                }
              >
                <ul class="space-y-0.5">
                  <For each={controllers()}>
                    {(session: AgentSession) => {
                      const state = () => stateIndicator(agentState(session));
                      return (
                        <li
                          class="flex items-center gap-1.5 rounded-md px-1.5 py-1"
                          title={agentStateReason(session) ?? undefined}
                        >
                          <AgentIcon agent={session.agent} size={11} class="shrink-0 text-muted/70" />
                          <span class="min-w-0 flex-1 truncate text-xs text-foreground">{agentLabel(session)}</span>
                          <span class={`lane-status is-${state().tone}`}>{state().label}</span>
                        </li>
                      );
                    }}
                  </For>
                </ul>
                <Show when={reasoned().length}>
                  <ul class="mt-1.5 space-y-0.5">
                    <For each={reasoned()}>
                      {(session) => (
                        <li class="px-1.5 text-[11px] leading-snug text-muted">
                          {agentLabel(session)}: {agentStateReason(session)}
                        </li>
                      )}
                    </For>
                  </ul>
                </Show>
              </Show>
            </Section>
          </Show>
        </Show>

        <Show when={view() === "transcript"}>
          <div class="space-y-2.5 p-3">
            <For each={items()}>
              {(item) => (
                <article class={`repomind-message is-${item.role}`}>
                  <p class="mb-1 font-mono text-[10px] font-semibold uppercase tracking-wider text-muted">{item.role}</p>
                  <p class="whitespace-pre-wrap text-xs leading-relaxed text-foreground">{item.text}</p>
                </article>
              )}
            </For>
            <Show when={!items().length}>
              <div class="rounded-xl border border-line bg-surface/50 p-4 text-center">
                <p class="text-xs text-muted">
                  {status().running && status().backend === "codex"
                    ? "This backend streams directly to the live feed."
                    : status().running
                      ? "Waiting for controller activity…"
                      : "Start Repomind to coordinate work across the fleet."}
                </p>
              </div>
            </Show>
          </div>
        </Show>

        <Show when={view() === "live"}>
          <pre
            aria-label="Repomind live pane"
            class={`p-3 font-mono text-xs leading-relaxed text-muted/90 ${props.fullscreen ? "overflow-x-auto whitespace-pre" : "whitespace-pre-wrap break-words"}`}
          >{pane() || (status().running ? "Attaching to the live repomind pane…" : "Start Repomind to view live agent orchestrations.")}</pre>
        </Show>
      </div>

      <form class="border-t border-line bg-surface/50 p-3" onSubmit={(event) => { event.preventDefault(); void send(); }}>
        <textarea
          aria-label="Message repomind"
          class="focus-ring min-h-16 w-full resize-none rounded-xl border border-line bg-background p-2.5 text-xs text-foreground outline-none placeholder:text-muted/60"
          placeholder="Coordinate the fleet or issue instructions…"
          value={message()}
          onInput={(event) => setMessage(event.currentTarget.value)}
          disabled={!status().running || busy() === "send"}
        />
        <button
          type="submit"
          class="focus-ring mt-2 flex w-full items-center justify-center gap-1.5 rounded-lg bg-signal px-3 py-1.5 text-xs font-semibold text-background transition-colors hover:bg-signal/90 disabled:opacity-40"
          disabled={!status().running || !message().trim() || busy() === "send"}
        >
          <IconSparkles size={13} />
          <span>{busy() === "send" ? "Sending…" : "Send Instruction"}</span>
        </button>
      </form>
    </div>
  );
}
