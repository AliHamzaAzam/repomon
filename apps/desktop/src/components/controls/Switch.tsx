export default function Switch(props: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <div class="flex items-center justify-between rounded border border-line px-3 py-2 text-xs" classList={{ "opacity-50": props.disabled }}>
      <span>{props.label}</span>
      <button
        type="button"
        role="switch"
        aria-checked={props.checked}
        aria-label={props.label}
        disabled={props.disabled}
        class="focus-ring relative h-4 w-8 shrink-0 rounded-full border border-line transition-colors"
        classList={{ "bg-signal/70": props.checked, "bg-raised": !props.checked }}
        onClick={() => props.onChange(!props.checked)}
      >
        <span
          class="absolute top-0.5 h-2.5 w-2.5 rounded-full bg-foreground transition-all"
          classList={{ "left-4": props.checked, "left-0.5": !props.checked }}
        />
      </button>
    </div>
  );
}
