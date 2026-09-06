import { Show, onMount, type JSX } from "solid-js";

/// The furniture every section of the Repomind control room is built from: one heading, one
/// count, its controls, and its body.
///
/// Sections are separated by rules rather than boxed into cards. The panel is one column of
/// readings about one subject, and a stack of bordered cards would claim they are unrelated
/// tiles. It also keeps the panel legible at rail width, where a card's own padding is most of
/// the available line.

/// How long ago something happened, or "never" for something that has not. Coarse on purpose:
/// the question these lines answer is "is this keeping up?", never "what time exactly".
export function sinceLabel(iso: string | null | undefined, now = Date.now()): string {
  if (!iso) return "never";
  const at = Date.parse(iso);
  if (Number.isNaN(at)) return "never";
  const seconds = Math.max(0, Math.round((now - at) / 1000));
  if (seconds < 60) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

/// When something next happens, the mirror of [`sinceLabel`]. "due" covers a schedule the daemon
/// has not fired yet rather than claiming a negative wait.
export function untilLabel(iso: string | null | undefined, now = Date.now()): string {
  if (!iso) return "unscheduled";
  const at = Date.parse(iso);
  if (Number.isNaN(at)) return "unscheduled";
  const seconds = Math.round((at - now) / 1000);
  if (seconds <= 30) return "due";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `in ${Math.max(1, minutes)}m`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `in ${hours}h`;
  return `in ${Math.round(hours / 24)}d`;
}

export function Section(props: {
  title: string;
  detail?: string;
  action?: JSX.Element;
  children?: JSX.Element;
}) {
  return (
    <section class="border-b border-line/70 px-3.5 py-3 last:border-b-0" aria-label={props.title}>
      <div class="mb-2 flex items-baseline justify-between gap-2">
        <h2 class="section-label">{props.title}</h2>
        <div class="flex shrink-0 items-center gap-1.5">
          <Show when={props.detail}>
            <span class="font-mono text-[10px] text-muted/80">{props.detail}</span>
          </Show>
          {props.action}
        </div>
      </div>
      {props.children}
    </section>
  );
}

/// The quiet control a section uses for an action: readable, but never louder than the reading it
/// sits beside. `tone` raises exactly one of them when the action is destructive.
export function SectionButton(props: {
  label: string;
  busy?: boolean;
  disabled?: boolean;
  title?: string;
  tone?: "quiet" | "fault";
  onClick: () => void;
}) {
  const tone = () =>
    props.tone === "fault"
      ? "border-fault/30 text-fault hover:bg-fault/10"
      : "border-line text-muted hover:bg-raised hover:text-foreground";
  return (
    <button
      type="button"
      class={`focus-ring rounded border bg-raised/50 px-1.5 py-0.5 font-mono text-[10px] transition-colors disabled:opacity-40 ${tone()}`}
      disabled={props.busy || props.disabled}
      title={props.title}
      onClick={props.onClick}
    >
      {props.busy ? "Working" : props.label}
    </button>
  );
}

/// A section's own message: the empty state that says what the file behind it looks like, or the
/// failure that says why the reading is missing. Both answer "what do I do now", so they share a
/// shape rather than one being a paragraph and the other an alert box.
export function SectionNote(props: { children: JSX.Element; tone?: "muted" | "fault" }) {
  return (
    <p
      class={`text-xs leading-relaxed ${props.tone === "fault" ? "text-fault" : "text-muted"}`}
      role={props.tone === "fault" ? "alert" : undefined}
    >
      {props.children}
    </p>
  );
}

/// One row of a section's list: the hover target, the alignment, and the gap that make five
/// different lists read as one column.
export function RowShell(props: { children: JSX.Element; title?: string }) {
  return (
    <li
      class="group/row flex items-center gap-1.5 rounded-md px-1.5 py-1 transition-colors hover:bg-raised/50"
      title={props.title}
    >
      {props.children}
    </li>
  );
}

/// The single-line form a section reveals in place of a modal: one or two fields and a confirm.
/// Submitting on Enter and cancelling on Escape are the whole interaction, so the form never
/// takes the operator away from the readings it was started from.
export function InlineForm(props: {
  label: string;
  submitLabel: string;
  busy?: boolean;
  canSubmit: boolean;
  onSubmit: () => void;
  onCancel: () => void;
  children: JSX.Element;
}) {
  const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const cancel = () => {
    props.onCancel();
    if (trigger?.isConnected) trigger.focus();
  };

  return (
    <form
      class="mb-2 space-y-1.5 rounded-lg border border-line bg-raised/40 p-2"
      aria-label={props.label}
      onSubmit={(event) => {
        event.preventDefault();
        if (props.canSubmit) props.onSubmit();
      }}
      onKeyDown={(event) => {
        if (event.key !== "Escape") return;
        event.preventDefault();
        event.stopPropagation();
        cancel();
      }}
    >
      {props.children}
      <div class="flex items-center justify-end gap-1.5">
        <button
          type="button"
          class="focus-ring rounded px-1.5 py-0.5 font-mono text-[10px] text-muted hover:text-foreground"
          onClick={cancel}
        >
          Cancel
        </button>
        <button
          type="submit"
          class="focus-ring rounded border border-signal/40 bg-signal/10 px-2 py-0.5 font-mono text-[10px] font-semibold text-signal transition-colors hover:bg-signal/20 disabled:opacity-40"
          disabled={!props.canSubmit || props.busy}
        >
          {props.busy ? "Working" : props.submitLabel}
        </button>
      </div>
    </form>
  );
}

/// The text input those forms are made of. One class string in one place, so a second field never
/// arrives half a pixel taller than the first.
export function InlineField(props: {
  label: string;
  placeholder?: string;
  value: string;
  autofocus?: boolean;
  onInput: (value: string) => void;
}) {
  let inputRef!: HTMLInputElement;
  onMount(() => {
    if (props.autofocus) inputRef.focus();
  });

  return (
    <input
      ref={inputRef}
      type="text"
      aria-label={props.label}
      placeholder={props.placeholder}
      class="focus-ring w-full rounded border border-line bg-background px-2 py-1 text-xs text-foreground outline-none placeholder:text-muted/60"
      value={props.value}
      onInput={(event) => props.onInput(event.currentTarget.value)}
    />
  );
}
