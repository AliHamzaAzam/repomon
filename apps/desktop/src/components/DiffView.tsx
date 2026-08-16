import { For, Show, createEffect, createMemo, createSignal } from "solid-js";

import { IconChevronDown, IconChevronRight, IconClose } from "./icons";

/// Change types a `diff --git` block can describe. Binary is tracked as a separate flag on
/// `DiffFile` (not folded into this union) since a binary file can also be an add, delete, or
/// rename; the type/binary axes are independent.
export type DiffChangeType = "add" | "delete" | "modify" | "rename";

export interface DiffLine {
  type: "context" | "add" | "remove";
  text: string;
}

export interface DiffHunk {
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  /// Trailing "section heading" text on the `@@ ... @@` line (often a function signature);
  /// empty string when git didn't emit one.
  heading: string;
  lines: DiffLine[];
}

export interface DiffFile {
  oldPath: string;
  newPath: string;
  /// Display path: `newPath`, or `oldPath` for the rare case a file lost its new path (a
  /// truncated tail cutting off before the `+++` line).
  path: string;
  /// Set only for renames (from `rename from`/`rename to`), holding the pre-rename path.
  renamedFrom?: string;
  changeType: DiffChangeType;
  binary: boolean;
  hunks: DiffHunk[];
  /// Sums of `+`/`-` hunk lines across the file; always 0 for binary files (no hunks parsed).
  adds: number;
  dels: number;
}

function stripAB(path: string): string {
  return path.replace(/^[ab]\//, "");
}

const HUNK_HEADER_RE = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)$/;
const DIFF_GIT_RE = /^diff --git a\/(.*) b\/(.*)$/;
const BINARY_FILES_RE = /^Binary files (.+) and (.+) differ$/;

/// Parses a raw unified diff (as returned by `lane.diff`'s `patch` field, i.e. `git diff HEAD` -
/// see `diff_patch()` in crates/repomon-core/src/git/diff.rs) into structured per-file records.
///
/// This is a best-effort line-by-line scanner, not a strict grammar: `patch` is capped by
/// character count server-side (`cap_chars`, not line- or hunk-aware), so the tail of a large
/// diff can be cut mid-line or mid-hunk. The parser never throws on malformed input - a line it
/// doesn't recognize inside a hunk is dropped, and a chopped-off final hunk simply renders
/// whatever lines it captured before the cut.
export function parseDiff(raw: string): DiffFile[] {
  const files: DiffFile[] = [];
  let current: DiffFile | null = null;
  let currentHunk: DiffHunk | null = null;
  // "GIT binary patch" is followed by base85-encoded blob lines with no fixed terminator other
  // than the next file's `diff --git` header (or EOF) - skip everything until then.
  let skippingBinaryBlob = false;

  function finalizeCurrent() {
    if (!current) return;
    current.path = current.newPath || current.oldPath;
    if (current.changeType === "rename" && current.oldPath && current.oldPath !== current.newPath) {
      current.renamedFrom = current.oldPath;
    }
    for (const hunk of current.hunks) {
      for (const line of hunk.lines) {
        if (line.type === "add") current.adds += 1;
        else if (line.type === "remove") current.dels += 1;
      }
    }
    files.push(current);
  }

  for (const line of raw.split("\n")) {
    if (line.startsWith("diff --git ")) {
      finalizeCurrent();
      const m = line.match(DIFF_GIT_RE);
      current = {
        oldPath: m ? m[1] : "",
        newPath: m ? m[2] : "",
        path: "",
        changeType: "modify",
        binary: false,
        hunks: [],
        adds: 0,
        dels: 0,
      };
      currentHunk = null;
      skippingBinaryBlob = false;
      continue;
    }
    if (!current) continue; // preamble before the first `diff --git` header, if any

    if (skippingBinaryBlob) continue;

    if (line.startsWith("rename from ")) {
      current.oldPath = line.slice("rename from ".length);
      current.changeType = "rename";
      continue;
    }
    if (line.startsWith("rename to ")) {
      current.newPath = line.slice("rename to ".length);
      current.changeType = "rename";
      continue;
    }
    if (line.startsWith("new file mode")) {
      current.changeType = "add";
      continue;
    }
    if (line.startsWith("deleted file mode")) {
      current.changeType = "delete";
      continue;
    }
    if (line === "GIT binary patch" || line.startsWith("GIT binary patch")) {
      current.binary = true;
      skippingBinaryBlob = true;
      continue;
    }
    if (line.startsWith("Binary files ")) {
      current.binary = true;
      const m = line.match(BINARY_FILES_RE);
      if (m) {
        const [, a, b] = m;
        if (a !== "/dev/null") current.oldPath = stripAB(a);
        if (b !== "/dev/null") current.newPath = stripAB(b);
      }
      continue;
    }
    if (line.startsWith("index ") || line.startsWith("similarity index") || line.startsWith("dissimilarity index") || line.startsWith("old mode") || line.startsWith("new mode")) {
      continue;
    }
    if (line.startsWith("--- ")) {
      const rest = line.slice(4);
      if (rest === "/dev/null") {
        if (current.changeType === "modify") current.changeType = "add";
      } else {
        current.oldPath = stripAB(rest);
      }
      continue;
    }
    if (line.startsWith("+++ ")) {
      const rest = line.slice(4);
      if (rest === "/dev/null") {
        if (current.changeType === "modify") current.changeType = "delete";
      } else {
        current.newPath = stripAB(rest);
      }
      continue;
    }
    const hunkMatch = line.match(HUNK_HEADER_RE);
    if (hunkMatch) {
      const [, oldStart, oldLines, newStart, newLines, heading] = hunkMatch;
      currentHunk = {
        oldStart: Number(oldStart),
        oldLines: oldLines !== undefined ? Number(oldLines) : 1,
        newStart: Number(newStart),
        newLines: newLines !== undefined ? Number(newLines) : 1,
        heading: heading.trim(),
        lines: [],
      };
      current.hunks.push(currentHunk);
      continue;
    }
    if (line.startsWith("\\")) continue; // "\ No newline at end of file"
    if (!currentHunk) continue; // stray line outside any hunk (or a chopped-off header)

    if (line.startsWith("+")) currentHunk.lines.push({ type: "add", text: line.slice(1) });
    else if (line.startsWith("-")) currentHunk.lines.push({ type: "remove", text: line.slice(1) });
    else if (line.startsWith(" ")) currentHunk.lines.push({ type: "context", text: line.slice(1) });
    // else: a truncated tail can chop a line before its leading marker even lands - drop it
    // rather than guess what it was.
  }
  finalizeCurrent();

  return files;
}

