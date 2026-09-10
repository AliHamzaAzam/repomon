import { attachmentFromPath, type ChatAttachment } from "./controls/AttachmentChip";

export type AttachmentTextPart = { text: string } | { attachment: ChatAttachment };
// Decode only the exact standalone delivery lines written by our composer. Fenced
// examples and malformed lines stay text; no transcript content is stripped here.
export function attachmentTextParts(text: string): AttachmentTextPart[] {
  const parts: AttachmentTextPart[] = [];
  let prose: string[] = [];
  let fence = "";
  const flush = () => { if (prose.length) parts.push({ text: prose.join("\n") }); prose = []; };
  for (const line of text.split("\n")) {
    const marker = line.trimStart().match(/^(`{3,}|~{3,})/);
    if (marker) {
      if (!fence) fence = marker[1];
      else if (marker[1][0] === fence[0] && marker[1].length >= fence.length) fence = "";
      prose.push(line); continue;
    }
    if (!fence && line.startsWith("Attached file: ")) {
      try {
        const path: unknown = JSON.parse(line.slice("Attached file: ".length));
        if (typeof path === "string" && /^(\/|~\/|[A-Za-z]:[\\/]|\\\\)/.test(path) && !/[\r\n\0]/.test(path)) {
          flush(); parts.push({ attachment: attachmentFromPath(path) }); continue;
        }
      } catch { /* Preserve incomplete streaming and malformed delivery lines. */ }
    }
    prose.push(line);
  }
  flush();
  return parts;
}
