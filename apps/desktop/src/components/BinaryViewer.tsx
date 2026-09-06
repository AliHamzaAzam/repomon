import { formatBytes } from "../formatBytes";
import { Show } from "solid-js";

export interface BinaryViewerProps {
  path: string;
  size?: number;
}


function basename(path: string): string {
  return path.split("/").pop() || path;
}

export default function BinaryViewer(props: BinaryViewerProps) {
  return (
    <div class="flex h-full flex-col items-center justify-center bg-surface p-6 text-center select-none">
      <div class="max-w-sm space-y-3 rounded-xl border border-line bg-surface/40 p-5">
        <div class="mx-auto flex size-10 items-center justify-center rounded-lg border border-line bg-raised/50 text-muted">
          <svg
            width="20"
            height="20"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.75"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
            <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
            <polyline points="14 2 14 8 20 8" />
            <line x1="16" y1="13" x2="8" y2="13" />
            <line x1="16" y1="17" x2="8" y2="17" />
            <polyline points="10 9 9 9 8 9" />
          </svg>
        </div>
        <div class="space-y-1">
          <p class="font-mono text-xs font-semibold text-foreground">{basename(props.path)}</p>
          <Show when={props.size !== undefined && props.size > 0}>
            <p class="font-mono text-[11px] text-muted">{formatBytes(props.size)}</p>
          </Show>
        </div>
        <div class="border-t border-line/60 pt-2.5">
          <p class="text-xs font-medium text-foreground/80">Binary file</p>
          <p class="mt-0.5 text-xs text-muted">
            This file contains binary data and cannot be displayed in the text editor.
          </p>
        </div>
      </div>
    </div>
  );
}
