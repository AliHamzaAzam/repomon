import { For } from "solid-js";
export type TranscriptDetail = "summary" | "normal" | "verbose";
export default function TranscriptDetailToggle(props: { value: TranscriptDetail; onChange: (value: TranscriptDetail) => void }) {
  return <div role="group" aria-label="Transcript detail" class="pointer-events-auto inline-flex shrink-0 rounded border border-line bg-raised p-0.5 font-sans text-xs normal-case tracking-normal">
    <For each={["summary", "normal", "verbose"] as const}>{(value) => <button type="button" aria-pressed={props.value === value}
      class={`focus-ring rounded px-2 py-1 capitalize transition-colors ${props.value === value ? "bg-surface text-foreground" : "text-muted hover:text-foreground"}`}
      onClick={() => props.onChange(value)}>{value}</button>}</For>
  </div>;
}
