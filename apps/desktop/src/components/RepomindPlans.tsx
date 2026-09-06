import { For, Show, createEffect, createSignal, onCleanup } from "solid-js";

import { daemonCall } from "../ipc/rpc";
import { addGoal as writeRepomindGoal } from "../stores/repomind";
import { IconPlus } from "./icons";
import {
  InlineField,
  InlineForm,
  Section,
  SectionButton,
  SectionNote,
  sinceLabel,
} from "./RepomindSection";
import { donePlanDocument, planSlug, readPlanSummary, type PlanSummary } from "./repomindDocs";

/// Configures file-backed active goals and their creation and completion actions.
export interface RepomindPlansProps {
  /// The controller lane. Null means the home has no lane yet and the board is not rendered.
  laneId: number;
  /// Opens a home-relative path in the editor on the home lane.
  onOpen: (path: string) => void;
  /// Something the home's counts depend on changed, so the status poll should catch up now.
  onChanged?: () => void;
  /// Bumped by the panel when the home's plan count moves under us (an agent wrote a file), so
  /// the board re-reads without polling the directory on every heartbeat.
  revision?: number;
}

const ACTIVE_DIR = "plans/active";
const DONE_DIR = "plans/done";

function errorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

export default function RepomindPlans(props: RepomindPlansProps) {
  const [plans, setPlans] = createSignal<PlanSummary[]>([]);
  const [error, setError] = createSignal<string | null>(null);
  const [note, setNote] = createSignal<string | null>(null);
  const [adding, setAdding] = createSignal(false);
  const [title, setTitle] = createSignal("");
  const [intent, setIntent] = createSignal("");
  const [closing, setClosing] = createSignal<string | null>(null);
  const [outcome, setOutcome] = createSignal("");
  const [busy, setBusy] = createSignal<string | null>(null);

  let live = true;
  // Newest read wins: a slow directory walk must never overwrite a later one's results.
  let token = 0;
  onCleanup(() => {
    live = false;
  });

  async function load(lane: number) {
    const mine = ++token;
    try {
      const listing = await daemonCall("file.list", { lane_id: lane, path: ACTIVE_DIR });
      const files = listing.entries.filter(
        (entry) =>
          !entry.is_dir &&
          entry.name.toLowerCase().endsWith(".md") &&
          // A directory's own README is its guide, never a goal.
          entry.name.toLowerCase() !== "readme.md",
      );
      const read = await Promise.all(
        files.map(async (entry) => {
          const file = await daemonCall("file.read", { lane_id: lane, path: entry.path });
          return readPlanSummary(entry.path, file.content);
        }),
      );
      if (!live || mine !== token) return;
      setPlans([...read].sort((a, b) => a.title.localeCompare(b.title)));
      setError(null);
    } catch (cause) {
      if (!live || mine !== token) return;
      setPlans([]);
      setError(errorMessage(cause));
    }
  }

  createEffect(() => {
    const lane = props.laneId;
    props.revision;
    void load(lane);
  });

  function reset() {
    setAdding(false);
    setTitle("");
    setIntent("");
    setClosing(null);
    setOutcome("");
  }

  /// Notify first to determine ownership, then write once; an unavailable controller must not
  /// prevent saving an unassigned goal.
  async function addGoal() {
    const name = title().trim();
    if (!name) return;
    setBusy("add");
    setError(null);
    setNote(null);
    try {
      const { path, told } = await writeRepomindGoal(props.laneId, name, intent().trim());
      reset();
      if (live) {
        setNote(
          told
            ? `Added ${path} and told the controller.`
            : `Added ${path}. No controller is running; start Repomind and it will pick this up at boot.`,
        );
      }
      await load(props.laneId);
      props.onChanged?.();
    } catch (cause) {
      if (live) setError(errorMessage(cause));
    } finally {
      if (live) setBusy(null);
    }
  }

  /// Move a finished goal into `plans/done` and stamp it with the operator's one-line outcome.
  /// The move happens first: a plan that reached `done/` without its outcome is a smaller problem
  /// than one still listed as active while its file says it is finished.
  async function finishPlan(plan: PlanSummary) {
    const line = outcome().trim();
    if (!line) return;
    const to = `${DONE_DIR}/${planSlug(plan.path)}.md`;
    setBusy(`done:${plan.path}`);
    setError(null);
    setNote(null);
    try {
      const file = await daemonCall("file.read", { lane_id: props.laneId, path: plan.path });
      await daemonCall("file.rename", { lane_id: props.laneId, from: plan.path, to });
      await daemonCall("file.write", {
        lane_id: props.laneId,
        path: to,
        content: donePlanDocument(file.content, line),
      });
      reset();
      if (live) setNote(`Moved ${plan.title} to ${to}.`);
      await load(props.laneId);
      props.onChanged?.();
    } catch (cause) {
      if (live) setError(errorMessage(cause));
    } finally {
      if (live) setBusy(null);
    }
  }

  return (
    <Section
      title="Plans"
      detail={String(plans().length)}
      action={
        <button
          type="button"
          class="focus-ring flex items-center gap-1 rounded border border-line bg-raised/50 px-1.5 py-0.5 font-mono text-[10px] text-muted transition-colors hover:bg-raised hover:text-foreground"
          onClick={() => {
            setClosing(null);
            setAdding((open) => !open);
          }}
          aria-expanded={adding()}
        >
          <IconPlus size={9} />
          <span>Add goal</span>
        </button>
      }
    >
      <Show when={adding()}>
        <InlineForm
          label="Add goal"
          submitLabel="Add goal"
          busy={busy() === "add"}
          canSubmit={Boolean(title().trim())}
          onSubmit={() => void addGoal()}
          onCancel={reset}
        >
          <InlineField
            label="Goal title"
            placeholder="Ship the control room"
            value={title()}
            autofocus
            onInput={setTitle}
          />
          <InlineField
            label="Next step"
            placeholder="What happens next (optional - defaults to the title)"
            value={intent()}
            onInput={setIntent}
          />
        </InlineForm>
      </Show>

      <Show when={error()}>{(message) => <SectionNote tone="fault">{message()}</SectionNote>}</Show>
      <Show when={note()}>
        {(message) => <p class="mb-1.5 text-[11px] leading-relaxed text-muted">{message()}</p>}
      </Show>

      <Show
        when={plans().length}
        fallback={
          <Show when={!error()}>
            <SectionNote>
              No goals in flight. A goal is one file in {ACTIVE_DIR} with a title, an owner, and a
              next step line. Add one here, or write the file yourself and it shows up.
            </SectionNote>
          </Show>
        }
      >
        <ul class="space-y-0.5">
          <For each={plans()}>
            {(plan) => (
              <li>
                <div class="group/plan flex items-start gap-1.5 rounded-md px-1.5 py-1 transition-colors hover:bg-raised/50">
                  <button
                    type="button"
                    class="focus-ring min-w-0 flex-1 text-left"
                    onClick={() => props.onOpen(plan.path)}
                    title={`Open ${plan.path}`}
                  >
                    <span class="block truncate text-xs font-medium text-foreground">
                      {plan.title}
                    </span>
                    <Show when={plan.nextStep}>
                      <span class="mt-0.5 block truncate text-[11px] text-muted">
                        Next: {plan.nextStep}
                      </span>
                    </Show>
                    <Show when={plan.owner || plan.updated}>
                      <span class="mt-0.5 block truncate font-mono text-[10px] text-muted/70">
                        {plan.owner ?? "unassigned"}
                        <Show when={plan.updated}> · {sinceLabel(plan.updated)}</Show>
                      </span>
                    </Show>
                  </button>
                  <SectionButton
                    label="Done"
                    title={`Close ${plan.title} out and move it to ${DONE_DIR}`}
                    onClick={() => {
                      setAdding(false);
                      setOutcome("");
                      setClosing((open) => (open === plan.path ? null : plan.path));
                    }}
                  />
                </div>
                <Show when={closing() === plan.path}>
                  <InlineForm
                    label={`Close ${plan.title}`}
                    submitLabel="Move to done"
                    busy={busy() === `done:${plan.path}`}
                    canSubmit={Boolean(outcome().trim())}
                    onSubmit={() => void finishPlan(plan)}
                    onCancel={reset}
                  >
                    <InlineField
                      label="Outcome"
                      placeholder="How it ended, in one line"
                      value={outcome()}
                      autofocus
                      onInput={setOutcome}
                    />
                  </InlineForm>
                </Show>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </Section>
  );
}