/// Splits a path into its muted directory prefix (trailing slash kept) and emphasized basename -
/// mirrors GitExplorerPanel's `splitPath`, kept local here so this file stays a self-contained,
/// independently testable unit.
function splitPath(path: string): { dir: string; base: string } {
  const idx = path.lastIndexOf("/");
  return idx === -1 ? { dir: "", base: path } : { dir: path.slice(0, idx + 1), base: path.slice(idx + 1) };
}

const GLYPH: Record<DiffChangeType, { letter: string; class: string }> = {
  add: { letter: "A", class: "bg-signal/10 text-signal" },
  delete: { letter: "D", class: "bg-fault/10 text-fault" },
  rename: { letter: "R", class: "bg-muted/10 text-muted/70" },
  modify: { letter: "M", class: "bg-muted/10 text-muted/70" },
};

function DiffTypeGlyph(props: { type: DiffChangeType }) {
  const glyph = () => GLYPH[props.type];
  return (
    <span
      class={`flex size-4 shrink-0 items-center justify-center rounded font-mono text-[9px] font-semibold ${glyph().class}`}
      aria-hidden="true"
    >
      {glyph().letter}
    </span>
  );
}

function DiffLineRow(props: { line: DiffLine }) {
  const tint = () => (props.line.type === "add" ? "diff-line-add" : props.line.type === "remove" ? "diff-line-remove" : "");
  const marker = () => (props.line.type === "add" ? "+" : props.line.type === "remove" ? "-" : "");
  const markerClass = () => (props.line.type === "add" ? "text-signal" : props.line.type === "remove" ? "text-fault" : "text-muted/40");
  return (
    <div class={`flex px-3 py-px font-mono text-[11px] leading-[1.5] ${tint()}`}>
      <span class={`w-3 shrink-0 select-none ${markerClass()}`}>{marker()}</span>
      <span class="whitespace-pre text-foreground/90">{props.line.text}</span>
    </div>
  );
}

