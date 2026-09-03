import {
  For,
  Show,
  createEffect,
  createMemo,
  createSignal,
} from "solid-js";
import { Dynamic } from "solid-js/web";

import type { EditorStore } from "../stores/editor";
import { daemonCall } from "../ipc/rpc";
import { filterAndRankPaths, type FuzzyMatch } from "./fuzzyScorer";
import { getFileIcon } from "./fileIcons";
import { IconClose, IconRefresh, IconSearch } from "./icons";

export interface FileFinderProps {
  editor: EditorStore;
  isOpen: boolean;
  onClose: () => void;
  onOpenPath?: (path: string) => void;
}

function HighlightedText(props: { text: string; offset: number; matchIndices: Set<number> }) {
  const runs = createMemo(() => {
    if (props.matchIndices.size === 0) {
      return [{ text: props.text, isMatch: false }];
    }
    const result: Array<{ text: string; isMatch: boolean }> = [];
    let currentText = "";
    let currentMatch: boolean | null = null;

    for (let i = 0; i < props.text.length; i++) {
      const isMatch = props.matchIndices.has(props.offset + i);
      if (currentMatch === null) {
        currentMatch = isMatch;
        currentText = props.text[i];
      } else if (currentMatch === isMatch) {
        currentText += props.text[i];
      } else {
        result.push({ text: currentText, isMatch: currentMatch });
        currentMatch = isMatch;
        currentText = props.text[i];
      }
    }
    if (currentText.length > 0 && currentMatch !== null) {
      result.push({ text: currentText, isMatch: currentMatch });
    }
    return result;
  });

  return (
    <span>
      <For each={runs()}>
        {(run) => (
          <span
            class={
              run.isMatch
                ? "font-semibold text-signal underline decoration-signal/40"
                : "text-inherit"
            }
          >
            {run.text}
          </span>
        )}
      </For>
    </span>
  );
}

