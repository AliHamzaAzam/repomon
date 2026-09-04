import { createMemo, onCleanup } from "solid-js";

export interface SvgPreviewProps {
  content: string;
}

// Strips <script> elements and any on*-style event-handler attribute from raw SVG markup before
// it ever reaches the DOM. The markup comes straight from the unsaved editor buffer (never from
// disk - see SvgPreview's caller in EditorWorkspace), so it is untrusted input even though it is
// the user's own file: a pasted or half-edited SVG can carry an inline script or an onload/
// onerror handler that must never execute just because the tab happens to be previewed.
export function sanitizeSvgMarkup(markup: string): string {
  const withoutScripts = markup.replace(/<script[\s\S]*?<\/script\s*>/gi, "");
  return withoutScripts.replace(/\s(on[a-z]+)\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)/gi, "");
}

// Renders the live SVG buffer beside the editor, in the same split panel MarkdownPreview
// occupies for .md tabs. The sanitized markup is handed to the <img> as a Blob URL rather than
// injected as inline HTML - the browser's image context does not execute script or event
// handlers at all, so this is a second, independent layer under the sanitizer above, not a
// replacement for it.
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
