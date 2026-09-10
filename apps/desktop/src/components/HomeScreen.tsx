import { For, Match, Show, Switch, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { openUrl } from "@tauri-apps/plugin-opener";

import type { AgentChoice, PullRequestSummary } from "../bindings";
import { fetchAgentChoices, modelChoicesFor, pickDefaultAgent } from "../ipc/agentChoices";
import { translateError, type TranslatedError } from "../ipc/errors";
import { daemonCall } from "../ipc/rpc";
import { laneIndicatorTitle, laneState, type FleetStore } from "../stores/fleet";
import {
  formatStripAge,
  laneTitle,
  needsInputLanes,
  recentLanes,
  stripMark,
  uniqueBranchName,
  type NeedsYouRow,
} from "../stores/home";
import Select from "./controls/Select";
import { AgentIcon, IconBolt, IconCheck, IconChevronRight, IconGitBranch, IconPlay, IconStop } from "./icons";

/// Re-fetches the headline cache and the PR list on this cadence, matching the daemon's own
/// cache TTLs so a poll never runs ahead of data the daemon hasn't recomputed yet.
const HEADLINE_REFRESH_MS = 20_000;
const PR_REFRESH_MS = 30_000;

function StripStatusMark(props: { state: ReturnType<typeof laneState>; title?: string }) {
  const mark = () => stripMark(props.state);
  const toneClass = () =>
    mark().tone === "attention"
      ? "text-attention"
      : mark().tone === "signal"
        ? "text-signal"
        : mark().tone === "fault"
          ? "text-fault"
          : "text-muted";
  return (
    <span class={`flex size-[18px] shrink-0 items-center justify-center ${toneClass()}`} title={props.title}>
      <span class="sr-only">{props.title ?? "idle"}</span>
      <Switch>
        <Match when={mark().icon === "bolt"}><IconBolt size={14} /></Match>
        <Match when={mark().icon === "play"}><IconPlay size={14} /></Match>
        <Match when={mark().icon === "check"}><IconCheck size={14} /></Match>
        <Match when={mark().icon === "stop"}><IconStop size={14} /></Match>
      </Switch>
    </span>
  );
}

function HoverOpen() {
  return (
    <span class="hidden items-center gap-1.5 font-mono text-[11px] text-foreground group-hover/strip:flex">
      <IconChevronRight size={11} />
      Open
    </span>
  );
}

function NeedsYouStrip(props: { row: NeedsYouRow; title: string; onOpen: () => void }) {
  const state = () => laneState(props.row.lane);
  return (
    <button
      type="button"
      class="group/strip grid w-full grid-cols-[18px_minmax(0,1fr)_132px_46px] items-center gap-x-3.5 gap-y-1.5 border-b border-line px-6 py-4 text-left"
      onClick={props.onOpen}
      aria-label={`${props.title} in ${props.row.lane.repo.label ?? props.row.lane.repo.name}, needs you`}
    >
      <StripStatusMark state={state()} title={laneIndicatorTitle(props.row.lane)} />
      <span class="min-w-0 truncate text-sm text-foreground">{props.title}</span>
      <span class="truncate text-right font-mono text-[11px] text-muted group-hover/strip:hidden">
        {props.row.lane.repo.label ?? props.row.lane.repo.name}
      </span>
      <span class="text-right font-mono text-[11px] text-muted group-hover/strip:hidden">
        {formatStripAge(props.row.lane.last_activity_at)}
      </span>
      <span class="col-start-2 col-end-5 flex min-w-0 items-center gap-3 font-mono text-xs text-muted">
        <span class="min-w-0 truncate">
          {props.row.question
            ? props.row.isQuestion
              ? `"${props.row.question}"`
              : props.row.question
            : "Waiting on you"}
        </span>
        <span class="ml-auto shrink-0 font-sans text-[11px] font-medium text-attention">needs you</span>
      </span>
    </button>
  );
}

function LaneStrip(props: { state: ReturnType<typeof laneState>; title: string; repo: string; age: string; stateTitle: string; onOpen: () => void }) {
  return (
    <button
      type="button"
      class="group/strip grid w-full grid-cols-[18px_minmax(0,1fr)_132px_46px] items-center gap-x-3.5 border-b border-line px-6 py-3.5 text-left"
      onClick={props.onOpen}
      aria-label={`${props.title} in ${props.repo}, ${props.stateTitle}`}
    >
      <StripStatusMark state={props.state} title={props.stateTitle} />
      <span class="min-w-0 truncate text-sm text-foreground">{props.title}</span>
      <span class="min-w-0 truncate text-right font-mono text-[11px] text-muted group-hover/strip:hidden">
        {props.repo}
      </span>
      <span class="text-right font-mono text-[11px] text-muted group-hover/strip:hidden">{props.age}</span>
      <span class="col-start-4 hidden justify-self-end group-hover/strip:flex">
        <HoverOpen />
      </span>
    </button>
  );
}

function PrStrip(props: { pr: PullRequestSummary }) {
  return (
    <button
      type="button"
      class="group/strip grid w-full grid-cols-[18px_minmax(0,1fr)_132px_46px] items-center gap-x-3.5 border-b border-line px-6 py-3.5 text-left"
      onClick={() => void openUrl(props.pr.url)}
      aria-label={`${props.pr.title} in ${props.pr.repo_name}, pull request`}
    >
      <span class="flex size-[18px] shrink-0 items-center justify-center text-muted" title="pull request">
        <span class="sr-only">pull request</span>
        <IconGitBranch size={14} />
      </span>
      <span class="min-w-0 truncate text-sm text-foreground">
        #{props.pr.number} {props.pr.title}
      </span>
      <span class="min-w-0 truncate text-right font-mono text-[11px] text-muted group-hover/strip:hidden">
        {props.pr.repo_name}
      </span>
      <span class="text-right font-mono text-[11px] text-muted group-hover/strip:hidden">PR</span>
      <span class="col-start-4 hidden justify-self-end group-hover/strip:flex">
        <HoverOpen />
      </span>
    </button>
  );
}

export default function HomeScreen(props: { fleet: FleetStore }) {
  let inputRef: HTMLInputElement | undefined;

  const [choices, setChoices] = createSignal<AgentChoice[]>([]);
  const [text, setText] = createSignal("");
  const [repoId, setRepoId] = createSignal<number | null>(null);
  const [agent, setAgent] = createSignal("");
  const [model, setModel] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [composeError, setComposeError] = createSignal<TranslatedError | null>(null);

  const [headlines, setHeadlines] = createSignal<Record<number, string | null>>({});
  const [prs, setPrs] = createSignal<PullRequestSummary[]>([]);

  const needsInput = createMemo(() => needsInputLanes(props.fleet.fleetLanes()));
  const recent = createMemo(() => recentLanes(props.fleet.fleetLanes()));
  const modelOptions = createMemo(() => modelChoicesFor(agent()));
  const hasContent = createMemo(() => props.fleet.fleetLanes().length > 0 || prs().length > 0);

  onMount(() => {
    inputRef?.focus();
    void fetchAgentChoices()
      .then((detected) => {
        setChoices(detected);
        setAgent(pickDefaultAgent(detected));
      })
      .catch((cause: unknown) => setComposeError(translateError(cause)));

    const fetchPrs = () => void daemonCall("repo.pull_requests").then(setPrs).catch(() => setPrs([]));
    fetchPrs();
    const prTimer = setInterval(fetchPrs, PR_REFRESH_MS);
    // Clearing the cache lets the fetch effect below refetch on the daemon's own TTL, rather
    // than running a second overlapping polling mechanism.
    const headlineTimer = setInterval(() => setHeadlines({}), HEADLINE_REFRESH_MS);
    onCleanup(() => {
      clearInterval(prTimer);
      clearInterval(headlineTimer);
    });
  });

  createEffect(() => {
    if (repoId() !== null) return;
    const first = props.fleet.visibleRepos()[0];
    if (first) setRepoId(first.id);
  });

  createEffect(() => {
    const opts = modelOptions();
    if (opts.length === 0) {
      if (model() !== "") setModel("");
      return;
    }
    if (!opts.includes(model())) setModel(opts[0]);
  });

  createEffect(() => {
    const ids = [...needsInput().map((row) => row.lane.id), ...recent().map((lane) => lane.id)];
    const known = headlines();
    const missing = ids.filter((id) => !(id in known));
    if (missing.length === 0) return;
    void Promise.all(
      missing.map((id) =>
        daemonCall("lane.headline", { lane_id: id })
          .then((headline) => [id, headline] as const)
          .catch(() => [id, null] as const),
      ),
    ).then((pairs) => setHeadlines((prev) => ({ ...prev, ...Object.fromEntries(pairs) })));
  });

  async function submit(event: Event) {
    event.preventDefault();
    const task = text().trim();
    const repo = repoId();
    if (!task || repo === null || !agent() || busy()) return;
    setBusy(true);
    setComposeError(null);
    try {
      const existingBranches = props.fleet
        .lanes()
        .filter((lane) => lane.repo.id === repo)
        .map((lane) => lane.worktree.branch)
        .filter((branch): branch is string => Boolean(branch));
      const branch = uniqueBranchName(task, existingBranches);
      const lane = await daemonCall("lane.create", { repo_id: repo, branch });
      await daemonCall("agent.spawn", { lane_id: lane.id, agent: agent(), task, model: model() || undefined });
      await props.fleet.refresh();
      props.fleet.setSelectedLaneId(lane.id);
      setText("");
    } catch (cause) {
      setComposeError(translateError(cause, { binary: "git" }));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="flex h-full min-h-0 flex-col overflow-y-auto bg-background">
      <form
        class="flex flex-none flex-wrap items-center gap-3 border-b border-line bg-surface px-6 py-4"
        onSubmit={(event) => void submit(event)}
      >
        <span class="text-signal">
          <IconChevronRight size={16} />
        </span>
        <input
          ref={inputRef}
          class="min-w-[10rem] flex-1 border-0 bg-transparent text-[15px] text-foreground outline-none placeholder:text-muted/60"
          value={text()}
          placeholder="Describe a task"
          onInput={(event) => setText(event.currentTarget.value)}
        />
        <div class="flex flex-none items-center gap-1.5">
          <Select
            ariaLabel="Repository"
            size="sm"
            value={repoId() === null ? "" : String(repoId())}
            options={props.fleet.visibleRepos().map((repo) => ({ value: String(repo.id), label: repo.label ?? repo.name }))}
            onChange={(value) => setRepoId(Number(value))}
          />
          <Select
            ariaLabel="Agent kind"
            size="sm"
            value={agent()}
            options={choices().map((choice) => ({
              value: choice.name,
              label: choice.name,
              icon: <AgentIcon agent={choice.name} size={12} />,
            }))}
            onChange={setAgent}
          />
          <Show when={modelOptions().length > 0}>
            <Select
              ariaLabel="Model"
              size="sm"
              value={model()}
              options={modelOptions().map((value) => ({ value, label: value }))}
              onChange={setModel}
            />
          </Show>
        </div>
        <button
          type="submit"
          class="focus-ring shrink-0 rounded px-2 py-1 font-mono text-[11px] text-signal disabled:opacity-50"
          disabled={busy() || !text().trim() || repoId() === null || !agent()}
        >
          {busy() ? "Starting…" : "enter"}
        </button>
      </form>

      <Show when={composeError()}>
        {(err) => (
          <div role="alert" class="border-b border-fault/30 bg-fault/8 px-6 py-2.5 text-xs text-fault">
            {err().friendly}
          </div>
        )}
      </Show>

      <Show
        when={hasContent()}
        fallback={
          <p class="max-w-md px-6 py-5 text-xs leading-relaxed text-muted">
            Start with a task. Repomon makes a lane in the repo you pick and hands it to the agent.
          </p>
        }
      >
        <div class="flex-1">
          <For each={needsInput()}>
            {(row) => (
              <NeedsYouStrip
                row={row}
                title={laneTitle(row.lane, headlines()[row.lane.id])}
                onOpen={() => props.fleet.setSelectedLaneId(row.lane.id)}
              />
            )}
          </For>
          <Show when={needsInput().length > 0}>
            <div class="h-2.5 border-y border-line bg-raised" role="separator" aria-label="End of lanes needing you" />
          </Show>
          <For each={recent()}>
            {(lane) => (
              <LaneStrip
                state={laneState(lane)}
                title={laneTitle(lane, headlines()[lane.id])}
                repo={lane.repo.label ?? lane.repo.name}
                age={formatStripAge(lane.last_activity_at)}
                stateTitle={laneIndicatorTitle(lane) ?? "idle"}
                onOpen={() => props.fleet.setSelectedLaneId(lane.id)}
              />
            )}
          </For>
          <For each={prs()}>{(pr) => <PrStrip pr={pr} />}</For>
        </div>
      </Show>
    </div>
  );
}
