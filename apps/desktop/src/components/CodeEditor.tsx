import {
  type CompletionResult,
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  completeAnyWord,
  completionKeymap,
  type CompletionContext,
} from "@codemirror/autocomplete";
import {
  copyLineDown,
  defaultKeymap,
  history,
  historyKeymap,
  indentWithTab,
  moveLineDown,
  moveLineUp,
  toggleComment,
} from "@codemirror/commands";
import { css } from "@codemirror/lang-css";
import { html } from "@codemirror/lang-html";
import { javascript } from "@codemirror/lang-javascript";
import { json } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { python } from "@codemirror/lang-python";
import { rust } from "@codemirror/lang-rust";
import { languages } from "@codemirror/language-data";
import {
  HighlightStyle,
  LanguageDescription,
  bracketMatching,
  foldGutter,
  foldKeymap,
  indentOnInput,
  indentUnit,
  syntaxHighlighting,
  type LanguageSupport,
} from "@codemirror/language";
import {
  gotoLine,
  highlightSelectionMatches,
  searchKeymap,
  selectNextOccurrence,
} from "@codemirror/search";
import {
  Compartment,
  EditorState,
  RangeSet,
  RangeSetBuilder,
  StateEffect,
  StateField,
} from "@codemirror/state";
import {
  Decoration,
  EditorView,
  GutterMarker,
  ViewPlugin,
  crosshairCursor,
  drawSelection,
  gutter,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightWhitespace,
  keymap,
  lineNumbers,
  rectangularSelection,
  type DecorationSet,
  type KeyBinding,
  type ViewUpdate,
} from "@codemirror/view";
import { tags as t } from "@lezer/highlight";
import { Show, createEffect, createSignal, onCleanup, onMount, untrack } from "solid-js";
import { daemonCall } from "../ipc/rpc";
import { IconClose } from "./icons";
import {
  computeLineDiff,
  computeRevertChange,
  type DiffChangeType,
  type DiffHunk,
} from "./lineDiff";

export type EditorLanguage =
  | "javascript"
  | "jsx"
  | "typescript"
  | "tsx"
  | "json"
  | "rust"
  | "markdown"
  | "python"
  | "css"
  | "html"
  | "plain"
  | string;

/// Selection state reported alongside cursor position: how many discrete ranges are selected
/// (more than one implies multi-cursor) and the total number of characters spanned by them.
export interface EditorSelectionInfo {
  rangeCount: number;
  selectedChars: number;
}

export interface CodeEditorReplaceRequest {
  query: string;
  replacement: string;
  regex: boolean;
  caseSensitive: boolean;
  all?: boolean;
  token: number;
}

export interface CodeEditorProps {
  value: string;
  path?: string;
  laneId?: number;
  diffBase?: string | null;
  disableGitGutter?: boolean;
  readOnly?: boolean;
  wrap?: boolean;
  whitespace?: boolean;
  languageOverride?: string;
  onChange?: (value: string) => void;
  onSave?: () => void;
  onCursorActivity?: (cursor: number, scrollTop: number, selection: EditorSelectionInfo) => void;
  onVisibleLineChange?: (line: number) => void;
  initialCursor?: number;
  initialScrollTop?: number;
  openAtTarget?: { line: number; column: number; token: number } | null;
  replaceRequest?: CodeEditorReplaceRequest | null;
  class?: string;
}

const languageCache = new Map<string, LanguageSupport>();

export function matchSpecialFilename(path: string): string | null {
  const base = path.split("/").pop() || path;
  if (/^Dockerfile(\..+)?$/i.test(base) || base === "Containerfile") return "Dockerfile";
  if (base === "Makefile" || base === "Justfile" || base === "Brewfile" || base === "Procfile") return "Shell";
  if (base === "Vagrantfile") return "Ruby";
  if (base === "Cargo.lock") return "TOML";
  if (base === "package-lock.json") return "JSON";
  if (base === "pnpm-lock.yaml") return "YAML";
  if (base === ".gitignore" || base === ".gitattributes" || base === ".gitmodules" || base.startsWith(".env")) return "Shell";
  if (base === ".editorconfig") return "Properties files";
  return null;
}

export function sniffShebang(content: string): string | null {
  if (!content.startsWith("#!")) return null;
  const firstLine = content.split("\n")[0].toLowerCase();
  if (firstLine.includes("python")) return "Python";
  if (firstLine.includes("bash") || firstLine.includes("sh") || firstLine.includes("zsh")) return "Shell";
  if (firstLine.includes("node")) return "JavaScript";
  if (firstLine.includes("ruby")) return "Ruby";
  if (firstLine.includes("perl")) return "Perl";
  if (firstLine.includes("php")) return "PHP";
  return null;
}

export function extensionToLanguage(path: string, content?: string, override?: string): EditorLanguage {
  if (override) return override;

  const special = matchSpecialFilename(path);
  if (special) return special.toLowerCase();

  if (content) {
    const shebang = sniffShebang(content);
    if (shebang) return shebang.toLowerCase();
  }

  const match = /\.([^./\\]+)$/.exec(path);
  const ext = (match?.[1] ?? "").toLowerCase();
  switch (ext) {
    case "js":
    case "mjs":
    case "cjs":
      return "javascript";
    case "jsx":
      return "jsx";
    case "ts":
    case "mts":
    case "cts":
      return "typescript";
    case "tsx":
      return "tsx";
    case "json":
    case "jsonc":
      return "json";
    case "rs":
      return "rust";
    case "md":
    case "markdown":
      return "markdown";
    case "py":
    case "pyw":
      return "python";
    case "css":
      return "css";
    case "html":
    case "htm":
      return "html";
    default: {
      const desc = LanguageDescription.matchFilename(languages, path);
      if (desc) return desc.name.toLowerCase();
      return "plain";
    }
  }
}

