import { For } from "solid-js";
import type { AgentView } from "../../stores/agentViews";

const OPTIONS: readonly { value: AgentView; label: string }[] = [
  { value: "terminal", label: "Terminal" },
  { value: "conversation", label: "Chat" },
];

/// The switch the operator hits more than any other control, so it is the one that got the liquid
/// treatment. The travel variable drives both blobs behind the labels; the labels themselves take
/// the colour at tap speed, so the click is acknowledged well before the pill finishes arriving.
export default function ViewToggle(props: { value: AgentView; onChange: (view: AgentView) => void; disabled?: boolean; label?: string }) {
  return <div role="group" aria-label={props.label ?? "Agent view"} class="view-toggle pointer-events-auto"
    style={{ "--view-toggle-travel": props.value === "conversation" ? "100%" : "0%" }}>
    <span class="view-toggle__liquid" aria-hidden="true">
      <span class="view-toggle__blob is-follower" />
      <span class="view-toggle__blob" />
    </span>
    <For each={OPTIONS}>{(option) => <button type="button" class="view-toggle__option focus-ring" disabled={props.disabled}
      aria-pressed={props.value === option.value} onClick={() => props.onChange(option.value)}>{option.label}</button>}</For>
  </div>;
}
