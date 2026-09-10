import { For, Show, createEffect, createSignal, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { IconArrowUp, IconPlus } from "../icons";
import AttachmentChip, { attachmentFromPath, type ChatAttachment } from "./AttachmentChip";

export type { ChatAttachment } from "./AttachmentChip";
export function attachmentPrompt(text: string, files: ChatAttachment[]): string {
  return [text.trim(), ...files.map((file) => `Attached file: ${JSON.stringify(file.path)}`)].filter(Boolean).join("\n\n");
}
export default function AttachmentComposer(props: {
  kind: string; model?: string; disabled: boolean; busy: boolean;
  onSend: (text: string) => Promise<boolean>;
}) {
  const [text, setText] = createSignal("");
  const [files, setFiles] = createSignal<ChatAttachment[]>([]);
  const [staging, setStaging] = createSignal(false);
  const [error, setError] = createSignal<string>();
  let field!: HTMLTextAreaElement;
  const resize = () => {
    if (!field) return;
    field.style.height = "0px";
    field.style.height = `${Math.max(40, Math.min(field.scrollHeight, 160))}px`;
  };
  createEffect(() => { text(); resize(); });
  onMount(resize);
  const locked = () => props.disabled || props.busy || staging();
  const add = (attachments: ChatAttachment[]) => setFiles((current) => [...current, ...attachments.filter((file) => !current.some((old) => old.path === file.path))]);
  async function pick() {
    if (locked()) return;
    setStaging(true); setError(undefined);
    try {
      const paths = await open({ multiple: true, title: "Attach images or files" });
      if (paths) add((Array.isArray(paths) ? paths : [paths]).map(attachmentFromPath));
    } catch { setError("Could not attach files. Try choosing them again."); }
    finally { setStaging(false); }
  }
  async function paste(event: ClipboardEvent) {
    const pasted = Array.from(event.clipboardData?.files ?? []);
    if (!pasted.length) return;
    event.preventDefault();
    if (locked()) return;
    setStaging(true); setError(undefined);
    try {
      for (const file of pasted) {
        if (file.size > 20 * 1024 * 1024) throw new Error("Choose an attachment smaller than 20 MB.");
        const path = await invoke<string>("save_chat_attachment", { name: file.name, bytes: Array.from(new Uint8Array(await file.arrayBuffer())) });
        add([{ path, name: file.name }]);
      }
    } catch (cause) { setError(`Could not save attachment. ${String(cause)}`); }
    finally { setStaging(false); }
  }
  async function send() {
    if (locked() || (!text().trim() && !files().length)) return;
    if (await props.onSend(attachmentPrompt(text(), files()))) { setText(""); setFiles([]); setError(undefined); }
  }
  return <form class="conversation-compose" onSubmit={(event) => { event.preventDefault(); void send(); }}>
    <div class="conversation-reply rounded">
      <textarea ref={field} aria-label={`Reply to ${props.kind}`} placeholder={props.disabled ? "Answer the prompt first" : "Ask a question or describe a change…"} disabled={props.disabled || props.busy} value={text()} rows={1}
        onPaste={(event) => void paste(event)} onInput={(event) => setText(event.currentTarget.value)}
        onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey && !event.isComposing) { event.preventDefault(); void send(); } }} />
      <div class="composer-actions">
        <div class="composer-leading">
        <button class="focus-ring rounded composer-attach" type="button" aria-label="Attach images or files" title="Attach images or files. You can also paste an image." disabled={locked()} onClick={() => void pick()}><IconPlus size={16} /></button>
        <Show when={files().length}><ul class="attachment-list" aria-label="Attachments"><For each={files()}>{(file) => <li><AttachmentChip file={file} disabled={locked()} onRemove={() => setFiles((current) => current.filter((entry) => entry !== file))} /></li>}</For></ul></Show>
        </div>
        <div class="composer-trailing">
        <span class="composer-agent">{props.kind}<Show when={props.model}><span class="text-muted"> · {props.model}</span></Show></span>
        <button class="focus-ring rounded composer-send" type="submit" aria-label="Send reply" disabled={locked() || (!text().trim() && !files().length)}><IconArrowUp size={16} /></button>
        </div>
      </div>
    </div>
    <p class="composer-hint" classList={{ "is-staging": staging() }} aria-live="polite">{staging() ? "Saving attachment…" : "Shift + Enter for a new line"}</p>
    <Show when={error()}><p class="text-xs text-fault" role="alert">{error()}</p></Show>
  </form>;
}
