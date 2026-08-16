import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { css } from "@codemirror/lang-css";
import { html } from "@codemirror/lang-html";
import { javascript } from "@codemirror/lang-javascript";
import { json } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { python } from "@codemirror/lang-python";
import { rust } from "@codemirror/lang-rust";
import {
  HighlightStyle,
  bracketMatching,
  indentOnInput,
  syntaxHighlighting,
  type LanguageSupport,
} from "@codemirror/language";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import { Compartment, EditorState } from "@codemirror/state";
import {
  EditorView,
  drawSelection,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  type KeyBinding,
} from "@codemirror/view";
import { tags as t } from "@lezer/highlight";
import { createEffect, onCleanup, onMount } from "solid-js";

/// The languages this wrapper can highlight - one per `@codemirror/lang-*` package pulled in for
/// D3. Anything else (yaml, toml, shell scripts, ...) falls back to `"plain"`: no language
/// package for those exists in this dependency set, and plain text is the honest fallback rather
/// than mis-highlighting them under a similar-looking grammar.
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
  | "plain";

/// Resolves a file path (or bare extension) to the language CodeMirror should highlight it as.
/// Matched by the extension only - a path with no recognized extension, or no extension at all,
/// resolves to `"plain"`.
export function extensionToLanguage(path: string): EditorLanguage {
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
    // No language package covers these in the D3 dependency set (yaml, toml, shell, plaintext,
    // and anything unrecognized) - render as plain text rather than guessing.
    default:
      return "plain";
  }
}

function languageSupport(language: EditorLanguage): LanguageSupport | null {
  switch (language) {
    case "javascript":
      return javascript();
    case "jsx":
      return javascript({ jsx: true });
    case "typescript":
      return javascript({ typescript: true });
    case "tsx":
      return javascript({ jsx: true, typescript: true });
    case "json":
      return json();
    case "rust":
      return rust();
    case "markdown":
      return markdown();
    case "python":
      return python();
    case "css":
      return css();
    case "html":
      return html();
    case "plain":
      return null;
  }
}

// The app's whole visual identity runs on four accent roles (--signal, --attention, --fault,
// --muted) layered over --surface/--foreground - see src/index.css. Syntax highlighting reuses
// exactly those roles instead of inventing a rainbow token palette: keywords/tags read as
// "structure" (signal), literals read as "data" (attention), comments/punctuation recede (muted),
// and only genuinely invalid syntax reaches for --fault. Every value below is a var(...) or
// color-mix(...) reference, so all six themes in index.css repaint the editor automatically.
const highlightStyle = HighlightStyle.define([
  { tag: t.comment, color: "var(--muted)", fontStyle: "italic" },
  { tag: t.lineComment, color: "var(--muted)", fontStyle: "italic" },
  { tag: t.blockComment, color: "var(--muted)", fontStyle: "italic" },
  { tag: t.docComment, color: "var(--muted)", fontStyle: "italic" },

  { tag: [t.keyword, t.controlKeyword, t.moduleKeyword, t.operatorKeyword], color: "var(--signal)" },
  { tag: [t.tagName, t.angleBracket], color: "var(--signal)" },
  { tag: [t.definitionKeyword, t.definition(t.variableName)], color: "var(--signal)", fontWeight: 600 },

  {
    tag: [t.string, t.special(t.string), t.regexp, t.character],
    color: "color-mix(in srgb, var(--attention) 88%, var(--foreground))",
  },
  { tag: [t.number, t.bool, t.atom, t.null], color: "var(--attention)" },

  { tag: [t.function(t.variableName), t.function(t.propertyName)], color: "var(--foreground)", fontWeight: 600 },
  { tag: [t.className, t.typeName, t.namespace], color: "var(--foreground)", fontWeight: 600 },
  { tag: [t.propertyName, t.attributeName], color: "color-mix(in srgb, var(--foreground) 78%, var(--muted))" },
  { tag: t.variableName, color: "var(--foreground)" },

  { tag: [t.punctuation, t.bracket, t.separator], color: "var(--muted)" },
  { tag: t.operator, color: "color-mix(in srgb, var(--foreground) 80%, var(--muted))" },
  { tag: t.meta, color: "var(--muted)" },
  { tag: t.heading, color: "var(--signal)", fontWeight: 700 },
  { tag: t.link, color: "var(--signal)", textDecoration: "underline" },
  { tag: t.strong, fontWeight: 700 },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.strikethrough, textDecoration: "line-through" },

  { tag: t.invalid, color: "var(--fault)" },
]);

