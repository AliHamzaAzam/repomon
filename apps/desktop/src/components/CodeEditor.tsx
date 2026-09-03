import {
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  completionKeymap,
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
import { Compartment, EditorState, RangeSetBuilder } from "@codemirror/state";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  drawSelection,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightWhitespace,
  keymap,
  lineNumbers,
  type DecorationSet,
  type KeyBinding,
  type ViewUpdate,
} from "@codemirror/view";
import { tags as t } from "@lezer/highlight";
import { createEffect, onCleanup, onMount } from "solid-js";

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

export interface CodeEditorProps {
  value: string;
  path?: string;
  readOnly?: boolean;
  wrap?: boolean;
  whitespace?: boolean;
  languageOverride?: string;
  onChange?: (value: string) => void;
  onSave?: () => void;
  onCursorActivity?: (cursor: number, scrollTop: number) => void;
  initialCursor?: number;
  initialScrollTop?: number;
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

async function resolveLanguageSupport(
  path: string,
  content?: string,
  override?: string,
): Promise<LanguageSupport | null> {
  const target = override || matchSpecialFilename(path) || (content ? sniffShebang(content) : null);
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

function getSyncLanguageSupport(path: string, content?: string, override?: string): LanguageSupport | null {
  const target = override || matchSpecialFilename(path) || (content ? sniffShebang(content) : null);
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

const appTheme = EditorView.theme(
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
      caretColor: "var(--accent)",
      tabSize: 4,
    },
    "&.cm-focused .cm-cursor": {
      borderLeftColor: "var(--accent)",
      borderLeftWidth: "2px",
    },
    "&.cm-focused .cm-selectionBackground, ::selection": {
      backgroundColor: "color-mix(in srgb, var(--accent) 25%, transparent)",
    },
    ".cm-selectionMatch": {
      backgroundColor: "color-mix(in srgb, var(--accent) 18%, transparent)",
      borderRadius: "2px",
    },
    ".cm-gutters": {
      backgroundColor: "var(--surface)",
      color: "color-mix(in srgb, var(--muted) 60%, transparent)",
      borderRight: "1px solid var(--line)",
      paddingRight: "4px",
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
      borderColor: "var(--accent)",
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

const highlightStyle = HighlightStyle.define([
  { tag: t.keyword, color: "#c678dd" },
  { tag: [t.name, t.deleted, t.character, t.propertyName, t.macroName], color: "#e06c75" },
  { tag: [t.function(t.variableName), t.labelName], color: "#61afef" },
  { tag: [t.color, t.constant(t.name), t.standard(t.name)], color: "#d19a66" },
  { tag: [t.definition(t.name), t.separator], color: "#abb2bf" },
  {
    tag: [
      t.typeName,
      t.className,
      t.number,
      t.changed,
      t.annotation,
      t.modifier,
      t.self,
      t.namespace,
    ],
    color: "#e5c07b",
  },
  {
    tag: [t.operator, t.operatorKeyword, t.url, t.escape, t.regexp, t.link, t.special(t.string)],
    color: "#56b6c2",
  },
  { tag: [t.meta, t.comment], color: "#7f848e", fontStyle: "italic" },
  { tag: t.strong, fontWeight: "bold" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.strikethrough, textDecoration: "line-through" },
  { tag: t.link, color: "#61afef", textDecoration: "underline" },
  { tag: t.heading, fontWeight: "bold", color: "#e06c75" },
  { tag: [t.atom, t.bool, t.special(t.variableName)], color: "#d19a66" },
  { tag: [t.processingInstruction, t.string, t.inserted], color: "#98c379" },
  { tag: t.invalid, color: "#ffffff", backgroundColor: "#e05252" },
]);

export default function CodeEditor(props: CodeEditorProps) {
  let containerRef!: HTMLDivElement;
  let view: EditorView | undefined;
  let lastKnownDoc = props.value;
  let applyingExternalValue = false;

  const readOnlyCompartment = new Compartment();
  const languageCompartment = new Compartment();
  const wrapCompartment = new Compartment();
  const whitespaceCompartment = new Compartment();
  const indentUnitCompartment = new Compartment();

  const saveBinding: KeyBinding = {
    key: "Mod-s",
    run: () => {
      props.onSave?.();
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

    const initialSupport = getSyncLanguageSupport(props.path ?? "", props.value, props.languageOverride);
    const unit = detectIndentUnit(props.value, props.path ?? "");

    const state = EditorState.create({
      doc: props.value,
      extensions: [
        lineNumbers(),
        foldGutter(),
        highlightActiveLineGutter(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        drawSelection(),
        bracketMatching(),
        closeBrackets(),
        indentOnInput(),
        autocompletion(),
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
          }
          if (update.selectionSet || update.docChanged) {
            const head = update.state.selection.main.head;
            props.onCursorActivity?.(head, update.view.scrollDOM.scrollTop);
          }
        }),
      ],
    });

    view = new EditorView({
      state,
      parent: containerRef,
    });

    if (props.initialCursor !== undefined && props.initialCursor > 0) {
      const pos = Math.min(props.initialCursor, view.state.doc.length);
      view.dispatch({ selection: { anchor: pos } });
    }
    if (props.initialScrollTop !== undefined && props.initialScrollTop > 0) {
      view.scrollDOM.scrollTop = props.initialScrollTop;
    }

    if (!initialSupport) {
      void resolveLanguageSupport(props.path ?? "", props.value, props.languageOverride).then((support) => {
        if (support && view) {
          view.dispatch({ effects: languageCompartment.reconfigure([support]) });
        }
      });
    }

    onCleanup(() => {
      view?.destroy();
      view = undefined;
    });
  });

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
    const path = props.path ?? "";
    const override = props.languageOverride;
    const content = props.value;
    if (!view) return;

    const sync = getSyncLanguageSupport(path, content, override);
    if (sync) {
      view.dispatch({ effects: languageCompartment.reconfigure([sync]) });
    } else {
      void resolveLanguageSupport(path, content, override).then((support) => {
        if (view) {
          view.dispatch({ effects: languageCompartment.reconfigure(support ? [support] : []) });
        }
      });
    }
  });

  return (
    <div
      ref={containerRef}
      class={props.class ? `min-h-0 flex-1 overflow-hidden ${props.class}` : "min-h-0 flex-1 overflow-hidden"}
    />
  );
}
