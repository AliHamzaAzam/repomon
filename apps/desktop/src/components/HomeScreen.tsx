import { For, Match, Show, Switch, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { openUrl } from "@tauri-apps/plugin-opener";

import type { AgentChoice, Lane, PullRequestSummary } from "../bindings";
import { fetchAgentChoices, modelChoicesFor, pickDefaultAgent } from "../ipc/agentChoices";
import { translateError, type TranslatedError } from "../ipc/errors";
import { daemonCall } from "../ipc/rpc";
import { laneIndicatorTitle, laneState, type FleetStore } from "../stores/fleet";
import {
  formatStripAge,
  laneTitle,
  needsInputLanes,
  recentLanes,
  stripStatus,
  uniqueBranchName,
  type NeedsYouRow,
} from "../stores/home";
import Select from "./controls/Select";
import { AgentIcon, IconBolt, IconCheck, IconChevronRight, IconGitBranch, IconPlay, IconStop, IconIdle } from "./icons";

/// Re-fetches the headline cache and the PR list on this cadence, matching the daemon's own
/// cache TTLs so a poll never runs ahead of data the daemon hasn't recomputed yet.
const HEADLINE_REFRESH_MS = 20_000;
const PR_REFRESH_MS = 30_000;

// Preserve the strip board, but make the ordinary repo/branch pair its first-class identity.
// Related information stays in a bounded reading column even on a very wide window.
const STATUS_TONE = { attention: "text-attention", signal: "text-signal", fault: "text-fault", muted: "text-muted" };

function StripStatusMark(props: { status: ReturnType<typeof stripStatus>; title?: string }) {
  return (
    <span class={`home-state-mark ${STATUS_TONE[props.status.tone]}`} title={props.title ?? props.status.label} aria-hidden="true">
      <Switch>
        <Match when={props.status.icon === "bolt"}><IconBolt size={14} /></Match>
        <Match when={props.status.icon === "play"}><IconPlay size={14} /></Match>
        <Match when={props.status.icon === "check"}><IconCheck size={14} /></Match>
        <Match when={props.status.icon === "stop"}><IconStop size={14} /></Match>
        <Match when={props.status.icon === "idle"}><IconIdle size={14} /></Match>
        <Match when={props.status.icon === "branch"}><IconGitBranch size={14} /></Match>
      </Switch>
    </span>
  );
}

function LaneIdentity(props: { lane: Lane; headline?: string | null; repeated?: boolean }) {
  return (
    <span class="home-identity">
      <span class="home-repo">{props.lane.repo.label ?? props.lane.repo.name}</span>
      <span class={`home-title ${props.headline?.trim() ? "" : "is-branch"}`}>
        <Show when={!props.headline?.trim()}><IconGitBranch size={12} /></Show>
        <span class="truncate">{laneTitle(props.lane, props.headline)}</span>
        <Show when={props.repeated}><span class="shrink-0 text-xs text-muted">lane {props.lane.id}</span></Show>
      </span>
    </span>
  );
}

function LaneStrip(props: { lane: Lane; headline?: string | null; repeated?: boolean; needsYou?: NeedsYouRow; onOpen: () => void }) {
  const status = () => stripStatus(props.lane);
  const identity = () => `${props.lane.repo.label ?? props.lane.repo.name}: ${laneTitle(props.lane, props.headline)}`;
  return (
    <button
      type="button"
      class={`home-strip focus-ring ${props.needsYou ? "is-urgent" : ""}`}
      onClick={props.onOpen}
      aria-label={`${identity()}${props.repeated ? `, lane ${props.lane.id}` : ""}, ${status().label}`}
      title={`${identity()}\n${laneIndicatorTitle(props.lane) ?? status().label}\n${props.lane.worktree.path}`}
    >
      <StripStatusMark status={status()} title={laneIndicatorTitle(props.lane)} />
      <LaneIdentity lane={props.lane} headline={props.headline} repeated={props.repeated} />
      <span class={`home-state ${STATUS_TONE[status().tone]}`}>{props.needsYou && status().label === "Needs you" ? "" : status().label}</span>
      <span class="home-age" title={`Last activity: ${props.lane.last_activity_at}`}>{formatStripAge(props.lane.last_activity_at)}</span>
      <Show when={props.needsYou}>
        {(row) => (
          <span class="home-question">
            <span class="truncate">{row().question
              ? row().isQuestion ? `"${row().question}"` : row().question
              : status().label === "Turn complete" ? "Ready for your review" : "Waiting on you"}</span>
            <span class="home-needs-you">needs you</span>
          </span>
        )}
      </Show>
    </button>
  );
}

function PrStrip(props: { pr: PullRequestSummary }) {
  return (
    <button
      type="button"
      class="home-strip focus-ring"
      onClick={() => void openUrl(props.pr.url)}
      aria-label={`${props.pr.repo_name}: ${props.pr.title}, pull request`}
    >
      <span class="home-state-mark text-muted"><IconGitBranch size={14} /></span>
      <span class="home-identity">
        <span class="home-repo">{props.pr.repo_name}</span>
        <span class="home-title truncate">#{props.pr.number} {props.pr.title}</span>
      </span>
      <span class="home-state text-muted">Pull request</span>
      <span class="home-age"><IconChevronRight size={12} /></span>
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
  const repeatedIdentities = createMemo(() => {
    const counts = new Map<string, number>();
    for (const lane of props.fleet.fleetLanes()) {
      const key = `${lane.repo.id}:${laneTitle(lane, headlines()[lane.id])}`;
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    return counts;
  });
  const repeated = (lane: Lane) => (repeatedIdentities().get(`${lane.repo.id}:${laneTitle(lane, headlines()[lane.id])}`) ?? 0) > 1;
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
    <div class="home-screen h-full min-h-0 overflow-y-auto bg-background">
      <div class="home-board">
        <form
          class="flex flex-none flex-wrap items-center gap-3 border-b border-line bg-surface px-6 py-4"
          onSubmit={(event) => void submit(event)}
        >
          <span class="text-signal">
            <IconChevronRight size={16} />
          </span>
          <input
            ref={inputRef}
            aria-label="Describe a task"
            class="min-w-[10rem] flex-1 border-0 bg-transparent text-[15px] text-foreground placeholder:text-muted"
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
          <div class="home-strips" onKeyDown={(event) => {
            if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
            const rows = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>(".home-strip"));
            const index = rows.indexOf(document.activeElement as HTMLButtonElement);
            if (index < 0) return;
            event.preventDefault();
            rows[(index + (event.key === "ArrowDown" ? 1 : -1) + rows.length) % rows.length]?.focus();
          }}>
            <Show when={props.fleet.synced() && needsInput().length === 0 && props.fleet.fleetLanes().length > 0}>
              <div class="home-quiet">
                <IconCheck size={16} />
                <span>Nothing needs you</span>
                <span class="home-quiet-detail">{recent().filter((lane) => laneState(lane) === "running" || laneState(lane) === "inferred").length} running</span>
                <span class="home-quiet-detail">{props.fleet.fleetLanes().length} lanes</span>
              </div>
            </Show>
            <For each={needsInput()}>
              {(row) => (
                <LaneStrip
                  lane={row.lane}
                  needsYou={row}
                  headline={headlines()[row.lane.id]}
                  repeated={repeated(row.lane)}
                  onOpen={() => props.fleet.setSelectedLaneId(row.lane.id)}
                />
              )}
            </For>
            <Show when={props.fleet.fleetLanes().length > 0}>
              <div class="home-attention-rule" role="separator" aria-label="End of attention strip" />
            </Show>
            <For each={recent()}>
              {(lane) => (
                <LaneStrip
                  lane={lane}
                  headline={headlines()[lane.id]}
                  repeated={repeated(lane)}
                  onOpen={() => props.fleet.setSelectedLaneId(lane.id)}
                />
              )}
            </For>
            <For each={prs()}>{(pr) => <PrStrip pr={pr} />}</For>
          </div>
        </Show>
      </div>
    </div>
  );
}
