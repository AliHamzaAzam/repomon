import { For, Show, createEffect, createSignal, createUniqueId, onCleanup, onMount, type JSX } from "solid-js";
import { IconCheck, IconChevronDown } from "../icons";

export interface SelectOption {
  value: string;
  label: string;
  icon?: JSX.Element;
}

export interface SelectProps {
  label?: string;
  ariaLabel?: string;
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  size?: "sm" | "md";
  disabled?: boolean;
  class?: string;
}

/// Fully custom-rendered, OS-independent floating dropdown component.
export default function Select(props: SelectProps) {
  const [open, setOpen] = createSignal(false);
  const [highlightedIndex, setHighlightedIndex] = createSignal(-1);
  let containerRef!: HTMLDivElement;
  let buttonRef!: HTMLButtonElement;
  let listboxRef!: HTMLUListElement;
  const labelId = createUniqueId();
  const listboxId = createUniqueId();

  const selectedOption = () => props.options.find((opt) => opt.value === props.value);
  const selectedLabel = () => selectedOption()?.label ?? props.value ?? "";

  const size = () => props.size ?? "md";

  function close(restoreFocus = true) {
    setOpen(false);
    setHighlightedIndex(-1);
    if (restoreFocus) buttonRef?.focus();
  }

  function choose(option: SelectOption) {
    props.onChange(option.value);
    close(true);
  }

  const onPointerDownOutside = (event: PointerEvent) => {
    if (containerRef && !containerRef.contains(event.target as Node)) {
      close(false);
    }
  };

  onMount(() => {
    window.addEventListener("pointerdown", onPointerDownOutside);
  });

  onCleanup(() => {
    window.removeEventListener("pointerdown", onPointerDownOutside);
  });

  const onKeyDown = (event: KeyboardEvent) => {
    if (props.disabled) return;

    if (!open()) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp" || event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        setOpen(true);
        const idx = props.options.findIndex((opt) => opt.value === props.value);
        setHighlightedIndex(idx >= 0 ? idx : 0);
      }
      return;
    }

    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close(true);
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      setHighlightedIndex((curr) => (curr + 1 < props.options.length ? curr + 1 : 0));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setHighlightedIndex((curr) => (curr - 1 >= 0 ? curr - 1 : props.options.length - 1));
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      const curr = highlightedIndex();
      if (curr >= 0 && curr < props.options.length) {
        choose(props.options[curr]);
      }
    } else if (event.key === "Tab") {
      close(false);
    }
  };

  createEffect(() => {
    if (open()) {
      const idx = props.options.findIndex((opt) => opt.value === props.value);
      setHighlightedIndex(idx >= 0 ? idx : 0);
    }
  });

  return (
    <div ref={containerRef} class={`relative ${props.label ? "block" : "inline-block"} ${props.class ?? ""}`}>
      <Show when={props.label}>
        <span id={labelId} class="section-label mb-1.5 block">
          {props.label}
        </span>
      </Show>
      <button
        ref={buttonRef}
        type="button"
        role="combobox"
        aria-haspopup="listbox"
        aria-expanded={open()}
        aria-labelledby={props.label ? labelId : undefined}
        aria-label={props.label ? undefined : (props.ariaLabel ?? "Select option")}
        aria-controls={open() ? listboxId : undefined}
        disabled={props.disabled}
        onClick={() => {
          if (props.disabled) return;
          setOpen(!open());
        }}
        onKeyDown={onKeyDown}
        class={`focus-ring flex w-full items-center justify-between gap-2 rounded-lg border border-line bg-surface text-foreground transition-colors hover:border-muted/50 focus:border-signal disabled:cursor-not-allowed disabled:opacity-50 ${
          size() === "sm"
            ? "h-7 px-2.5 font-mono text-[10px] uppercase tracking-wider text-muted hover:text-foreground"
            : "h-8.5 px-3 text-xs font-medium"
        }`}
      >
        <span class="flex min-w-0 items-center gap-2 truncate">
          <Show when={selectedOption()?.icon}>
            <span class="shrink-0">{selectedOption()!.icon}</span>
          </Show>
          <span class="truncate">{selectedLabel()}</span>
        </span>
        <span class={`shrink-0 text-muted transition-transform duration-150 ${open() ? "rotate-180 text-foreground" : ""}`}>
          <IconChevronDown size={size() === "sm" ? 10 : 12} strokeWidth={2} />
        </span>
      </button>

      <Show when={open()}>
        <ul
          ref={listboxRef}
          id={listboxId}
          role="listbox"
          aria-labelledby={props.label ? labelId : undefined}
          aria-label={props.label ? undefined : (props.ariaLabel ?? "Options")}
          tabIndex={-1}
          class="absolute left-0 right-0 z-50 mt-1 max-h-60 min-w-[8rem] overflow-y-auto rounded-xl border border-line bg-surface p-1 shadow-[0_12px_36px_var(--shadow)] outline-none backdrop-blur-md"
        >
          <For each={props.options}>
            {(option, index) => {
              const isSelected = () => option.value === props.value;
              const isHighlighted = () => highlightedIndex() === index();
              return (
                <li
                  role="option"
                  aria-selected={isSelected()}
                  class={`flex cursor-pointer select-none items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 transition-colors ${
                    size() === "sm" ? "font-mono text-[10px] uppercase tracking-wider" : "text-xs font-medium"
                  } ${
                    isHighlighted()
                      ? "bg-raised text-foreground shadow-xs"
                      : isSelected()
                        ? "text-signal font-semibold"
                        : "text-muted hover:bg-raised/60 hover:text-foreground"
                  }`}
                  onPointerMove={() => setHighlightedIndex(index())}
                  onClick={() => choose(option)}
                >
                  <span class="flex min-w-0 items-center gap-2 truncate">
                    <Show when={option.icon}>
                      <span class="shrink-0">{option.icon}</span>
                    </Show>
                    <span class="truncate">{option.label}</span>
                  </span>
                  <Show when={isSelected()}>
                    <span class="shrink-0 text-signal">
                      <IconCheck size={13} strokeWidth={2.5} />
                    </span>
                  </Show>
                </li>
              );
            }}
          </For>
        </ul>
      </Show>
    </div>
  );
}
