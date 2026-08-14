import { For } from "solid-js";
import { IconChevronDown } from "../icons";

export default function Select(props: {
  label: string;
  value: string;
  options: Array<{ value: string; label: string }>;
  onChange: (value: string) => void;
}) {
  return (
    <label class="block">
      <span class="section-label">{props.label}</span>
      <div class="relative mt-1.5">
        <select
          class="focus-ring h-8 w-full appearance-none rounded-lg border border-line bg-surface px-3 pr-8 text-xs font-medium text-foreground transition-colors hover:border-muted/50 focus:border-signal"
          value={props.value}
          onChange={(event) => props.onChange(event.currentTarget.value)}
        >
          <For each={props.options}>
            {(option) => <option value={option.value}>{option.label}</option>}
          </For>
        </select>
        <span class="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 text-muted">
          <IconChevronDown size={12} strokeWidth={2} />
        </span>
      </div>
    </label>
  );
}