function DiffFileCard(props: {
  file: DiffFile;
  expanded: boolean;
  onToggle: () => void;
  ref?: (el: HTMLDivElement) => void;
}) {
  const parts = () => splitPath(props.file.path);
  return (
    <div ref={props.ref} class="diff-file-card overflow-hidden rounded-xl border border-line bg-surface">
      <button
        type="button"
        class="focus-ring flex w-full items-center gap-1.5 px-2 py-1.5 text-left hover:bg-raised/40"
        onClick={props.onToggle}
        aria-expanded={props.expanded}
      >
        <Show when={props.expanded} fallback={<IconChevronRight size={10} class="shrink-0 text-muted/50" />}>
          <IconChevronDown size={10} class="shrink-0 text-muted/50" />
        </Show>
        <DiffTypeGlyph type={props.file.changeType} />
        <span class="min-w-0 flex-1 truncate font-mono text-xs">
          <span class="text-muted/70">{parts().dir}</span>
          <span class="text-foreground">{parts().base}</span>
          <Show when={props.file.renamedFrom} keyed>
            {(from) => <span class="text-muted/50"> ← {from}</span>}
          </Show>
        </span>
        <Show
          when={!props.file.binary}
          fallback={<span class="shrink-0 text-[10px] text-muted/60">binary</span>}
        >
          <span class="shrink-0 text-[10px] tabular-nums">
            <Show when={props.file.adds > 0}>
              <span class="text-signal">+{props.file.adds}</span>
            </Show>
            <Show when={props.file.adds > 0 && props.file.dels > 0}> </Show>
            <Show when={props.file.dels > 0}>
              <span class="text-fault">-{props.file.dels}</span>
            </Show>
          </span>
        </Show>
      </button>
      <Show when={props.expanded}>
        <div class="border-t border-line">
          <Show
            when={!props.file.binary}
            fallback={<p class="px-3 py-2 text-xs text-muted">Binary file, no text diff to show.</p>}
          >
            <Show
              when={props.file.hunks.length > 0}
              fallback={<p class="px-3 py-2 text-xs text-muted">No line changes (rename only).</p>}
            >
              <div class="overflow-x-auto py-1">
                <div class="min-w-max">
                  <For each={props.file.hunks}>
                    {(hunk) => (
                      <div>
                        <div class="bg-raised/50 px-3 py-1 font-mono text-[10px] text-muted">
                          @@ -{hunk.oldStart},{hunk.oldLines} +{hunk.newStart},{hunk.newLines} @@ {hunk.heading}
                        </div>
                        <For each={hunk.lines}>{(line) => <DiffLineRow line={line} />}</For>
                      </div>
                    )}
                  </For>
                </div>
              </div>
            </Show>
          </Show>
        </div>
      </Show>
    </div>
  );
}

export interface DiffViewProps {
  /// Raw unified diff text, as returned in `LaneDiff.patch`.
  patch: string;
  /// Mirrors `LaneDiff.patch_truncated`: the daemon cut `patch` off at its character cap.
  truncated?: boolean;
  /// When set, only this file's card starts expanded (scrolled into view); every other card
  /// starts collapsed. `undefined` leaves every card collapsed - the overview-first default for
  /// opening the diff view without a specific file in mind.
  focusPath?: string;
  onClose?: () => void;
}

/// Lightweight unified-diff viewer for the Git explorer's right rail. Pure presentation: the
/// daemon computes the diff (`lane.diff` with `include_patch: true`), this only parses and
/// renders the text it's given - no diffing library, per C4's brief.
export default function DiffView(props: DiffViewProps) {
  const files = createMemo(() => parseDiff(props.patch));
  const [expanded, setExpanded] = createSignal<Set<string>>(new Set());
  const fileRefs = new Map<string, HTMLDivElement>();

  // Re-focusing on a new path (a fresh file-row click) collapses every other card down to just
  // that one; re-running this only on `focusPath` changing (not on `files()`) means the user's
  // own manual expand/collapse clicks after landing here aren't fought on the next poll refetch.
  createEffect(() => {
    const fp = props.focusPath;
    if (!fp) return;
    setExpanded(new Set([fp]));
    // Deferred one tick so the card for `fp` exists in `fileRefs` before we scroll to it.
    queueMicrotask(() => fileRefs.get(fp)?.scrollIntoView?.({ block: "nearest" }));
  });

  function toggle(path: string) {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  return (
    <div class="flex h-full min-h-0 flex-col">
      <div class="flex h-8 shrink-0 items-center justify-between border-b border-line px-2">
        <span class="section-label">Diff</span>
        <Show when={props.onClose} keyed>
          {(onClose) => (
            <button
              type="button"
              class="focus-ring flex size-5 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground"
              onClick={onClose}
              title="Close diff"
              aria-label="Close diff"
            >
              <IconClose size={11} />
            </button>
          )}
        </Show>
      </div>
      <Show when={props.truncated}>
        <div role="status" class="m-2 mb-0 rounded-lg border border-attention/40 bg-attention/10 px-2.5 py-1.5 text-[11px] text-attention">
          Diff truncated at {props.patch.length.toLocaleString()} chars, showing partial diff.
        </div>
      </Show>
      <div class="min-h-0 flex-1 space-y-2 overflow-y-auto p-2">
        <Show when={files().length > 0} fallback={<p class="px-1 py-2 text-xs text-muted">No uncommitted changes to show.</p>}>
          <For each={files()}>
            {(file) => (
              <DiffFileCard
                file={file}
                expanded={expanded().has(file.path)}
                onToggle={() => toggle(file.path)}
                ref={(el) => fileRefs.set(file.path, el)}
              />
            )}
          </For>
        </Show>
      </div>
    </div>
  );
}
