export default function Switch(props: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <div
      class="flex items-center justify-between rounded-lg border border-line bg-surface/60 px-3.5 py-2.5 text-xs transition-colors hover:bg-surface"
      classList={{ "opacity-50 pointer-events-none": props.disabled }}
    >
      <span class="font-medium text-foreground">{props.label}</span>
      <button
        type="button"
        role="switch"
        aria-checked={props.checked}
        aria-label={props.label}
        disabled={props.disabled}
        class="focus-ring relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full border border-transparent transition-colors duration-150 ease-in-out"
        classList={{
          "bg-signal": props.checked,
          "bg-raised border-line": !props.checked,
        }}
        onClick={() => props.onChange(!props.checked)}
      >
        <span
          class="pointer-events-none inline-block h-3.5 w-3.5 rounded-full bg-white shadow-sm ring-0 transition-transform duration-150 ease-in-out"
          classList={{
            "translate-x-4": props.checked,
            "translate-x-0.5 bg-foreground/80": !props.checked,
          }}
        />
      </button>
    </div>
  );
}
