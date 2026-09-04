import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { syntaxHighlighting } from "@codemirror/language";
import { appTheme, highlightStyle, resolveLanguageSupport } from "../CodeEditor";

export interface MiniCodeBlockProps {
  code: string;
  language?: string;
}

export default function MiniCodeBlock(props: MiniCodeBlockProps) {
  let containerRef: HTMLDivElement | undefined;
  let view: EditorView | null = null;
  const [copied, setCopied] = createSignal(false);
  const [cmReady, setCmReady] = createSignal(false);

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(props.code);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // Fallback or ignore
    }
  }

  onMount(() => {
    let active = true;
    const lang = props.language?.trim().toLowerCase() || "";

    if (!lang || !containerRef) {
      return;
    }

    void resolveLanguageSupport("file." + lang, null, lang).then((support) => {
      if (!active || !containerRef || !support) return;

      try {
        const state = EditorState.create({
          doc: props.code,
          extensions: [
            EditorState.readOnly.of(true),
            EditorView.editable.of(false),
            EditorView.lineWrapping,
            support,
            appTheme,
            syntaxHighlighting(highlightStyle),
          ],
        });

        view = new EditorView({
          state,
          parent: containerRef,
        });
        setCmReady(true);
      } catch {
        // Fall back to pre/code
        setCmReady(false);
      }
    });

    onCleanup(() => {
      active = false;
      if (view) {
        view.destroy();
        view = null;
      }
    });
  });

  return (
    <div class="group relative my-4 overflow-hidden rounded-md border border-line bg-surface/70">
      {/* Language badge & copy button header */}
      <div class="flex h-7 items-center justify-between border-b border-line/60 bg-raised/30 px-3 font-mono text-[11px] text-muted">
        <span>{props.language || "text"}</span>
        <button
          type="button"
          onClick={handleCopy}
          class="focus-ring flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] text-muted transition-colors hover:bg-raised hover:text-foreground"
          title="Copy code"
        >
          <Show
            when={copied()}
            fallback={
              <>
                <svg
                  class="size-3"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <rect width="14" height="14" x="8" y="8" rx="2" ry="2" />
                  <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" />
                </svg>
                <span>Copy</span>
              </>
            }
          >
            <svg
              class="size-3 text-signal"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path d="M20 6 9 17l-5-5" />
            </svg>
            <span class="text-signal">Copied</span>
          </Show>
        </button>
      </div>

      {/* CodeMirror container */}
      <div
        ref={containerRef}
        class="min-h-0"
        style={{ display: cmReady() ? "block" : "none" }}
      />

      {/* Fallback pre/code */}
      <Show when={!cmReady()}>
        <pre class="overflow-x-auto p-3 font-mono text-xs leading-relaxed text-foreground">
          <code>{props.code}</code>
        </pre>
      </Show>
    </div>
  );
}