function getBundledSupport(name: string): LanguageSupport | null {
  switch (name.toLowerCase()) {
    case "javascript":
    case "js":
      return javascript();
    case "jsx":
      return javascript({ jsx: true });
    case "typescript":
    case "ts":
      return javascript({ typescript: true });
    case "tsx":
      return javascript({ jsx: true, typescript: true });
    case "json":
      return json();
    case "rust":
    case "rs":
      return rust();
    case "markdown":
    case "md":
      return markdown();
    case "python":
    case "py":
      return python();
    case "css":
      return css();
    case "html":
      return html();
    default:
      return null;
  }
}

/// `shebangLanguage` is a language name already sniffed from the file's first line (or `null`/
/// `undefined`), not raw content - callers sniff it once, outside any reactive tracking, so this
/// resolver never needs the full document to decide a language.
export async function resolveLanguageSupport(
  path: string,
  shebangLanguage?: string | null,
  override?: string,
): Promise<LanguageSupport | null> {
  const target = override || matchSpecialFilename(path) || shebangLanguage || null;
  if (target) {
    const bundled = getBundledSupport(target);
    if (bundled) return bundled;
    if (languageCache.has(target)) return languageCache.get(target)!;
    const desc = LanguageDescription.matchLanguageName(languages, target, true);
    if (desc) {
      try {
        const loaded = await desc.load();
        languageCache.set(target, loaded);
        return loaded;
      } catch {
        return null;
      }
    }
  }

  const match = /\.([^./\\]+)$/.exec(path);
  const ext = (match?.[1] ?? "").toLowerCase();
  const bundled = getBundledSupport(ext);
  if (bundled) return bundled;

  const desc = LanguageDescription.matchFilename(languages, path);
  if (desc) {
    if (languageCache.has(desc.name)) return languageCache.get(desc.name)!;
    try {
      const loaded = await desc.load();
      languageCache.set(desc.name, loaded);
      return loaded;
    } catch {
      return null;
    }
  }

  return null;
}

function getSyncLanguageSupport(
  path: string,
  shebangLanguage?: string | null,
  override?: string,
): LanguageSupport | null {
  const target = override || matchSpecialFilename(path) || shebangLanguage || null;
  if (target) {
    const bundled = getBundledSupport(target);
    if (bundled) return bundled;
    if (languageCache.has(target)) return languageCache.get(target)!;
  }

  const match = /\.([^./\\]+)$/.exec(path);
  const ext = (match?.[1] ?? "").toLowerCase();
  const bundled = getBundledSupport(ext);
  if (bundled) return bundled;

  const desc = LanguageDescription.matchFilename(languages, path);
  if (desc && languageCache.has(desc.name)) {
    return languageCache.get(desc.name)!;
  }

  return null;
}

export function detectIndentUnit(content: string, path: string): string {
  const ext = (path.split(".").pop() || "").toLowerCase();
  const base = path.split("/").pop() || path;
  let defaultUnit = "  ";
  if (["rs", "py", "c", "cpp", "h", "hpp", "cs", "java"].includes(ext)) {
    defaultUnit = "    ";
  } else if (["go"].includes(ext) || base === "Makefile" || base === "Justfile") {
    defaultUnit = "\t";
  }

  if (!content) return defaultUnit;

  const lines = content.split("\n").slice(0, 100);
  let space2Count = 0;
  let space4Count = 0;
  let tabCount = 0;

  for (const line of lines) {
    if (line.startsWith("\t")) {
      tabCount++;
    } else if (line.startsWith("    ")) {
      space4Count++;
    } else if (line.startsWith("  ")) {
      space2Count++;
    }
  }

  if (tabCount > space2Count && tabCount > space4Count) return "\t";
  if (space4Count > space2Count && space4Count > tabCount) return "    ";
  if (space2Count > 0) return "  ";

  return defaultUnit;
}

const indentGuideMark = Decoration.mark({ class: "cm-indent-guide" });

function buildIndentGuides(view: EditorView): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  for (const { from, to } of view.visibleRanges) {
    let pos = from;
    while (pos < to) {
      const line = view.state.doc.lineAt(pos);
      const text = line.text;
      const match = /^(\s+)/.exec(text);
      if (match && match[1].length > 1) {
        const leading = match[1];
        let i = 0;
        const step = line.text.startsWith("\t") ? 1 : 2;
        while (i < leading.length) {
          const start = line.from + i;
          const end = Math.min(line.from + i + 1, line.to);
          builder.add(start, end, indentGuideMark);
          i += step;
        }
      }
      pos = line.to + 1;
    }
  }
  return builder.finish();
}

