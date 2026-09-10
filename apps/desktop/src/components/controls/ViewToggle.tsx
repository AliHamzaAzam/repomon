import { For } from "solid-js";
import type { AgentView } from "../../stores/agentViews";
export default function ViewToggle(props: { value: AgentView; onChange: (view: AgentView) => void; disabled?: boolean; label?: string }) {
  return <div role="group" aria-label={props.label ?? "Agent view"} class="pointer-events-auto inline-flex shrink-0 rounded border border-line bg-raised p-0.5 font-sans text-xs normal-case tracking-normal">
    <For each={["terminal", "conversation"] as const}>{(value) => <button type="button" disabled={props.disabled} aria-pressed={props.value === value}
      class={`focus-ring rounded px-2.5 py-1 transition-colors disabled:cursor-not-allowed disabled:opacity-50 ${props.value === value ? "bg-surface text-foreground" : "text-muted hover:text-foreground"}`}
      onClick={() => props.onChange(value)}>{value === "terminal" ? "Terminal" : "Chat"}</button>}</For>
  </div>;
}
