import { For, Show, createEffect, createSignal, onCleanup } from "solid-js";

import type { JournalEntry } from "../bindings";
import { daemonCall } from "../ipc/rpc";
import type { RepomindStore } from "../stores/repomind";
import { formatTime, journalQueryParams } from "./automation";
import Select from "./controls/Select";
import { IconChevronDown, IconChevronRight } from "./icons";
import { Section, SectionButton, SectionNote, sinceLabel } from "./RepomindSection";
import { journalDays, journalEntries, journalPathFor, type JournalDay } from "./repomindDocs";

/// Configures the combined boot-context, export-health, and journal view.
export interface RepomindMemoryProps {
  laneId: number;
  repomind?: RepomindStore;
  /// Opens a home-relative path in the editor on the home lane.
  onOpen: (path: string) => void;
}

/// The daemon-owned boot document, opened from the boot line.
export const BOOT_PATH = ".repomind/boot.md";

const JOURNAL_DIR = "journal";
const ARCHIVE_DIR = "journal/archive";

function errorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

export default function RepomindMemory(props: RepomindMemoryProps) {
  const [days, setDays] = createSignal<JournalDay[]>([]);
  const [archive, setArchive] = createSignal<JournalDay[]>([]);
  const [showArchive, setShowArchive] = createSignal(false);
  const [selected, setSelected] = createSignal<string | null>(null);
  const [entries, setEntries] = createSignal<string[]>([]);
  const [error, setError] = createSignal<string | null>(null);

  const [showActivity, setShowActivity] = createSignal(false);
  const [activity, setActivity] = createSignal<JournalEntry[]>([]);
  const [activityError, setActivityError] = createSignal<string | null>(null);

  let live = true;
  // Newest read wins, one guard per read: the listing and the day's content resolve separately.
  let listToken = 0;
  let dayToken = 0;
  let activityToken = 0;
  onCleanup(() => {
    live = false;
  });

  const home = () => props.repomind?.status() ?? null;
  const boot = () => home()?.boot ?? null;
  const exportState = () => home()?.export ?? null;

  async function listDays(lane: number) {
    const mine = ++listToken;
    try {
      const [recent, older] = await Promise.all([
        daemonCall("file.list", { lane_id: lane, path: JOURNAL_DIR }),
        daemonCall("file.list", { lane_id: lane, path: ARCHIVE_DIR }).catch(() => ({
          entries: [],
          truncated: false,
        })),
      ]);
      if (!live || mine !== listToken) return;
      const split = journalDays(
        [...recent.entries, ...older.entries].filter((entry) => !entry.is_dir).map((entry) => entry.path),
      );
      setDays(split.days);
      setArchive(split.archive);
      setError(null);
      // Default to today when the export has written it, else the newest day there is.
      const today = journalPathFor(new Date());
      const current = selected();
      const known = [...split.days, ...split.archive].some((day) => day.path === current);
      if (!known) {
        setSelected(
          split.days.find((day) => day.path === today)?.path ?? split.days[0]?.path ?? null,
        );
      }
    } catch (cause) {
      if (!live || mine !== listToken) return;
      setDays([]);
      setArchive([]);
      setError(errorMessage(cause));
    }
  }

  async function readDay(lane: number, path: string) {
    const mine = ++dayToken;
    try {
      const file = await daemonCall("file.read", { lane_id: lane, path });
      if (!live || mine !== dayToken) return;
      setEntries(journalEntries(file.content));
    } catch {
      // A day file that vanished between the listing and the read is not a fault; the listing
      // will catch up on the next pass.
      if (live && mine === dayToken) setEntries([]);
    }
  }

  createEffect(() => {
    const lane = props.laneId;
    home()?.export.last_run;
    void listDays(lane);
  });

  async function readActivity() {
    const mine = ++activityToken;
    try {
      const result = await daemonCall("journal.query", journalQueryParams(""));
      if (!live || mine !== activityToken) return;
      setActivity(result.entries ?? []);
      setActivityError(null);
    } catch (cause) {
      if (!live || mine !== activityToken) return;
      setActivity([]);
      setActivityError(errorMessage(cause));
    }
  }

  createEffect(() => {
    if (!showActivity()) return;
    void readActivity();
  });

  createEffect(() => {
    const lane = props.laneId;
    const path = selected();
    if (!path) {
      setEntries([]);
      return;
    }
    void readDay(lane, path);
  });

  const dayOptions = () =>
    [...days(), ...(showArchive() ? archive() : [])].map((day) => ({
      value: day.path,
      label: day.archived ? `${day.label} (archived)` : day.label,
    }));

  return (
    <Section title="Memory" detail={boot()?.generated_at ? `${boot()?.tokens_estimate} tokens` : undefined}>
      <div class="space-y-2.5">
        <div>
          <div class="flex items-center gap-1.5">
            <span class="min-w-0 flex-1 truncate text-xs text-foreground">
              Boot context assembled {sinceLabel(boot()?.generated_at)}
            </span>
            <SectionButton
              label="Regenerate"
              busy={props.repomind?.busy() === "boot"}
              title="Reassemble the context a controller would start with right now"
              onClick={() => void props.repomind?.regenerateBoot()}
            />
            <SectionButton label="Open" title={`Open ${BOOT_PATH}`} onClick={() => props.onOpen(BOOT_PATH)} />
          </div>
          <Show when={boot()?.trimmed.length}>
            <p class="mt-1 text-[11px] leading-relaxed text-attention">
              The token budget left out: {boot()?.trimmed.join(", ")}
            </p>
          </Show>
        </div>

        <div>
          <div class="flex items-center gap-1.5">
            <span class="min-w-0 flex-1 truncate text-xs text-foreground">
              Export ran {sinceLabel(exportState()?.last_run)}
              <Show when={exportState()?.pending}>
                <span class="ml-1 font-mono text-[10px] text-muted">one pending</span>
              </Show>
            </span>
            <SectionButton
              label="Export now"
              busy={props.repomind?.busy() === "export"}
              title="Run the one-way export now instead of waiting out its debounce"
              onClick={() => void props.repomind?.runExport()}
            />
          </div>
          <Show when={exportState()?.last_error}>
            <p class="mt-1 text-[11px] leading-relaxed text-fault">{exportState()?.last_error}</p>
          </Show>
        </div>

        <div class="border-t border-line/70 pt-2.5">
          <div class="mb-1.5 flex items-center gap-1.5">
            <span class="section-label shrink-0">Journal</span>
            <SectionButton
              label="Activity"
              title="Browse what the daemon actually did, newest first"
              onClick={() => setShowActivity((open) => !open)}
            />
            <Show when={dayOptions().length}>
              <div class="min-w-0 flex-1">
                <Select
                  ariaLabel="Journal day"
                  size="sm"
                  variant="frameless"
                  align="right"
                  value={selected() ?? ""}
                  options={dayOptions()}
                  onChange={setSelected}
                />
              </div>
            </Show>
            <Show when={selected()}>
              {(path) => (
                <SectionButton label="Open" title={`Open ${path()}`} onClick={() => props.onOpen(path())} />
              )}
            </Show>
          </div>

          <Show when={showActivity()}>
            <div class="mb-2 rounded-lg border border-line bg-raised/30 p-2">
              <p class="section-label mb-1">Activity journal</p>
              <Show when={activityError()}>
                {(message) => <SectionNote tone="fault">{message()}</SectionNote>}
              </Show>
              <Show
                when={activity().length}
                fallback={
                  <Show when={!activityError()}>
                    <SectionNote>Nothing journaled yet.</SectionNote>
                  </Show>
                }
              >
                <ul class="max-h-48 space-y-1 overflow-y-auto pr-1">
                  <For each={activity()}>
                    {(entry) => (
                      <li class="flex items-baseline gap-1.5 font-mono text-[10px] leading-relaxed">
                        <span
                          class={entry.outcome === "ok" ? "shrink-0 text-signal" : "shrink-0 text-fault"}
                        >
                          {entry.outcome}
                        </span>
                        <span class="min-w-0 flex-1 truncate text-foreground" title={entry.detail ?? undefined}>
                          {entry.action}
                          <Show when={entry.repo}>{(repo) => <span class="text-muted"> · {repo()}</span>}</Show>
                        </span>
                        <span class="shrink-0 text-muted/80">{formatTime(entry.at)}</span>
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
            </div>
          </Show>

          <Show when={archive().length}>
            <button
              type="button"
              class="focus-ring mb-1.5 flex items-center gap-1 rounded font-mono text-[10px] text-muted transition-colors hover:text-foreground"
              aria-expanded={showArchive()}
              onClick={() => setShowArchive((open) => !open)}
            >
              {showArchive() ? <IconChevronDown size={9} /> : <IconChevronRight size={9} />}
              <span>
                {archive().length} archived {archive().length === 1 ? "month" : "months"}
              </span>
            </button>
          </Show>

          <Show when={error()}>{(message) => <SectionNote tone="fault">{message()}</SectionNote>}</Show>

          <Show
            when={entries().length}
            fallback={
              <Show when={!error()}>
                <SectionNote>
                  {days().length
                    ? "Nothing in this day's digest."
                    : "No journal yet. The daemon exports one day file per day of fleet activity."}
                </SectionNote>
              </Show>
            }
          >
            <ul class="space-y-1.5">
              <For each={entries()}>
                {(entry) => (
                  <li class="whitespace-pre-wrap break-words border-l border-line pl-2 font-mono text-[10px] leading-relaxed text-muted">
                    {entry}
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </div>
      </div>
    </Section>
  );
}
