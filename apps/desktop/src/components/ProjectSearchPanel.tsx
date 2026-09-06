import {
  For,
  Show,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
} from "solid-js";
import { Dynamic } from "solid-js/web";

import type { FileSearchHit } from "../bindings";
import type { EditorStore } from "../stores/editor";
import { daemonCall } from "../ipc/rpc";
import { groupSearchHits, type FileSearchGroup } from "./projectSearch";
import { getFileIcon } from "./fileIcons";
import {
  IconChevronDown,
  IconChevronRight,
  IconClose,
  IconRefresh,
  IconSearch,
} from "./icons";

export interface ProjectSearchPanelProps {
  editor: EditorStore;
  onReplace?: (query: string, replacement: string, regex: boolean, caseSensitive: boolean, all?: boolean) => void;
  compact?: boolean;
}

export default function ProjectSearchPanel(props: ProjectSearchPanelProps) {
  const [query, setQuery] = createSignal("");
  const [regex, setRegex] = createSignal(false);
  const [caseSensitive, setCaseSensitive] = createSignal(false);
  const [glob, setGlob] = createSignal("");
  const [replaceText, setReplaceText] = createSignal("");
  const [showReplace, setShowReplace] = createSignal(false);
  const [showGlob, setShowGlob] = createSignal(false);

  const [hits, setHits] = createSignal<FileSearchHit[]>([]);
  const [searching, setSearching] = createSignal(false);
  const [truncated, setTruncated] = createSignal(false);
  const [searchError, setSearchError] = createSignal<string | null>(null);
  const [collapsedFiles, setCollapsedFiles] = createSignal<Set<string>>(new Set());

  let debounceTimer: ReturnType<typeof setTimeout> | null = null;
  let searchInputRef: HTMLInputElement | undefined;

  const lane = () => props.editor.selectedLane();
  const laneId = () => lane()?.id ?? null;
  const activeFile = () => props.editor.activeFile();
  let searchRequestId = 0;

  function triggerSearch() {
    const q = query().trim();
    const id = laneId();
    const currentRequestId = ++searchRequestId;

    if (!q || id == null) {
      setHits([]);
      setSearching(false);
      setTruncated(false);
      setSearchError(null);
      return;
    }

    setSearching(true);
    setSearchError(null);

    void daemonCall("file.search", {
      lane_id: id,
      query: q,
      regex: regex(),
      case_sensitive: caseSensitive(),
      glob: glob().trim() || undefined,
      max_results: 2000,
    })
      .then((res) => {
        if (currentRequestId !== searchRequestId || laneId() !== id) return;
        setHits(res.hits);
        setTruncated(res.truncated);
        setSearching(false);
      })
      .catch((err) => {
        if (currentRequestId !== searchRequestId || laneId() !== id) return;
        setHits([]);
        setTruncated(false);
        setSearching(false);
        setSearchError(err instanceof Error ? err.message : String(err));
      });
  }

  createEffect(() => {

    const q = query();
    void regex();
    void caseSensitive();
    void glob();
    const id = laneId();

    if (debounceTimer) clearTimeout(debounceTimer);
    if (!q.trim() || id == null) {
      ++searchRequestId;
      setHits([]);
      setSearching(false);
      setTruncated(false);
      setSearchError(null);
      return;
    }

    setSearching(true);
    debounceTimer = setTimeout(() => {
      triggerSearch();
    }, 250);
  });

  onCleanup(() => {
    if (debounceTimer) clearTimeout(debounceTimer);
  });

  const groups = createMemo<FileSearchGroup[]>(() => {
    return groupSearchHits(hits());
  });

  function toggleFileCollapse(path: string) {
    setCollapsedFiles((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  function handleHitClick(hit: FileSearchHit) {
    void props.editor.openAt(hit.path, hit.line, hit.column);
  }

  function handleReplace(all: boolean) {
    const q = query().trim();
    if (!q) return;
    props.onReplace?.(q, replaceText(), regex(), caseSensitive(), all);
  }

  return (
    <div class="flex h-full min-w-0 flex-1 flex-col overflow-hidden bg-surface">

      <div class="flex flex-col gap-1.5 border-b border-line p-2">

        <div class="flex items-center gap-1">
          <div class="relative flex min-w-0 flex-1 items-center">
            <IconSearch size={13} class="pointer-events-none absolute left-2 text-muted" />
            <input
              ref={searchInputRef}
              type="text"
              aria-label="Search in project"
              class="focus-ring w-full rounded border border-line bg-background py-1 pr-7 pl-7 font-mono text-xs text-foreground placeholder:text-muted/60"
              placeholder="Search in project..."
              value={query()}
              onInput={(e) => setQuery(e.currentTarget.value)}
              spellcheck={false}
            />
            <Show when={query().length > 0}>
              <button
                type="button"
                class="absolute right-1.5 text-muted hover:text-foreground"
                onClick={() => setQuery("")}
                title="Clear"
              >
                <IconClose size={11} />
              </button>
            </Show>
          </div>

          <div class="flex shrink-0 items-center gap-0.5 rounded border border-line bg-background p-0.5">
            <button
              type="button"
              class={`flex size-5 items-center justify-center rounded font-mono text-[10px] font-semibold transition-colors ${
                caseSensitive()
                  ? "bg-signal/20 text-signal ring-1 ring-signal/40"
                  : "text-muted hover:text-foreground"
              }`}
              onClick={() => setCaseSensitive((v) => !v)}
              title="Match Case (Alt+C)"
              aria-label="Match case"
              aria-pressed={caseSensitive()}
            >
              Aa
            </button>
            <button
              type="button"
              class={`flex size-5 items-center justify-center rounded font-mono text-[11px] font-semibold transition-colors ${
                regex()
                  ? "bg-signal/20 text-signal ring-1 ring-signal/40"
                  : "text-muted hover:text-foreground"
              }`}
              onClick={() => setRegex((v) => !v)}
              title="Use Regular Expression (Alt+R)"
              aria-label="Use regular expression"
              aria-pressed={regex()}
            >
              .*
            </button>
          </div>
        </div>

        <div class="flex items-center justify-between px-0.5 text-[10px] text-muted">
          <button
            type="button"
            class="flex items-center gap-1 hover:text-foreground"
            onClick={() => setShowReplace((v) => !v)}
            aria-expanded={showReplace()}
          >
            <Show when={showReplace()} fallback={<IconChevronRight size={9} />}>
              <IconChevronDown size={9} />
            </Show>
            <span>Replace in file</span>
          </button>
          <button
            type="button"
            class="flex items-center gap-1 hover:text-foreground"
            onClick={() => setShowGlob((v) => !v)}
            aria-expanded={showGlob()}
          >
            <Show when={showGlob()} fallback={<IconChevronRight size={9} />}>
              <IconChevronDown size={9} />
            </Show>
            <span>Filter paths</span>
          </button>
        </div>

        <Show when={showReplace()}>
          <div class="flex flex-col gap-1 pt-1">
            <div class="relative flex items-center">
              <input
                type="text"
                class="focus-ring w-full rounded border border-line bg-background px-2 py-1 font-mono text-xs text-foreground placeholder:text-muted/60"
                placeholder="Replace in current file..."
                value={replaceText()}
                onInput={(e) => setReplaceText(e.currentTarget.value)}
                spellcheck={false}
              />
            </div>
            <div class="flex items-center justify-end gap-1.5">
              <button
                type="button"
                disabled={!activeFile() || !query().trim()}
                class="focus-ring rounded border border-line bg-surface px-2 py-0.5 text-[11px] font-medium text-muted transition-colors hover:bg-raised hover:text-foreground disabled:opacity-40"
                onClick={() => handleReplace(false)}
                title="Replace next occurrence in open file"
              >
                Replace
              </button>
              <button
                type="button"
                disabled={!activeFile() || !query().trim()}
                class="focus-ring rounded border border-line bg-surface px-2 py-0.5 text-[11px] font-medium text-muted transition-colors hover:bg-raised hover:text-foreground disabled:opacity-40"
                onClick={() => handleReplace(true)}
                title="Replace all occurrences in open file"
              >
                Replace All
              </button>
            </div>
          </div>
        </Show>

        <Show when={showGlob()}>
          <div class="pt-1">
            <input
              type="text"
              class="focus-ring w-full rounded border border-line bg-background px-2 py-1 font-mono text-xs text-foreground placeholder:text-muted/60"
              placeholder="e.g. *.rs, src/*"
              value={glob()}
              onInput={(e) => setGlob(e.currentTarget.value)}
              spellcheck={false}
            />
          </div>
        </Show>
      </div>

      <div class="flex items-center justify-between border-b border-line bg-surface/70 px-3 py-1 text-[11px] text-muted">
        <Show
          when={searching()}
          fallback={
            <span>
              <Show
                when={query().trim().length > 0}
                fallback="Type a query to search"
              >
                {hits().length} {hits().length === 1 ? "match" : "matches"} in{" "}
                {groups().length} {groups().length === 1 ? "file" : "files"}
              </Show>
            </span>
          }
        >
          <div class="flex items-center gap-1.5 text-signal">
            <IconRefresh size={11} class="animate-spin" />
            <span>Searching...</span>
          </div>
        </Show>

        <Show when={truncated()}>
          <span class="rounded bg-attention/15 px-1.5 py-0.2 text-[10px] font-medium text-attention">
            2,000 cap reached
          </span>
        </Show>
      </div>

      <Show when={searchError()}>
        <div class="border-b border-fault/30 bg-fault/10 p-2 text-xs text-fault">
          {searchError()}
        </div>
      </Show>

      <div class="flex-1 overflow-y-auto p-1 text-xs outline-none">
        <Show
          when={groups().length > 0}
          fallback={
            <Show when={query().trim().length > 0 && !searching()}>
              <div class="py-8 text-center text-xs text-muted">
                No matching results found
              </div>
            </Show>
          }
        >
          <For each={groups()}>
            {(group) => {
              const isCollapsed = () => collapsedFiles().has(group.path);
              const slashIdx = group.path.lastIndexOf("/");
              const basename = slashIdx >= 0 ? group.path.slice(slashIdx + 1) : group.path;
              const dirname = slashIdx >= 0 ? group.path.slice(0, slashIdx) : "";
              const Icon = getFileIcon(group.path);

              return (
                <div class="mb-1">

                  <button
                    type="button"
                    class="focus-ring flex w-full items-center justify-between rounded px-1.5 py-1 text-left font-medium text-foreground hover:bg-raised/60"
                    onClick={() => toggleFileCollapse(group.path)}
                    aria-expanded={!isCollapsed()}
                    title={group.path}
                  >
                    <div class="flex min-w-0 flex-1 items-center gap-1.5">
                      <span class="size-3 shrink-0 text-muted/60">
                        <Show when={isCollapsed()} fallback={<IconChevronDown size={10} />}>
                          <IconChevronRight size={10} />
                        </Show>
                      </span>
                      <span class="size-3.5 shrink-0 text-muted">
                        <Dynamic component={Icon} size={13} />
                      </span>
                      <span class="min-w-0 truncate font-mono text-xs font-medium text-foreground">
                        {basename}
                      </span>
                      <Show when={dirname.length > 0}>
                        <span class="truncate font-mono text-[10px] text-muted/70">
                          {dirname}
                        </span>
                      </Show>
                    </div>
                    <span class="shrink-0 rounded-full bg-raised px-1.5 py-0.2 font-mono text-[10px] text-muted">
                      {group.hits.length}
                    </span>
                  </button>

                  <Show when={!isCollapsed()}>
                    <div class="flex flex-col pl-4">
                      <For each={group.hits}>
                        {(hit) => (
                          <button
                            type="button"
                            class="focus-ring flex w-full items-start gap-2 rounded px-2 py-0.5 text-left font-mono text-[11px] text-foreground/80 hover:bg-raised/40 hover:text-foreground"
                            onClick={() => handleHitClick(hit)}
                            title={`${hit.path}:${hit.line}:${hit.column}`}
                          >
                            <span class="shrink-0 text-muted/70 text-[10px]">
                              {hit.line}:{hit.column}
                            </span>
                            <span class="min-w-0 flex-1 truncate text-foreground/90">
                              {hit.preview}
                            </span>
                          </button>
                        )}
                      </For>
                    </div>
                  </Show>
                </div>
              );
            }}
          </For>
        </Show>
      </div>
    </div>
  );
}
