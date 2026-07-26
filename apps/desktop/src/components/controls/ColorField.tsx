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
      <div class="mt-1.5 flex flex-wrap items-center gap-1.5">
        <For each={Object.entries(ACCENTS)}>
          {([name, color]) => (
            <button
              type="button"
              aria-label={name}
              aria-pressed={props.value === name}
              title={name}
              class="focus-ring h-5 w-5 rounded-full border"
              classList={{ "border-foreground": props.value === name, "border-line": props.value !== name }}
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
          class="focus-ring h-5 w-5 rounded-full border bg-raised"
          classList={{ "border-foreground": props.value === "mono", "border-line": props.value !== "mono" }}
          onClick={() => props.onChange("mono")}
        />
        <input
          class="settings-input mt-0 ml-1 w-28"
          value={props.value}
          placeholder="#rrggbb"
          aria-label="Custom accent"
          onInput={(event) => props.onChange(event.currentTarget.value)}
        />
      </div>
    </div>
  );
}
