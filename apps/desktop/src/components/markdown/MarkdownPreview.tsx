import { createEffect, createMemo } from "solid-js";
import MarkdownRenderer from "./MarkdownRenderer";
import { parseMarkdown } from "./parser";

export interface MarkdownPreviewProps {
  content: string;
  filePath: string;
  laneId?: number;
  nearestHeading?: string | null;
}

export default function MarkdownPreview(props: MarkdownPreviewProps) {
  let containerRef: HTMLDivElement | undefined;

  const parsed = createMemo(() => parseMarkdown(props.content));

  createEffect(() => {
    const slug = props.nearestHeading;
    if (!containerRef) return;

    if (!slug) {
      containerRef.scrollTo({ top: 0, behavior: "smooth" });
      return;
    }

    try {
      const target = containerRef.querySelector(`#${CSS.escape(slug)}`);
      if (target && typeof (target as HTMLElement).scrollIntoView === "function") {
        (target as HTMLElement).scrollIntoView({ behavior: "smooth", block: "start" });
      }
    } catch {
      // Ignore querySelector escaping errors
    }
  });

  return (
    <div
      ref={containerRef}
      class="h-full w-full overflow-y-auto bg-surface/30 px-8 py-6 select-text"
      data-testid="markdown-preview"
    >
      <div class="mx-auto max-w-3xl pb-12">
        <MarkdownRenderer
          ast={parsed().ast}
          filePath={props.filePath}
          laneId={props.laneId}
        />
      </div>
    </div>
  );
}
