import { Show, onCleanup, onMount } from "solid-js";

import type { Lane } from "../bindings";
import { IconPin, IconTrash } from "./icons";

/// Offers lane pinning and worktree removal through the shared confirmation flow.
export default function LaneRowMenu(props: {
  lane: Lane;
  x: number;
  y: number;
  onPin: () => void;
  onRemoveWorktree: () => void;
  onClose: () => void;
}) {
  function onKey(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    event.stopPropagation();
    props.onClose();
  }
  onMount(() => window.addEventListener("keydown", onKey, true));
  onCleanup(() => window.removeEventListener("keydown", onKey, true));

  const top = () => Math.max(8, Math.min(props.y, window.innerHeight - 120));
  const item =
    "focus-ring flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs font-medium transition-colors hover:bg-raised";

  return (
    <>
      <div class="fixed inset-0 z-40" onClick={() => props.onClose()} />
      <div
        class="fixed z-50 w-56 rounded-xl border border-line bg-surface p-1.5 shadow-[0_12px_40px_var(--shadow)]"
        style={{ left: `${props.x}px`, top: `${top()}px` }}
        role="menu"
      >
        <button
          type="button"
          class={`${item} text-foreground`}
          onClick={() => {
            props.onPin();
            props.onClose();
          }}
          role="menuitem"
        >
          <IconPin size={13} />
          <span>{props.lane.pinned ? "Unpin lane" : "Pin lane to top"}</span>
        </button>
        <Show when={!props.lane.worktree.is_main}>
          <button
            type="button"
            class={`${item} text-fault`}
            onClick={() => {
              props.onRemoveWorktree();
              props.onClose();
            }}
            role="menuitem"
          >
            <IconTrash size={13} />
            <span>Remove worktree</span>
          </button>
          <p class="px-2.5 pb-1 pt-0.5 text-[10px] leading-snug text-muted">
            <Show when={props.lane.state.merged} fallback="The branch is kept.">
              This branch is already in the default branch. The branch is kept.
            </Show>
          </p>
        </Show>
      </div>
    </>
  );
}