export const indentGuidePlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = buildIndentGuides(view);
    }
    update(update: ViewUpdate) {
      if (update.docChanged || update.viewportChanged) {
        this.decorations = buildIndentGuides(update.view);
      }
    }
  },
  {
    decorations: (v) => v.decorations,
  },
);

class GitDiffGutterMarker extends GutterMarker {
  type: DiffChangeType;
  hunk: DiffHunk;
  onMarkerClick: (hunk: DiffHunk, el: HTMLElement) => void;

  constructor(
    type: DiffChangeType,
    hunk: DiffHunk,
    onMarkerClick: (hunk: DiffHunk, el: HTMLElement) => void,
  ) {
    super();
    this.type = type;
    this.hunk = hunk;
    this.onMarkerClick = onMarkerClick;
  }

  eq(other: GutterMarker): boolean {
    return (
      other instanceof GitDiffGutterMarker &&
      other.type === this.type &&
      other.hunk.id === this.hunk.id
    );
  }

  toDOM(): Node {
    const el = document.createElement("div");
    el.className = `cm-git-gutter-marker cm-git-gutter-${this.type}`;
    el.setAttribute("role", "button");
    el.setAttribute("aria-label", `${this.type} diff hunk`);
    el.title = `${this.type} hunk: click to inspect or revert`;
    el.onmousedown = (e) => {
      e.stopPropagation();
    };
    el.onclick = (e) => {
      e.stopPropagation();
      e.preventDefault();
      this.onMarkerClick(this.hunk, el);
    };
    if (this.type === "removed") {
      const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      svg.setAttribute("width", "6");
      svg.setAttribute("height", "6");
      svg.setAttribute("viewBox", "0 0 6 6");
      svg.setAttribute("class", "cm-git-removed-triangle");
      const polygon = document.createElementNS("http://www.w3.org/2000/svg", "polygon");
      polygon.setAttribute("points", "0,0 6,0 3,6");
      polygon.setAttribute("fill", "var(--fault)");
      svg.appendChild(polygon);
      el.appendChild(svg);
    }
    return el;
  }
}

class GitSpacerMarker extends GutterMarker {
  toDOM() {
    const el = document.createElement("div");
    el.style.width = "6px";
    return el;
  }
}

const setGitMarkersEffect = StateEffect.define<RangeSet<GutterMarker>>();

const gitGutterField = StateField.define<RangeSet<GutterMarker>>({
  create() {
    return RangeSet.empty;
  },
  update(markers, tr) {
    for (const e of tr.effects) {
      if (e.is(setGitMarkersEffect)) {
        return e.value;
      }
    }
    return markers.map(tr.changes);
  },
});

export const appTheme = EditorView.theme(
  {
    "&": {
      color: "var(--foreground)",
      backgroundColor: "var(--surface)",
      fontFamily: "var(--font-mono)",
      fontSize: "12px",
      lineHeight: "1.5",
      height: "100%",
    },
    ".cm-scroller": {
      fontFamily: "inherit",
      lineHeight: "inherit",
      overflow: "auto",
    },
    ".cm-content": {
      padding: "8px 0",
      caretColor: "var(--signal)",
      tabSize: 4,
    },
    "&.cm-focused .cm-cursor": {
      borderLeftColor: "var(--signal)",
      borderLeftWidth: "2px",
    },
    "&.cm-focused .cm-selectionBackground, ::selection": {
      backgroundColor: "color-mix(in srgb, var(--signal) 25%, transparent)",
    },
    ".cm-selectionMatch": {
      backgroundColor: "color-mix(in srgb, var(--attention) 18%, transparent)",
      borderRadius: "2px",
    },
    ".cm-gutters": {
      backgroundColor: "var(--surface)",
      color: "color-mix(in srgb, var(--muted) 60%, transparent)",
      borderRight: "1px solid var(--line)",
      paddingRight: "4px",
    },
    ".cm-git-diff-gutter": {
      width: "6px",
      minWidth: "6px",
      backgroundColor: "var(--surface)",
      borderRight: "none",
    },
    ".cm-git-diff-gutter .cm-gutterElement": {
      padding: "0",
      minWidth: "6px",
      width: "6px",
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      cursor: "pointer",
    },
    ".cm-git-gutter-marker": {
      width: "3px",
      height: "100%",
      borderRadius: "1px",
      transition: "opacity 0.15s ease",
    },
    ".cm-git-gutter-marker:hover": {
      opacity: "0.8",
    },
    ".cm-git-gutter-added": {
      backgroundColor: "var(--signal)",
    },
    ".cm-git-gutter-modified": {
      backgroundColor: "var(--attention)",
    },
    ".cm-git-gutter-removed": {
      width: "6px",
      height: "6px",
      backgroundColor: "transparent",
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
    },
    ".cm-gutterElement": {
      paddingLeft: "8px",
      paddingRight: "8px",
      minWidth: "2.5em",
      textAlign: "right",
      userSelect: "none",
    },
    ".cm-activeLine": {
      backgroundColor: "color-mix(in srgb, var(--raised) 50%, transparent)",
    },
    ".cm-activeLineGutter": {
      color: "var(--foreground)",
      backgroundColor: "color-mix(in srgb, var(--raised) 50%, transparent)",
    },
    ".cm-foldGutter .cm-gutterElement": {
      cursor: "pointer",
      color: "color-mix(in srgb, var(--muted) 50%, transparent)",
    },
    ".cm-foldGutter .cm-gutterElement:hover": {
      color: "var(--foreground)",
    },
    ".cm-indent-guide": {
      borderLeft: "1px solid color-mix(in srgb, var(--line) 40%, transparent)",
      marginLeft: "-1px",
    },
    ".cm-panel.cm-search": {
      backgroundColor: "var(--surface)",
      borderBottom: "1px solid var(--line)",
      padding: "6px 10px",
      fontSize: "12px",
      fontFamily: "var(--font-mono)",
      display: "flex",
      alignItems: "center",
      flexWrap: "wrap",
      gap: "6px",
    },
    ".cm-panel.cm-search input, .cm-panel.cm-search button": {
      fontSize: "11px",
      fontFamily: "var(--font-mono)",
    },
    ".cm-panel.cm-search .cm-textfield": {
      backgroundColor: "var(--raised)",
      border: "1px solid var(--line)",
      borderRadius: "4px",
      padding: "2px 6px",
      color: "var(--foreground)",
      outline: "none",
    },
    ".cm-panel.cm-search .cm-textfield:focus": {
      borderColor: "var(--signal)",
    },
    ".cm-panel.cm-search .cm-button": {
      backgroundColor: "var(--surface)",
      border: "1px solid var(--line)",
      borderRadius: "4px",
      padding: "2px 8px",
      color: "var(--muted)",
      cursor: "pointer",
    },
    ".cm-panel.cm-search .cm-button:hover": {
      backgroundColor: "var(--raised)",
      color: "var(--foreground)",
    },
    ".cm-panel.cm-search label": {
      display: "inline-flex",
      alignItems: "center",
      gap: "4px",
      color: "var(--muted)",
      fontSize: "11px",
      cursor: "pointer",
    },
    ".cm-tooltip": {
      backgroundColor: "var(--surface)",
      border: "1px solid var(--line)",
      borderRadius: "6px",
      boxShadow: "0 4px 12px rgba(0, 0, 0, 0.3)",
    },
    ".cm-tooltip-autocomplete": {
      "& > ul": {
        fontFamily: "var(--font-mono)",
        fontSize: "11px",
      },
      "& > ul > li": {
        padding: "3px 8px",
      },
      "& > ul > li[aria-selected]": {
        backgroundColor: "var(--raised)",
        color: "var(--foreground)",
      },
    },
  },
  { dark: true },
);

