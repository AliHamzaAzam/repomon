import { For, Show, createEffect, createSignal, onCleanup } from "solid-js";

import type { Schedule } from "../bindings";
import { daemonCall } from "../ipc/rpc";
import { Section, SectionButton, SectionNote, sinceLabel, untilLabel } from "./RepomindSection";

/// Standing duties: the schedules that run repomind without anyone starting it.
///
/// The panel reads and removes them, and sends adding one to Settings > Automation > Schedules
/// rather than growing a second form. A spec, a goal, and an action cap deserve the room that
/// surface gives them, and two forms for one record is how the two drift apart.
export interface RepomindDutiesProps {
  /// Opens Settings on the Automation tab, already on Schedules.
  onAdd?: () => void;
  /// Bumped by the panel so the list re-reads when the home changes under us.
  revision?: number;
}

type ScheduleRow = Schedule & { next_run?: string };

function errorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

export default function RepomindDuties(props: RepomindDutiesProps) {
  const [duties, setDuties] = createSignal<ScheduleRow[]>([]);
  const [error, setError] = createSignal<string | null>(null);
  const [confirming, setConfirming] = createSignal<number | null>(null);
  const [busy, setBusy] = createSignal<number | null>(null);

  let live = true;
  // Newest read wins.
  let token = 0;
  onCleanup(() => {
    live = false;
  });

  async function load() {
    const mine = ++token;
    try {
      const result = await daemonCall("schedule.list");
      if (!live || mine !== token) return;
      setDuties(result.schedules);
      setError(null);
    } catch (cause) {
      if (!live || mine !== token) return;
      setDuties([]);
      setError(errorMessage(cause));
    }
  }

  createEffect(() => {
    props.revision;
    void load();
  });

  async function remove(id: number) {
    setBusy(id);
    setError(null);
    try {
      await daemonCall("schedule.remove", { id });
      setConfirming(null);
      await load();
    } catch (cause) {
      if (live) setError(errorMessage(cause));
    } finally {
      if (live) setBusy(null);
    }
  }

  return (
    <Section
      title="Standing duties"
      detail={String(duties().length)}
      action={<SectionButton label="Add" title="Open Settings > Automation > Schedules" onClick={() => props.onAdd?.()} />}
    >
      <Show when={error()}>{(message) => <SectionNote tone="fault">{message()}</SectionNote>}</Show>

      <Show
        when={duties().length}
        fallback={
          <Show when={!error()}>
            <SectionNote>
              No standing duties. A duty is a spec, a goal, and an action cap that runs repomind on
              a timer, unattended and more conservatively than you would.
            </SectionNote>
          </Show>
        }
      >
        <ul class="space-y-1">
          <For each={duties()}>
            {(duty) => (
              <li class="rounded-md px-1.5 py-1 transition-colors hover:bg-raised/50">
                <div class="flex items-center gap-1.5">
                  <span class="shrink-0 font-mono text-[10px] font-semibold text-foreground">
                    {duty.spec}
                  </span>
                  <span class="min-w-0 flex-1 truncate text-xs text-muted" title={duty.prompt}>
                    {duty.prompt}
                  </span>
                  <Show
                    when={confirming() === duty.id}
                    fallback={
                      <SectionButton
                        label="Remove"
                        title={`Stop running "${duty.prompt}" on ${duty.spec}`}
                        onClick={() => setConfirming(duty.id)}
                      />
                    }
                  >
                    <SectionButton
                      label="Confirm"
                      tone="fault"
                      busy={busy() === duty.id}
                      onClick={() => void remove(duty.id)}
                    />
                    <SectionButton label="Keep" onClick={() => setConfirming(null)} />
                  </Show>
                </div>
                <p class="mt-0.5 font-mono text-[10px] text-muted/70">
                  cap {duty.max_actions} · ran {sinceLabel(duty.last_run_at)} · next{" "}
                  {untilLabel(duty.next_run)}
                </p>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </Section>
  );
}
