import { For, Show, createEffect, createSignal, onCleanup } from "solid-js";

import { daemonCall } from "../ipc/rpc";
import type { RepomindStore } from "../stores/repomind";
import Select from "./controls/Select";
import { IconChevronDown, IconChevronRight } from "./icons";
import { Section, SectionButton, SectionNote, sinceLabel } from "./RepomindSection";
import { journalDays, journalEntries, journalPathFor, type JournalDay } from "./repomindDocs";

/// Memory health: whether the context repomind boots with is current, whether the daemon's export
/// into the home is keeping up, and what actually got written down.
///
/// One section rather than three, because the three answer one question - is the memory in good
/// order - and splitting them put the journal, the only part with real content in it, three
/// headings away from the two lines that say whether it is being written at all.
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

  let live = true;
  // Newest read wins, one guard per read: the listing and the day's content resolve separately.
  let listToken = 0;
  let dayToken = 0;
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
