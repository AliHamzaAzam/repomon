import { For } from "solid-js";

export default function Select(props: {
  label: string;
  value: string;
  options: Array<{ value: string; label: string }>;
  onChange: (value: string) => void;
}) {
  return (
    <label class="block">
      <span class="section-label">{props.label}</span>
      <select class="settings-input" value={props.value} onChange={(event) => props.onChange(event.currentTarget.value)}>
        <For each={props.options}>{(option) => <option value={option.value}>{option.label}</option>}</For>
      </select>
    </label>
  );
}
