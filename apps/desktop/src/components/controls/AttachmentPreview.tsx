import { Show, createEffect, createResource, createSignal } from "solid-js";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import AttachmentChip, { type ChatAttachment } from "./AttachmentChip";

export default function AttachmentPreview(props: { file: ChatAttachment; number: number; onResize?: () => void }) {
  const [loaded, setLoaded] = createSignal(false);
  const [failed, setFailed] = createSignal(false);
  const [source] = createResource(() => props.file.path, async (path) => {
    try { return convertFileSrc(await invoke<string>("allow_chat_attachment_preview", { path })); }
    catch { return undefined; }
  });
  createEffect(() => { props.file.path; setLoaded(false); setFailed(false); });
  return <span class="attachment-preview" title={props.file.path}>
    <Show when={!loaded() || failed()}><AttachmentChip file={props.file} /></Show>
    <Show when={!failed() && source()}>{(url) => <>
      <img class="rounded" src={url()} alt={`Image #${props.number}: ${props.file.name}`} style={{ display:loaded() ? "block" : "none" }} onLoad={() => { setLoaded(true); props.onResize?.(); }} onError={() => { setFailed(true); props.onResize?.(); }} />
      <Show when={loaded()}><span class="attachment-preview-number" aria-hidden="true">{props.number}</span></Show>
    </>}</Show>
  </span>;
}
