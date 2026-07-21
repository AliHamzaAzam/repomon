import { Show, onCleanup, onMount, type JSX } from "solid-js";

interface ModalProps {
  title: string;
  subtitle?: string;
  onClose: () => void;
  children: JSX.Element;
  footer?: JSX.Element;
  width?: string;
}

/// Shared modal shell: centered card, backdrop-dismiss, Escape-to-close. Replaces the browser
/// prompt()/confirm() popups the app used to lean on, so every input flow renders in-app.
export default function Modal(props: ModalProps) {
  const onKey = (event: KeyboardEvent) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      props.onClose();
    }
  };
  onMount(() => window.addEventListener("keydown", onKey, true));
  onCleanup(() => window.removeEventListener("keydown", onKey, true));

  return (
    <div
      class="fixed inset-0 z-[60] flex items-center justify-center bg-background/70 p-6 backdrop-blur-sm"
      onPointerDown={(event) => {
        if (event.target === event.currentTarget) props.onClose();
      }}
    >
      <section
        role="dialog"
        aria-modal="true"
        aria-label={props.title}
        class="flex max-h-[88vh] flex-col overflow-hidden rounded-xl border border-line bg-surface shadow-[0_28px_90px_var(--shadow)]"
        style={{ width: props.width ?? "min(34rem, 94vw)" }}
      >
        <header class="flex items-start justify-between border-b border-line px-5 py-4">
          <div>
            <p class="section-label">{props.title}</p>
            <Show when={props.subtitle}>
              <p class="mt-1 text-sm text-muted">{props.subtitle}</p>
            </Show>
          </div>
          <button
            type="button"
            class="focus-ring rounded border border-line px-2 py-1 text-xs text-muted hover:text-foreground"
            onClick={props.onClose}
          >
            Esc
          </button>
        </header>
        <div class="min-h-0 flex-1 overflow-y-auto px-5 py-4">{props.children}</div>
        <Show when={props.footer}>
          <footer class="flex items-center justify-end gap-2 border-t border-line px-5 py-3">
            {props.footer}
          </footer>
        </Show>
      </section>
    </div>
  );
}