// The app's whole visual identity runs on four accent roles (--signal, --attention, --fault,
// --muted) layered over --surface/--foreground - see src/index.css. Syntax highlighting reuses
// exactly those roles instead of inventing a rainbow token palette: keywords/tags read as
// "structure" (signal), literals read as "data" (attention), comments/punctuation recede (muted),
// and only genuinely invalid syntax reaches for --fault. Every value below is a var(...) or
// color-mix(...) reference, so all six themes in index.css repaint the editor automatically.
// Extended past main's base set with the tags the wider @codemirror/language-data catalog (yaml,
// toml, shell, go, sql, dockerfile, ...) actually emits, mapped onto the same four roles.
export const highlightStyle = HighlightStyle.define([
  { tag: t.comment, color: "var(--muted)", fontStyle: "italic" },
  { tag: t.lineComment, color: "var(--muted)", fontStyle: "italic" },
  { tag: t.blockComment, color: "var(--muted)", fontStyle: "italic" },
  { tag: t.docComment, color: "var(--muted)", fontStyle: "italic" },

  { tag: [t.keyword, t.controlKeyword, t.moduleKeyword, t.operatorKeyword], color: "var(--signal)" },
  { tag: [t.tagName, t.angleBracket], color: "var(--signal)" },
  { tag: [t.definitionKeyword, t.definition(t.variableName)], color: "var(--signal)", fontWeight: 600 },
  { tag: [t.macroName, t.labelName, t.modifier], color: "var(--signal)" },

  {
    tag: [t.string, t.special(t.string), t.regexp, t.character],
    color: "color-mix(in srgb, var(--attention) 88%, var(--foreground))",
  },
  { tag: [t.number, t.bool, t.atom, t.null], color: "var(--attention)" },
  { tag: [t.constant(t.name), t.standard(t.name), t.color, t.special(t.variableName)], color: "var(--attention)" },
  { tag: t.inserted, color: "color-mix(in srgb, var(--attention) 88%, var(--foreground))" },

  { tag: [t.function(t.variableName), t.function(t.propertyName)], color: "var(--foreground)", fontWeight: 600 },
  { tag: [t.className, t.typeName, t.namespace], color: "var(--foreground)", fontWeight: 600 },
  { tag: [t.propertyName, t.attributeName], color: "color-mix(in srgb, var(--foreground) 78%, var(--muted))" },
  { tag: [t.variableName, t.self], color: "var(--foreground)" },
  { tag: [t.name, t.character], color: "var(--foreground)" },

  { tag: [t.punctuation, t.bracket, t.separator], color: "var(--muted)" },
  { tag: t.operator, color: "color-mix(in srgb, var(--foreground) 80%, var(--muted))" },
  { tag: [t.meta, t.annotation, t.url, t.escape, t.processingInstruction], color: "var(--muted)" },
  { tag: t.heading, color: "var(--signal)", fontWeight: 700 },
  { tag: t.link, color: "var(--signal)", textDecoration: "underline" },
  { tag: t.strong, fontWeight: 700 },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.strikethrough, textDecoration: "line-through" },

  { tag: t.changed, color: "var(--signal)" },
  { tag: t.deleted, color: "var(--fault)" },
  { tag: t.invalid, color: "var(--fault)" },
]);

