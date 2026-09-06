import { Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";

import { detectActiveScope, type KeymapScope } from "../keymap";
import type { ActionsStore } from "../stores/actions";
import { IconClose, IconSearch } from "./icons";
import KeyboardHelp from "./KeyboardHelp";

export interface ShortcutsOverlayProps {
  actions: ActionsStore;
}

/// Renders the shortcut overlay in a portal with Escape and outside-click dismissal.
export default function ShortcutsOverlay(props: ShortcutsOverlayProps) {
  const isOpen = () => props.actions.shortcutsGuideOpen();
  const [activeScope, setActiveScope] = createSignal<KeymapScope>("global");
  let previouslyFocused: HTMLElement | null = null;
  let dialogRef: HTMLDivElement | undefined;

  function close(restoreFocus = true) {
    props.actions.closeShortcutsGuide();
    if (restoreFocus) queueMicrotask(() => (previouslyFocused?.isConnected ? previouslyFocused : null)?.focus());
  }

  // Snapshot what had focus (and therefore which scope is relevant) the instant the overlay
  // opens, before focus moves into the overlay's own search input.
  createEffect(() => {
    if (!isOpen()) return;
    previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setActiveScope(detectActiveScope(previouslyFocused));
    queueMicrotask(() => dialogRef?.querySelector<HTMLInputElement>("input")?.focus());
  });

  function onKeyDown(event: KeyboardEvent) {
    if (!isOpen()) return;
    if (event.key === "Tab" && dialogRef) {
      const controls = [...dialogRef.querySelectorAll<HTMLElement>(
        'button:not([disabled]), input:not([disabled]), a[href], [tabindex="0"]',
      )];
      const first = controls[0];
      const last = controls[controls.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus();
      }
    }
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close();
    }
  }

  onMount(() => {
    window.addEventListener("keydown", onKeyDown, true);
  });
  onCleanup(() => {
    window.removeEventListener("keydown", onKeyDown, true);
  });

  return (
    <Show when={isOpen()}>
      <Portal>
        <div
          class="fixed inset-0 z-[70] flex items-start justify-center pt-[10vh] bg-background/80 p-4 backdrop-blur-md"
          onClick={(event) => {
            if (event.target === event.currentTarget) close();
          }}
          role="presentation"
        >
          <div
            ref={dialogRef}
            class="focus-ring flex max-h-[75vh] w-full max-w-lg flex-col overflow-hidden rounded-2xl border border-line bg-surface shadow-2xl"
            role="dialog"
            aria-modal="true"
            aria-label="Keyboard shortcuts"
          >
            <div class="flex items-center justify-between gap-2 border-b border-line px-4 py-3">
              <div class="flex items-center gap-2 text-sm font-semibold text-foreground">
                <IconSearch size={14} class="text-muted" />
                Keyboard shortcuts
              </div>
              <button
                type="button"
                class="focus-ring rounded p-1 text-muted hover:text-foreground"
                onClick={() => close()}
                aria-label="Close"
              >
                <IconClose size={14} />
              </button>
            </div>
            <div class="min-h-0 flex-1 overflow-y-auto p-3">
              <KeyboardHelp variant="compact" activeScope={activeScope()} autofocus />
            </div>
            <div class="flex items-center justify-between gap-2 border-t border-line px-4 py-2.5 text-[11px] text-muted">
              <span>Esc to close</span>
              <button
                type="button"
                class="focus-ring text-signal hover:underline"
                onClick={() => {
                  close(false);
                  props.actions.openSettingsTab("keyboard");
                }}
              >
                Open full reference in Settings
              </button>
            </div>
          </div>
        </div>
      </Portal>
    </Show>
  );
}
