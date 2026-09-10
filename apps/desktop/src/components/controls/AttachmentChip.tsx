import { Show } from "solid-js";
import { IconClose, IconFile, IconFileImage } from "../icons";

export type ChatAttachment = { path: string; name: string };
export function attachmentFromPath(path: string): ChatAttachment {
  return { path, name: path.split(/[\\/]/).pop() || "Attachment" };
}
export const isImageAttachment = (file: ChatAttachment) => /\.(png|jpe?g|gif|webp|heic|avif|bmp|tiff?)$/i.test(file.name);
export default function AttachmentChip(props: { file: ChatAttachment; disabled?: boolean; onRemove?: () => void }) {
  return <span class="attachment-chip rounded" title={props.file.path}>
    <Show when={isImageAttachment(props.file)} fallback={<IconFile size={12} />}><IconFileImage size={12} /></Show>
    <span>{props.file.name}</span><span class="attachment-type text-muted">{props.file.name.includes(".") ? props.file.name.split(".").pop()?.toUpperCase() : "File"}</span>
    <Show when={props.onRemove}><button type="button" class="focus-ring rounded" disabled={props.disabled} aria-label={`Remove ${props.file.name}`} onClick={() => props.onRemove?.()}><IconClose size={12} /></button></Show>
  </span>;
}
