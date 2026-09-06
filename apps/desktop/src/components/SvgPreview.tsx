import { createMemo, onCleanup } from "solid-js";

export interface SvgPreviewProps {
  content: string;
}

// Strips script elements and event-handler attributes from untrusted editor-buffer SVG before
// previewing it.
export function sanitizeSvgMarkup(markup: string): string {
  const withoutScripts = markup.replace(/<script[\s\S]*?<\/script\s*>/gi, "");
  return withoutScripts.replace(/\s(on[a-z]+)\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)/gi, "");
}

// Previews sanitized SVG through an image Blob URL, whose image context independently prevents
// script execution.
export default function SvgPreview(props: SvgPreviewProps) {
  let objectUrl: string | undefined;

  const url = createMemo(() => {
    if (objectUrl) URL.revokeObjectURL(objectUrl);
    const sanitized = sanitizeSvgMarkup(props.content);
    const blob = new Blob([sanitized], { type: "image/svg+xml" });
    objectUrl = URL.createObjectURL(blob);
    return objectUrl;
  });

  onCleanup(() => {
    if (objectUrl) URL.revokeObjectURL(objectUrl);
  });

  return (
    <div
      class="flex h-full w-full items-center justify-center overflow-auto bg-surface/30 p-6"
      data-testid="svg-preview"
      style={{
        "background-image": "radial-gradient(var(--line) 1px, transparent 1px)",
        "background-size": "14px 14px",
      }}
    >
      <img
        src={url()}
        alt="SVG preview"
        data-testid="svg-preview-image"
        class="max-h-full max-w-full object-contain"
      />
    </div>
  );
}
