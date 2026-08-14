import { For } from "solid-js";

import { ACCENTS } from "../../theme";

export default function ColorField(props: {
  label: string;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <div class="block">
      <span class="section-label">{props.label}</span>
      <div class="mt-2 flex flex-wrap items-center gap-2">
        <For each={Object.entries(ACCENTS)}>
          {([name, color]) => (
            <button
              type="button"
              aria-label={name}
              aria-pressed={props.value === name}
              title={name}
              class="focus-ring relative h-6 w-6 rounded-full border transition-transform hover:scale-110"
              classList={{
                "border-foreground ring-2 ring-foreground/20 ring-offset-2 ring-offset-surface scale-105": props.value === name,
                "border-line/60": props.value !== name,
              }}
              style={{ background: color }}
              onClick={() => props.onChange(name)}
            />
          )}
        </For>
        <button
          type="button"
          aria-label="mono"
          aria-pressed={props.value === "mono"}
          title="mono"
          class="focus-ring h-6 w-6 rounded-full border bg-raised transition-transform hover:scale-110"
          classList={{
            "border-foreground ring-2 ring-foreground/20 ring-offset-2 ring-offset-surface scale-105": props.value === "mono",
            "border-line/60": props.value !== "mono",
          }}
          onClick={() => props.onChange("mono")}
        />
        <input
          class="focus-ring h-8 w-28 rounded-lg border border-line bg-surface px-2.5 font-mono text-xs text-foreground placeholder:text-muted/60"
          value={props.value}
          placeholder="#rrggbb"
          aria-label="Custom accent"
          onInput={(event) => props.onChange(event.currentTarget.value)}
        />
      </div>
    </div>
  );
}