// EditorView.theme values reference the app's CSS custom properties (var(...) / color-mix(...))
// instead of hardcoded colors, so the editor repaints for free under every theme class in
// index.css - no CodeMirror theme package involved. Font size matches TerminalPane's default
// terminal size (DEFAULT_TERMINAL_APPEARANCE.fontSize in theme.ts is 12) for a consistent feel
// between the two monospace surfaces in the app.
const appTheme = EditorView.theme({
  "&": {
    height: "100%",
    color: "var(--foreground)",
    backgroundColor: "var(--surface)",
    fontFamily: "var(--font-mono)",
    fontSize: "12px",
  },
  ".cm-scroller": {
    fontFamily: "var(--font-mono)",
    lineHeight: "1.45",
  },
  ".cm-content": {
    caretColor: "var(--signal)",
  },
  ".cm-cursor, .cm-dropCursor": {
    borderLeftColor: "var(--signal)",
  },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
    backgroundColor: "color-mix(in srgb, var(--signal) 28%, transparent)",
  },
  ".cm-gutters": {
    backgroundColor: "var(--surface)",
    color: "var(--muted)",
    border: "none",
    borderRight: "1px solid var(--line)",
  },
  ".cm-lineNumbers .cm-gutterElement": {
    color: "var(--muted)",
  },
  ".cm-activeLine": {
    backgroundColor: "color-mix(in srgb, var(--signal) 6%, var(--surface))",
  },
  ".cm-activeLineGutter": {
    backgroundColor: "color-mix(in srgb, var(--signal) 10%, var(--surface))",
    color: "var(--foreground)",
  },
  ".cm-matchingBracket, .cm-nonmatchingBracket": {
    backgroundColor: "color-mix(in srgb, var(--signal) 20%, transparent)",
    outline: "1px solid color-mix(in srgb, var(--signal) 40%, transparent)",
  },
  ".cm-selectionMatch": {
    backgroundColor: "color-mix(in srgb, var(--attention) 22%, transparent)",
  },
  ".cm-searchMatch": {
    backgroundColor: "color-mix(in srgb, var(--attention) 25%, transparent)",
    outline: "1px solid color-mix(in srgb, var(--attention) 45%, transparent)",
  },
  ".cm-searchMatch.cm-searchMatch-selected": {
    backgroundColor: "color-mix(in srgb, var(--attention) 45%, transparent)",
  },
  ".cm-panels": {
    backgroundColor: "var(--raised)",
    color: "var(--foreground)",
  },
  "&.cm-focused": {
    outline: "none",
  },
});

export interface CodeEditorProps {
  /// The document content. Changing this after mount replaces the editor's document - but only
  /// when it actually differs from the live doc, so the app's own onChange echo (parent state
  /// updated from our onChange, then handed straight back down as this prop) never bounces back
  /// in as a second, redundant replace-the-whole-doc transaction.
  value: string;
  /// File path (or bare extension) used to resolve syntax highlighting via `extensionToLanguage`.
  path: string;
  onChange?: (content: string) => void;
  /// Fired for Mod-s (Cmd-S / Ctrl-S) pressed while the editor has focus. Bound inside
  /// CodeMirror's own keymap with `preventDefault: true` so the surrounding Tauri webview never
  /// sees the keystroke and cannot intercept it as a native "save page" shortcut.
  onSave?: () => void;
  readOnly?: boolean;
  class?: string;
}

export default function CodeEditor(props: CodeEditorProps) {
  let container!: HTMLDivElement;
  let view: EditorView | undefined;
  const readOnlyCompartment = new Compartment();
  const languageCompartment = new Compartment();
  // Set for the span of a dispatch triggered by our own `value`-prop sync effect, so the
  // updateListener below can tell "the parent handed us new content" apart from "the user typed"
  // and skip re-firing onChange for changes that originated from the parent in the first place.
  let applyingExternalValue = false;
  // Mirrors the doc content the view was last synced to (by us or by the user typing), so the
  // value-sync effect below can no-op on the very next tick after firing onChange - Solid re-runs
  // the effect once `props.value` round-trips back down from the parent's state update.
  let lastKnownDoc = props.value;

  onMount(() => {
    const saveBinding: KeyBinding = {
      key: "Mod-s",
      preventDefault: true,
      run: () => {
        props.onSave?.();
        return true;
      },
    };

    const support = languageSupport(extensionToLanguage(props.path));

    const state = EditorState.create({
      doc: props.value,
      extensions: [
        lineNumbers(),
        highlightActiveLineGutter(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        drawSelection(),
        bracketMatching(),
        indentOnInput(),
        history(),
        readOnlyCompartment.of(EditorState.readOnly.of(props.readOnly ?? false)),
        keymap.of([saveBinding, ...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab]),
        languageCompartment.of(support ? [support] : []),
        appTheme,
        syntaxHighlighting(highlightStyle),
        EditorView.updateListener.of((update) => {
          if (!update.docChanged) return;
          lastKnownDoc = update.state.doc.toString();
          if (applyingExternalValue) return;
          props.onChange?.(lastKnownDoc);
        }),
      ],
    });

    view = new EditorView({ state, parent: container });
  });

  // Keep the live doc in sync with an externally-updated `value` prop (e.g. the file changing on
  // disk, or a discard/reload action in D4) without re-creating the EditorView - a fresh view
  // would drop cursor position, scroll offset, and undo history on every keystroke's re-render.
  // Guarded on `lastKnownDoc` (not the view's live doc) so the parent handing our own onChange
  // value straight back down as `props.value` is recognized as an echo and never re-dispatched.
  createEffect(() => {
    const next = props.value;
    if (!view || next === lastKnownDoc) return;
    lastKnownDoc = next;
    applyingExternalValue = true;
    try {
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: next },
      });
    } finally {
      applyingExternalValue = false;
    }
  });

  createEffect(() => {
    const readOnly = props.readOnly ?? false;
    view?.dispatch({ effects: readOnlyCompartment.reconfigure(EditorState.readOnly.of(readOnly)) });
  });

  createEffect(() => {
    const support = languageSupport(extensionToLanguage(props.path));
    view?.dispatch({ effects: languageCompartment.reconfigure(support ? [support] : []) });
  });

  onCleanup(() => {
    view?.destroy();
    view = undefined;
  });

  return <div ref={container} class={props.class ?? "h-full w-full overflow-hidden"} />;
}
