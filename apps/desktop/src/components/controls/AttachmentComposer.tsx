import { For, Show, createEffect, createMemo, createSignal, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { IconArrowUp, IconChevronDown, IconPlus } from "../icons";
import AttachmentChip, { attachmentFromPath, isImageAttachment, type ChatAttachment } from "./AttachmentChip";
import ModelPanel from "./ModelPanel";
import SlashPalette from "./SlashPalette";
import { bareAlias } from "../agentCommands";
import type { CatalogCommand, CommandCatalog } from "../../bindings";

export type { ChatAttachment } from "./AttachmentChip";
// Claude TUI style: attaching an image drops a friendly [Image #N] marker into the draft at the
// caret, so the reference reads where the user put it rather than always at the end. The real
// path never appears in the visible draft - attachmentPrompt swaps each marker for its
// "Attached file:" line, in place, only at send time.
export function imageMarker(n: number): string { return `[Image #${n}]`; }
export function insertMarkerAtCaret(text: string, caret: number, marker: string): { text: string; caret: number } {
  const before = text.slice(0, caret).replace(/\s+$/, "");
  const after = text.slice(caret).replace(/^\s+/, "");
  const prefix = before ? `${before}\n\n` : "";
  const suffix = after ? `\n\n${after}` : "";
  return { text: `${prefix}${marker}${suffix}`, caret: prefix.length + marker.length };
}
function escapeRegExp(value: string): string { return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"); }
export function removeMarker(text: string, n: number): string {
  const marker = escapeRegExp(imageMarker(n));
  return text
    .replace(new RegExp(`\\n\\n${marker}\\n\\n`), "\n\n")
    .replace(new RegExp(`^${marker}\\n\\n`), "")
    .replace(new RegExp(`\\n\\n${marker}$`), "")
    .replace(new RegExp(`^${marker}$`), "");
}
export function renumberMarkersAfterRemoval(text: string, removedNumber: number, totalBeforeRemoval: number): string {
  let next = text;
  for (let k = removedNumber + 1; k <= totalBeforeRemoval; k++) next = next.split(imageMarker(k)).join(imageMarker(k - 1));
  return next;
}
export function attachmentPrompt(text: string, files: ChatAttachment[]): string {
  let body = text;
  const trailing: ChatAttachment[] = [];
  let imageNumber = 0;
  for (const file of files) {
    if (isImageAttachment(file)) {
      imageNumber += 1;
      const marker = imageMarker(imageNumber);
      if (body.includes(marker)) { body = body.replace(marker, `Attached file: ${JSON.stringify(file.path)}`); continue; }
    }
    trailing.push(file);
  }
  return [body.trim(), ...trailing.map((file) => `Attached file: ${JSON.stringify(file.path)}`)].filter(Boolean).join("\n\n");
}
// A bare "/" as the first character opens the palette; anything with whitespace after it is no
// longer a single in-progress command token (an argument, or an ordinary sentence that happens to
// contain a slash), so the palette closes on its own the moment that happens.
function paletteQueryOf(text: string): string | null {
  return /^\/[^\s]*$/.test(text) ? text.slice(1) : null;
}
function matchesQuery(command: CatalogCommand, query: string): boolean {
  const lower = query.toLowerCase();
  return command.name.toLowerCase().startsWith(lower) || bareAlias(command.name).toLowerCase().startsWith(lower);
}
export default function AttachmentComposer(props: {
  kind: string; model?: string; disabled: boolean; busy: boolean;
  onSend: (text: string) => Promise<boolean>;
  catalog: CommandCatalog;
  hasTerminalFallback: boolean;
  onSelectModel: (id: string) => void;
  onModelFallback: () => void;
}) {
  const [text, setText] = createSignal("");
  const [files, setFiles] = createSignal<ChatAttachment[]>([]);
  const [staging, setStaging] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const [dragging, setDragging] = createSignal(false);
  // TUI-style history: Up/Down recall previously sent messages, oldest to newest, only ever
  // starting from an empty composer so an in-progress draft can never be clobbered by an arrow
  // press. Local to this mount, like the draft itself - a fresh pane starts with empty history.
  const [history, setHistory] = createSignal<string[]>([]);
  const [historyIndex, setHistoryIndex] = createSignal<number | null>(null);
  const [modelOpen, setModelOpen] = createSignal(false);
  const [paletteDismissed, setPaletteDismissed] = createSignal(false);
  const [highlightedIndex, setHighlightedIndex] = createSignal(0);
  let dragDepth = 0;
  let field!: HTMLTextAreaElement;
  let modelButtonRef!: HTMLButtonElement;
  const resize = () => {
    if (!field) return;
    field.style.height = "0px";
    field.style.height = `${Math.max(40, Math.min(field.scrollHeight, 160))}px`;
  };
  createEffect(() => { text(); resize(); });
  onMount(resize);
  const locked = () => props.disabled || props.busy || staging();

  const paletteQuery = createMemo(() => paletteQueryOf(text()));
  const filteredCommands = createMemo(() => {
    const query = paletteQuery();
    if (query === null) return [];
    return query ? props.catalog.commands.filter((command) => matchesQuery(command, query)) : props.catalog.commands;
  });
  // The palette owns the arrows only while it is genuinely open; dismissing it (Escape) or
  // opening the model panel hands them straight back to history, with no dead middle state.
  const paletteOpen = () => paletteQuery() !== null && !paletteDismissed() && !modelOpen();
  createEffect(() => { filteredCommands(); setHighlightedIndex(0); });

  const add = (attachments: ChatAttachment[]) => {
    const fresh = attachments.filter((file) => !files().some((old) => old.path === file.path));
    if (!fresh.length) return;
    let nextText = text();
    let caret = field ? field.selectionStart ?? nextText.length : nextText.length;
    let imageNumber = files().filter(isImageAttachment).length;
    for (const file of fresh) {
      if (!isImageAttachment(file)) continue;
      imageNumber += 1;
      const result = insertMarkerAtCaret(nextText, caret, imageMarker(imageNumber));
      nextText = result.text;
      caret = result.caret;
    }
    if (nextText !== text()) setText(nextText);
    setFiles((current) => [...current, ...fresh]);
  };
  function removeFile(file: ChatAttachment) {
    if (isImageAttachment(file)) {
      const images = files().filter(isImageAttachment);
      const removedNumber = images.indexOf(file) + 1;
      setText((current) => renumberMarkersAfterRemoval(removeMarker(current, removedNumber), removedNumber, images.length));
    }
    setFiles((current) => current.filter((entry) => entry !== file));
  }
  async function pick() {
    if (locked()) return;
    setStaging(true); setError(undefined);
    try {
      const paths = await open({ multiple: true, title: "Attach images or files" });
      if (paths) add((Array.isArray(paths) ? paths : [paths]).map(attachmentFromPath));
    } catch { setError("Could not attach files. Try choosing them again."); }
    finally { setStaging(false); }
  }
  // The one pipeline pasted and dropped bytes both go through: saved into the same app-data
  // attachments directory paste already used, then handed to add() for the same caret marker and
  // chip treatment. Neither caller does its own staging or path handling.
  async function stageFiles(incoming: File[]) {
    if (!incoming.length || locked()) return;
    setStaging(true); setError(undefined);
    try {
      for (const file of incoming) {
        if (file.size > 20 * 1024 * 1024) throw new Error("Choose an attachment smaller than 20 MB.");
        const path = await invoke<string>("save_chat_attachment", { name: file.name, bytes: Array.from(new Uint8Array(await file.arrayBuffer())) });
        add([{ path, name: file.name }]);
      }
    } catch (cause) { setError(`Could not save attachment. ${String(cause)}`); }
    finally { setStaging(false); }
  }
  function paste(event: ClipboardEvent) {
    const pasted = Array.from(event.clipboardData?.files ?? []);
    if (!pasted.length) return;
    event.preventDefault();
    void stageFiles(pasted);
  }
  function hasFiles(event: DragEvent) {
    return Array.from(event.dataTransfer?.types ?? []).includes("Files");
  }
  function dragEnter(event: DragEvent) {
    if (!hasFiles(event)) return;
    event.preventDefault();
    dragDepth += 1;
    setDragging(true);
  }
  function dragOver(event: DragEvent) {
    if (!hasFiles(event)) return;
    event.preventDefault();
  }
  function dragLeave(event: DragEvent) {
    if (!hasFiles(event)) return;
    dragDepth = Math.max(0, dragDepth - 1);
    if (dragDepth === 0) setDragging(false);
  }
  function drop(event: DragEvent) {
    if (!hasFiles(event)) return;
    event.preventDefault();
    dragDepth = 0;
    setDragging(false);
    void stageFiles(Array.from(event.dataTransfer?.files ?? []));
  }
  async function send() {
    if (locked() || (!text().trim() && !files().length)) return;
    const draft = text();
    if (await props.onSend(attachmentPrompt(draft, files()))) {
      if (draft.trim()) setHistory((current) => [...current, draft].slice(-50));
      setText(""); setFiles([]); setError(undefined); setHistoryIndex(null);
    }
  }
  function runPaletteCommand(command: CatalogCommand) {
    setPaletteDismissed(true);
    setText(`/${command.name}`);
    void send();
  }
  function recallOlder() {
    const items = history();
    if (!items.length) return;
    const current = historyIndex();
    const next = current === null ? items.length - 1 : Math.max(0, current - 1);
    setHistoryIndex(next);
    setText(items[next]);
  }
  function recallNewer() {
    const current = historyIndex();
    if (current === null) return;
    const items = history();
    if (current >= items.length - 1) { setHistoryIndex(null); setText(""); return; }
    setHistoryIndex(current + 1);
    setText(items[current + 1]);
  }
  function onModelChipClick() {
    if (locked()) return;
    if (props.catalog.models.length > 0) { setModelOpen(true); return; }
    if (props.hasTerminalFallback) props.onModelFallback();
  }
  return <form class="conversation-compose" onSubmit={(event) => { event.preventDefault(); void send(); }}>
    <div class="conversation-reply rounded" classList={{ "is-drag-target": dragging() }}
      onDragEnter={dragEnter} onDragOver={dragOver} onDragLeave={dragLeave} onDrop={drop}>
      <Show when={dragging()}><div class="conversation-reply-drop" aria-hidden="true">Drop to attach</div></Show>
      <textarea ref={field} aria-label={`Reply to ${props.kind}`} placeholder={props.disabled ? "Answer the prompt first" : "Ask a question or describe a change…"} disabled={props.disabled || props.busy} value={text()} rows={1}
        onPaste={(event) => paste(event)}
        onInput={(event) => { setText(event.currentTarget.value); if (historyIndex() !== null) setHistoryIndex(null); setPaletteDismissed(false); }}
        onKeyDown={(event) => {
          if (event.key === "Escape" && paletteOpen()) { event.preventDefault(); setPaletteDismissed(true); return; }
          if (event.key === "Escape" && modelOpen()) { event.preventDefault(); setModelOpen(false); return; }
          if (paletteOpen() && !event.shiftKey && !event.altKey && !event.metaKey && !event.ctrlKey) {
            const count = filteredCommands().length;
            if (event.key === "ArrowDown") { event.preventDefault(); setHighlightedIndex((i) => count ? (i + 1) % count : 0); return; }
            if (event.key === "ArrowUp") { event.preventDefault(); setHighlightedIndex((i) => count ? (i - 1 + count) % count : 0); return; }
            // Nothing to highlight (an empty catalog, or a query that matches nothing): fall
            // through to the normal Enter-send below instead of silently eating the keystroke -
            // resolveCommand there decides one-shot vs. the terminal fallback on its own.
            if (event.key === "Enter" && !event.isComposing && filteredCommands().length > 0) {
              event.preventDefault();
              runPaletteCommand(filteredCommands()[highlightedIndex()]);
              return;
            }
          }
          if (event.key === "Enter" && !event.shiftKey && !event.isComposing) { event.preventDefault(); void send(); return; }
          if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
          if (event.shiftKey || event.altKey || event.metaKey || event.ctrlKey) return;
          // Never steal the arrows while editing multiline text - a recalled entry that itself
          // spans lines falls back to ordinary cursor movement the moment it's on screen.
          if (text().includes("\n")) return;
          if (event.key === "ArrowUp") {
            if (text() !== "" && historyIndex() === null) return; // empty composer only
            event.preventDefault();
            recallOlder();
          } else {
            if (historyIndex() === null) return; // not browsing: normal cursor behavior
            event.preventDefault();
            recallNewer();
          }
        }} />
      <Show when={paletteOpen() && field}>
        <SlashPalette
          commands={filteredCommands()}
          query={paletteQuery() ?? ""}
          highlightedIndex={highlightedIndex()}
          anchor={field}
          onHighlight={setHighlightedIndex}
          onRun={runPaletteCommand}
        />
      </Show>
      <div class="composer-actions">
        <div class="composer-leading">
        <button class="focus-ring rounded composer-attach" type="button" aria-label="Attach images or files" title="Attach images or files. You can also paste an image." disabled={locked()} onClick={() => void pick()}><IconPlus size={16} /></button>
        <Show when={files().length}><ul class="attachment-list" aria-label="Attachments"><For each={files()}>{(file) => <li><AttachmentChip file={file} disabled={locked()} onRemove={() => removeFile(file)} /></li>}</For></ul></Show>
        </div>
        <div class="composer-trailing">
        <Show when={props.catalog.model_command} fallback={<span class="composer-agent">{props.kind}<Show when={props.model}><span class="text-muted"> · {props.model}</span></Show></span>}>
          <button ref={modelButtonRef} type="button" class="composer-model focus-ring rounded" aria-label={`Change ${props.kind} model`} aria-haspopup="menu" aria-expanded={modelOpen()} title={props.model ?? "Choose a model"} disabled={locked()} onClick={onModelChipClick}>
            <span class="composer-agent">{props.model ?? props.kind}</span><IconChevronDown size={12} />
          </button>
        </Show>
        <button class="focus-ring rounded composer-send" type="submit" aria-label="Send reply" disabled={locked() || (!text().trim() && !files().length)}><IconArrowUp size={16} /></button>
        </div>
      </div>
    </div>
    <Show when={modelOpen() && modelButtonRef}>
      <ModelPanel models={props.catalog.models} anchor={modelButtonRef} onSelect={(id) => { setModelOpen(false); props.onSelectModel(id); }} onClose={() => setModelOpen(false)} />
    </Show>
    <p class="composer-hint" classList={{ "is-staging": staging() }} aria-live="polite">{staging() ? "Saving attachment…" : "/ for commands · Shift + Enter for a new line"}</p>
    <Show when={error()}><p class="text-xs text-fault" role="alert">{error()}</p></Show>
  </form>;
}