const MIN_WORD_COMPLETION_LENGTH = 3;

/// Document-word completion: offers words already present in the open document as completion
/// candidates, alongside whatever the active language's own completion source contributes.
/// Wired in as language data (see `wordCompletionData` below) rather than an `autocompletion()`
/// `override`, so it merges with the language's completions instead of replacing them.
export function documentWordCompletionSource(
  context: CompletionContext,
): CompletionResult | Promise<CompletionResult | null> | null {
  const result = completeAnyWord(context);
  if (result && "then" in result) {
    return result.then((r) => (r ? filterShortWordOptions(r) : r));
  }
  return result ? filterShortWordOptions(result) : result;
}

function filterShortWordOptions(result: CompletionResult): CompletionResult {
  return {
    ...result,
    options: result.options.filter((option) => option.label.length >= MIN_WORD_COMPLETION_LENGTH),
  };
}

const wordCompletionData = EditorState.languageData.of(() => [
  { autocomplete: documentWordCompletionSource },
]);

export default function CodeEditor(props: CodeEditorProps) {
  let containerRef!: HTMLDivElement;
  let view: EditorView | undefined;
  let lastKnownDoc = props.value;
  let applyingExternalValue = false;
  // Guards the cursor/scroll restoration effect below so it fires once per file activation
  // (a `path` change) rather than on every reactive read - the initial file's restoration is
  // handled inline in onMount, so this starts equal to the first path to skip a redundant re-run
  // the moment the effect's initial pass executes.
  let lastCursorAppliedPath = props.path;
  // Incremented on every language-resolution request so an async `resolveLanguageSupport` call
  // that resolves after a newer request has started (e.g. the user switched files again before
  // the first load finished) can recognize itself as stale and skip its dispatch.
  let languageRequestId = 0;

  const readOnlyCompartment = new Compartment();
  const languageCompartment = new Compartment();
  const wrapCompartment = new Compartment();
  const whitespaceCompartment = new Compartment();
  const indentUnitCompartment = new Compartment();
  const gitGutterCompartment = new Compartment();

  const [hunkPopover, setHunkPopover] = createSignal<{
    hunk: DiffHunk;
    top: number;
    left: number;
  } | null>(null);

  let gitDiffRequestId = 0;
  let currentBaseContent: string | null = null;
  let currentDiffPath = props.path ?? "";
  let diffDebounceTimer: number | null = null;

  const updateGutterMarkers = (base: string | null, current: string) => {
    if (!view || props.disableGitGutter) return;
    const diff = computeLineDiff(base, current);
    const doc = view.state.doc;
    const builder = new RangeSetBuilder<GutterMarker>();
    const sortedLines = Array.from(diff.markers.keys()).sort((a, b) => a - b);
    for (const lineNum of sortedLines) {
      if (lineNum >= 1 && lineNum <= doc.lines) {
        const line = doc.line(lineNum);
        const m = diff.markers.get(lineNum)!;
        builder.add(
          line.from,
          line.from,
          new GitDiffGutterMarker(m.type, m.hunk, (hunk, el) => {
            const elRect = el.getBoundingClientRect();
            const contRect = containerRef.getBoundingClientRect();
            setHunkPopover({
              hunk,
              top: Math.max(8, elRect.top - contRect.top),
              left: Math.max(8, elRect.right - contRect.left + 6),
            });
          }),
        );
      }
    }
    view.dispatch({
      effects: setGitMarkersEffect.of(builder.finish()),
    });
  };

  const refreshDiffBase = async (
    path = props.path,
    laneId = props.laneId,
    diffBase = props.diffBase,
    disabled = props.disableGitGutter,
  ) => {
    const reqId = ++gitDiffRequestId;
    currentDiffPath = path ?? "";
    setHunkPopover(null);

    if (disabled) {
      currentBaseContent = null;
      if (view) {
        view.dispatch({ effects: setGitMarkersEffect.of(RangeSet.empty) });
      }
      return;
    }

    if (diffBase !== undefined) {
      currentBaseContent = diffBase;
      updateGutterMarkers(currentBaseContent, view ? view.state.doc.toString() : (props.value ?? ""));
      return;
    }

    if (!laneId || !path) {
      currentBaseContent = null;
      updateGutterMarkers(null, view ? view.state.doc.toString() : (props.value ?? ""));
      return;
    }

    try {
      const res = await daemonCall("file.diff_base", { lane_id: laneId, path });
      if (reqId !== gitDiffRequestId || (props.path ?? "") !== path) return;
      currentBaseContent = res.kind === "text" ? res.content : null;
      updateGutterMarkers(currentBaseContent, view ? view.state.doc.toString() : (props.value ?? ""));
    } catch {
      if (reqId !== gitDiffRequestId || (props.path ?? "") !== path) return;
      currentBaseContent = null;
      updateGutterMarkers(null, view ? view.state.doc.toString() : (props.value ?? ""));
    }
  };

  const saveBinding: KeyBinding = {
    key: "Mod-s",
    run: () => {
      props.onSave?.();
      void refreshDiffBase();
      return true;
    },
  };

  const extraKeymaps: KeyBinding[] = [
    saveBinding,
    { key: "Mod-/", run: toggleComment },
    { key: "Alt-ArrowUp", run: moveLineUp },
    { key: "Alt-ArrowDown", run: moveLineDown },
    { key: "Shift-Alt-ArrowDown", run: copyLineDown },
    { key: "Mod-d", run: selectNextOccurrence },
    { key: "Mod-g", run: gotoLine },
  ];

  onMount(() => {
    lastKnownDoc = props.value;

    // Shebang sniffing only ever looks at the file's first line, so it is cheap and safe to do
    // once, synchronously, off the initial content - it must not become a reactive dependency on
    // `props.value` (see the language-resolution effect below) or it would re-run on every
    // keystroke.
    const initialShebang = sniffShebang(props.value);
    const initialSupport = getSyncLanguageSupport(props.path ?? "", initialShebang, props.languageOverride);
    const unit = detectIndentUnit(props.value, props.path ?? "");

    const state = EditorState.create({
      doc: props.value,
      extensions: [
        lineNumbers(),
        gitGutterCompartment.of(
          props.disableGitGutter
            ? []
            : [
                gitGutterField,
                gutter({
                  class: "cm-git-diff-gutter",
                  markers: (v) => v.state.field(gitGutterField),
                  initialSpacer: () => new GitSpacerMarker(),
                }),
              ],
        ),
        foldGutter(),
        highlightActiveLineGutter(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        drawSelection(),
        bracketMatching(),
        closeBrackets(),
        indentOnInput(),
        autocompletion(),
        wordCompletionData,
        EditorState.allowMultipleSelections.of(true),
        rectangularSelection(),
        crosshairCursor(),
        history(),
        indentGuidePlugin,
        readOnlyCompartment.of(EditorState.readOnly.of(props.readOnly ?? false)),
        wrapCompartment.of(props.wrap ? [EditorView.lineWrapping] : []),
        whitespaceCompartment.of(props.whitespace ? [highlightWhitespace()] : []),
        indentUnitCompartment.of(indentUnit.of(unit)),
        keymap.of([
          ...extraKeymaps,
          ...closeBracketsKeymap,
          ...completionKeymap,
          ...foldKeymap,
          ...defaultKeymap,
          ...historyKeymap,
          ...searchKeymap,
          indentWithTab,
        ]),
        languageCompartment.of(initialSupport ? [initialSupport] : []),
        appTheme,
        syntaxHighlighting(highlightStyle),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            lastKnownDoc = update.state.doc.toString();
            if (!applyingExternalValue) {
              props.onChange?.(lastKnownDoc);
            }
            if (diffDebounceTimer !== null) {
              clearTimeout(diffDebounceTimer);
            }
            diffDebounceTimer = window.setTimeout(() => {
              updateGutterMarkers(currentBaseContent, lastKnownDoc);
            }, 300);
          }
          if (update.selectionSet || update.docChanged) {
            const selection = update.state.selection;
            const head = selection.main.head;
            let selectedChars = 0;
            for (const range of selection.ranges) {
              selectedChars += range.to - range.from;
            }
            const line = update.state.doc.lineAt(head).number;
            props.onVisibleLineChange?.(line);
            props.onCursorActivity?.(head, update.view.scrollDOM.scrollTop, {
              rangeCount: selection.ranges.length,
              selectedChars,
            });
          }
        }),
      ],
    });

    view = new EditorView({
      state,
      parent: containerRef,
    });

    const onScroll = () => {
      if (!view) return;
      try {
        const block = view.lineBlockAtHeight(view.scrollDOM.scrollTop);
        const line = view.state.doc.lineAt(block.from).number;
        props.onVisibleLineChange?.(line);
      } catch {}
    };
    view.scrollDOM.addEventListener("scroll", onScroll, { passive: true });

    if (props.initialCursor !== undefined && props.initialCursor > 0) {
      const pos = Math.min(props.initialCursor, view.state.doc.length);
      view.dispatch({ selection: { anchor: pos } });
    }
    if (props.initialScrollTop !== undefined && props.initialScrollTop > 0) {
      view.scrollDOM.scrollTop = props.initialScrollTop;
    }

    void refreshDiffBase();

    onCleanup(() => {
      view?.scrollDOM.removeEventListener("scroll", onScroll);
      if (diffDebounceTimer !== null) {
        clearTimeout(diffDebounceTimer);
      }
      view?.destroy();
      view = undefined;
    });
  });

  // Keeps the live doc in sync with an externally-updated `value` prop that is *not* a file
  // activation - e.g. a reload after resolving a conflict, or the parent handing our own
  // `onChange` value straight back down (a no-op here, since it already matches `lastKnownDoc`).
  // File-activation doc swaps are instead handled inside the path-keyed effect below, together
  // with cursor restoration, in a single dispatch - see that effect's comment for why.
  createEffect(() => {
    const nextVal = props.value;
    if (!view) return;
    if (nextVal === lastKnownDoc) return;

    applyingExternalValue = true;
    try {
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: nextVal },
      });
      lastKnownDoc = nextVal;
    } finally {
      applyingExternalValue = false;
    }
  });

  // Re-applies cursor/scroll restoration on every file activation, not just the first mount - the
  // center workspace and the rail panel keep a single CodeEditor instance alive across tab
  // switches (see EditorWorkspace.tsx and FileEditorPanel.tsx), so `onMount` only ever fires once
  // for the very first file opened.
  //
  // Tracks `path` alone. `value`/`initialCursor`/`initialScrollTop` are read `untrack`ed, at the
  // instant `path` changes, so this does not also fire on ordinary edits or on the store
  // recording cursor activity for reasons other than switching files.
  //
  // This effect does its OWN (idempotent) doc replacement first, in the same dispatch as the
  // selection restore, rather than relying on the value-sync effect above to have already swapped
  // the doc by the time this runs: `path` and `value` change together on a file switch, but
  // Solid does not guarantee these two sibling effects run in a fixed relative order across
  // updates (observed empirically to flip between one file switch and the next), and letting the
  // value-sync effect's default-mapped selection collapse win afterward silently discards the
  // cursor position this effect just restored.
  createEffect(() => {
    const path = props.path ?? "";
    if (!view) return;
    if (path === lastCursorAppliedPath) return;
    lastCursorAppliedPath = path;

    const nextVal = untrack(() => props.value);
    const cursor = untrack(() => props.initialCursor);
    const scrollTop = untrack(() => props.initialScrollTop);

    const needsReplace = nextVal !== lastKnownDoc;
    const docLength = needsReplace ? nextVal.length : view.state.doc.length;
    const pos = cursor !== undefined && cursor > 0 ? Math.min(cursor, docLength) : 0;

    applyingExternalValue = needsReplace;
    try {
      view.dispatch({
        ...(needsReplace ? { changes: { from: 0, to: view.state.doc.length, insert: nextVal } } : {}),
        selection: { anchor: pos },
      });
      if (needsReplace) lastKnownDoc = nextVal;
    } finally {
      applyingExternalValue = false;
    }

    view.scrollDOM.scrollTop = scrollTop !== undefined && scrollTop > 0 ? scrollTop : 0;
  });

  createEffect(() => {
    const ro = props.readOnly ?? false;
    if (!view) return;
    view.dispatch({
      effects: readOnlyCompartment.reconfigure(EditorState.readOnly.of(ro)),
    });
  });

  createEffect(() => {
    const w = Boolean(props.wrap);
    if (!view) return;
    view.dispatch({
      effects: wrapCompartment.reconfigure(w ? [EditorView.lineWrapping] : []),
    });
  });

  createEffect(() => {
    const ws = Boolean(props.whitespace);
    if (!view) return;
    view.dispatch({
      effects: whitespaceCompartment.reconfigure(ws ? [highlightWhitespace()] : []),
    });
  });

  createEffect(() => {
    const disabled = Boolean(props.disableGitGutter);
    if (!view) return;
    view.dispatch({
      effects: gitGutterCompartment.reconfigure(
        disabled
          ? []
          : [
              gitGutterField,
              gutter({
                class: "cm-git-diff-gutter",
                markers: (v) => v.state.field(gitGutterField),
                initialSpacer: () => new GitSpacerMarker(),
              }),
            ],
      ),
    });
  });

  createEffect(() => {
    const path = props.path;
    const laneId = props.laneId;
    const diffBase = props.diffBase;
    const disabled = props.disableGitGutter;
    if (!view) return;
    void refreshDiffBase(path, laneId, diffBase, disabled);
  });

  createEffect(() => {
    if (!hunkPopover()) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        setHunkPopover(null);
      }
    };
    const onPointerDown = (e: PointerEvent) => {
      const popoverEl = containerRef.querySelector("[role=dialog]");
      if (popoverEl && !popoverEl.contains(e.target as Node)) {
        setHunkPopover(null);
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("pointerdown", onPointerDown, true);
    onCleanup(() => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("pointerdown", onPointerDown, true);
    });
  });

  // Resolves the language for the active file. Depends on `path` and `languageOverride` only -
  // *not* `props.value` - so it does not re-run on every keystroke; the shebang is re-sniffed
  // from the current content only at the instant `path` changes (read `untrack`ed, so later edits
  // to that same file don't retrigger this effect). Every run gets a fresh request id, and an
  // async `resolveLanguageSupport` result is dropped if a newer request has started or `path` has
  // since changed again - otherwise a slow load for a file the user already navigated away from
  // could win the race and paint the wrong language onto whatever is open now.
  createEffect(() => {
    const path = props.path ?? "";
    const override = props.languageOverride;
    if (!view) return;

    const shebang = untrack(() => sniffShebang(props.value));
    const requestId = ++languageRequestId;

    const sync = getSyncLanguageSupport(path, shebang, override);
    if (sync) {
      view.dispatch({ effects: languageCompartment.reconfigure([sync]) });
      return;
    }

    void resolveLanguageSupport(path, shebang, override).then((support) => {
      if (!view) return;
      if (requestId !== languageRequestId) return;
      if ((props.path ?? "") !== path) return;
      view.dispatch({ effects: languageCompartment.reconfigure(support ? [support] : []) });
    });
  });

  let lastOpenAtToken = 0;
  createEffect(() => {
    const target = props.openAtTarget;
    if (!target || !view) return;
    if (target.token === lastOpenAtToken) return;
    lastOpenAtToken = target.token;

    const doc = view.state.doc;
    const lineNum = Math.max(1, Math.min(target.line, doc.lines));
    const line = doc.line(lineNum);
    const colOffset = Math.max(0, Math.min(target.column - 1, line.length));
    const pos = line.from + colOffset;

    view.dispatch({
      selection: { anchor: pos },
      scrollIntoView: true,
    });
    view.focus();
  });

  let lastReplaceToken = 0;
  createEffect(() => {
    const req = props.replaceRequest;
    if (!req || !view) return;
    if (req.token === lastReplaceToken) return;
    lastReplaceToken = req.token;

    const doc = view.state.doc;
    const docText = doc.toString();
    if (!req.query) return;

    if (req.all) {
      try {
        const pattern = req.regex
          ? new RegExp(req.query, req.caseSensitive ? "g" : "gi")
          : new RegExp(req.query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), req.caseSensitive ? "g" : "gi");

        const changes: Array<{ from: number; to: number; insert: string }> = [];
        let m: RegExpExecArray | null;
        let count = 0;
        const maxIterations = docText.length + 1;
        while ((m = pattern.exec(docText)) !== null) {
          if (++count > maxIterations) break;
          if (m[0].length === 0) {
            pattern.lastIndex = m.index + 1;
            continue;
          }
          changes.push({ from: m.index, to: m.index + m[0].length, insert: req.replacement });
        }
        if (changes.length > 0) {
          view.dispatch({ changes });
          props.onChange?.(view.state.doc.toString());
        }
      } catch {}
    } else {
      const currentSel = view.state.selection.main;
      const selectedText = docText.slice(currentSel.from, currentSel.to);
      const isMatch = req.caseSensitive
        ? selectedText === req.query
        : selectedText.toLowerCase() === req.query.toLowerCase();

      if (isMatch) {
        view.dispatch({
          changes: { from: currentSel.from, to: currentSel.to, insert: req.replacement },
          selection: { anchor: currentSel.from + req.replacement.length },
          scrollIntoView: true,
        });
        props.onChange?.(view.state.doc.toString());
      } else {
        try {
          const pattern = req.regex
            ? new RegExp(req.query, req.caseSensitive ? "g" : "gi")
            : new RegExp(req.query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), req.caseSensitive ? "g" : "gi");
          pattern.lastIndex = currentSel.to;
          let m = pattern.exec(docText);
          if (!m) {
            pattern.lastIndex = 0;
            m = pattern.exec(docText);
          }
          if (m) {
            view.dispatch({
              changes: { from: m.index, to: m.index + m[0].length, insert: req.replacement },
              selection: { anchor: m.index + req.replacement.length },
              scrollIntoView: true,
            });
            props.onChange?.(view.state.doc.toString());
          }
        } catch {}
      }
    }
  });

  return (
    <div
      ref={containerRef}
      class={props.class ? `relative min-h-0 flex-1 overflow-hidden ${props.class}` : "relative min-h-0 flex-1 overflow-hidden"}
    >
      <Show when={hunkPopover()}>
        {(popover) => {
          const hunk = popover().hunk;
          return (
            <div
              role="dialog"
              aria-label="Diff hunk details"
              class="absolute z-50 min-w-[240px] max-w-sm rounded-lg border border-line bg-surface p-3 shadow-xl font-mono text-xs"
              style={{
                top: `${popover().top}px`,
                left: `${popover().left}px`,
              }}
            >
              <div class="flex items-center justify-between gap-2 border-b border-line pb-1.5 mb-2">
                <span class="font-semibold text-foreground">
                  {hunk.type === "added"
                    ? "Added lines"
                    : hunk.type === "modified"
                    ? "Modified lines"
                    : "Removed lines"}
                </span>
                <button
                  type="button"
                  class="focus-ring text-muted hover:text-foreground"
                  onClick={() => setHunkPopover(null)}
                  aria-label="Close diff popover"
                >
                  <IconClose size={12} />
                </button>
              </div>

              <Show
                when={hunk.originalLines.length > 0}
                fallback={
                  <div class="mb-2 text-[11px] text-muted italic">
                    New lines (not in HEAD)
                  </div>
                }
              >
                <div class="mb-2 max-h-36 overflow-y-auto rounded border border-line/60 bg-background p-2 text-[11px] text-foreground">
                  <div class="mb-1 text-[10px] uppercase tracking-wider text-muted">
                    HEAD version:
                  </div>
                  <pre class="whitespace-pre font-mono">{hunk.originalLines.join("\n")}</pre>
                </div>
              </Show>

              <div class="flex items-center justify-end gap-2 pt-1 border-t border-line/40">
                <button
                  type="button"
                  class="focus-ring rounded border border-line bg-surface px-2.5 py-1 text-xs font-medium text-foreground hover:bg-raised"
                  onClick={() => {
                    if (!view) return;
                    if ((props.path ?? "") !== currentDiffPath) return;
                    const change = computeRevertChange(hunk, view.state.doc);
                    view.dispatch({ changes: change });
                    props.onChange?.(view.state.doc.toString());
                    setHunkPopover(null);
                  }}
                >
                  Revert hunk
                </button>
              </div>
            </div>
          );
        }}
      </Show>
    </div>
  );
}