export default function FileFinder(props: FileFinderProps) {
  const [query, setQuery] = createSignal("");
  const [selectedIndex, setSelectedIndex] = createSignal(0);
  const [paths, setPaths] = createSignal<string[]>([]);
  const [loading, setLoading] = createSignal(false);

  let inputRef: HTMLInputElement | undefined;
  let listRef: HTMLDivElement | undefined;

  const currentLane = () => props.editor.selectedLane();
  const currentLaneId = () => currentLane()?.id ?? null;

  async function fetchIndex(laneId: number) {
    setLoading(true);
    try {
      const res = await daemonCall("file.index", { lane_id: laneId });
      setPaths(res.paths);
    } catch {
      setPaths([]);
    } finally {
      setLoading(false);
    }
  }

  createEffect(() => {
    if (props.isOpen) {
      const id = currentLaneId();
      if (id != null) {
        void fetchIndex(id);
      } else {
        setPaths([]);
      }
      setQuery("");
      setSelectedIndex(0);
      queueMicrotask(() => {
        inputRef?.focus();
        inputRef?.select();
      });
    }
  });

  const matches = createMemo<FuzzyMatch[]>(() => {
    return filterAndRankPaths(paths(), query(), 50);
  });

  // Reset selected index when matches change
  createEffect(() => {
    matches();
    setSelectedIndex(0);
  });

  function selectPath(path: string) {
    props.onClose();
    if (props.onOpenPath) {
      props.onOpenPath(path);
    } else {
      void props.editor.openFile(path);
    }
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      props.onClose();
      return;
    }

    const items = matches();
    if (items.length === 0) return;

    if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) {
      e.preventDefault();
      setSelectedIndex((prev) => {
        const next = prev < items.length - 1 ? prev + 1 : 0;
        scrollIndexIntoView(next);
        return next;
      });
    } else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p")) {
      e.preventDefault();
      setSelectedIndex((prev) => {
        const next = prev > 0 ? prev - 1 : items.length - 1;
        scrollIndexIntoView(next);
        return next;
      });
    } else if (e.key === "Enter") {
      e.preventDefault();
      const item = items[selectedIndex()];
      if (item) {
        selectPath(item.path);
      }
    }
  }

  function scrollIndexIntoView(index: number) {
    if (!listRef) return;
    const element = listRef.children[index] as HTMLElement | undefined;
    if (element && typeof element.scrollIntoView === "function") {
      element.scrollIntoView({ block: "nearest" });
    }
  }

  return (
    <Show when={props.isOpen}>
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Go to file"
        class="fixed inset-0 z-50 flex items-start justify-center bg-background/60 pt-[12vh] backdrop-blur-sm"
        onClick={(e) => {
          if (e.target === e.currentTarget) {
            props.onClose();
          }
        }}
      >
        <div
          class="flex max-h-[70vh] w-[600px] max-w-[94vw] flex-col overflow-hidden rounded-xl border border-line bg-surface shadow-2xl"
          onKeyDown={handleKeyDown}
        >
          {/* Header input bar */}
          <div class="relative flex items-center border-b border-line px-3 py-2.5">
            <Show
              when={loading()}
              fallback={<IconSearch size={16} class="shrink-0 text-muted" />}
            >
              <IconRefresh size={16} class="shrink-0 animate-spin text-signal" />
            </Show>
            <input
              ref={inputRef}
              type="text"
              class="focus-ring ml-2 flex-1 bg-transparent font-mono text-sm text-foreground placeholder:text-muted/60 focus:outline-none"
              placeholder="Search files by name..."
              value={query()}
              onInput={(e) => setQuery(e.currentTarget.value)}
              spellcheck={false}
              autocomplete="off"
            />
            <div class="flex items-center gap-1.5 pl-2">
              <Show when={query().length > 0}>
                <button
                  type="button"
                  class="flex size-5 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground"
                  onClick={() => setQuery("")}
                  title="Clear query"
                >
                  <IconClose size={12} />
                </button>
              </Show>
              <kbd class="rounded border border-line/60 bg-raised/50 px-1.5 py-0.5 font-mono text-[10px] text-muted">
                ESC
              </kbd>
            </div>
          </div>

          {/* Results list */}
          <div
            ref={listRef}
            class="flex-1 overflow-y-auto p-1.5 outline-none"
            tabIndex={-1}
          >
            <Show
              when={matches().length > 0}
              fallback={
                <div class="px-4 py-8 text-center text-xs text-muted">
                  <Show when={!loading()} fallback="Indexing worktree files...">
                    No files found matching "{query()}"
                  </Show>
                </div>
              }
            >
              <For each={matches()}>
                {(match, idx) => {
                  const isSelected = () => idx() === selectedIndex();
                  const slashIdx = match.path.lastIndexOf("/");
                  const basename = slashIdx >= 0 ? match.path.slice(slashIdx + 1) : match.path;
                  const dirname = slashIdx >= 0 ? match.path.slice(0, slashIdx) : "";
                  const basenameOffset = slashIdx >= 0 ? slashIdx + 1 : 0;
                  const matchSet = new Set(match.indices);
                  const Icon = getFileIcon(match.path);

                  return (
                    <button
                      type="button"
                      class={`flex w-full items-center justify-between rounded-lg px-2.5 py-1.5 text-left text-xs transition-colors ${
                        isSelected()
                          ? "bg-raised text-foreground ring-1 ring-line"
                          : "text-foreground/80 hover:bg-raised/50 hover:text-foreground"
                      }`}
                      onClick={() => selectPath(match.path)}
                      onMouseEnter={() => setSelectedIndex(idx())}
                      title={match.path}
                    >
                      <div class="flex min-w-0 items-center gap-2">
                        <span class="size-4 shrink-0 text-muted">
                          <Dynamic component={Icon} size={14} />
                        </span>
                        <span class="font-mono text-xs text-foreground">
                          <HighlightedText
                            text={basename}
                            offset={basenameOffset}
                            matchIndices={matchSet}
                          />
                        </span>
                        <Show when={dirname.length > 0}>
                          <span class="truncate font-mono text-[11px] text-muted/70">
                            <HighlightedText
                              text={dirname}
                              offset={0}
                              matchIndices={matchSet}
                            />
                          </span>
                        </Show>
                      </div>
                      <Show when={isSelected()}>
                        <kbd class="shrink-0 font-mono text-[10px] text-muted">↵</kbd>
                      </Show>
                    </button>
                  );
                }}
              </For>
            </Show>
          </div>

          {/* Footer status */}
          <div class="flex items-center justify-between border-t border-line bg-surface/80 px-3 py-1 text-[11px] text-muted">
            <span>
              {matches().length} of {paths().length} files
            </span>
            <div class="flex items-center gap-2 font-mono text-[10px]">
              <span>↑↓ navigate</span>
              <span>↵ open</span>
            </div>
          </div>
        </div>
      </div>
    </Show>
  );
}
