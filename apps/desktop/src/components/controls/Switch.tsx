export default function Switch(props: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <div
      class="flex items-center justify-between rounded-lg border border-line bg-surface/60 px-3.5 py-2.5 text-xs transition-colors duration-[var(--motion-tap)] ease-[var(--ease-tap)] hover:bg-surface"
      classList={{ "opacity-50 pointer-events-none": props.disabled }}
    >
      <span class="font-medium text-foreground">{props.label}</span>
      <button
        type="button"
        role="switch"
        aria-checked={props.checked}
        aria-label={props.label}
        disabled={props.disabled}
        class="focus-ring switch-track"
        classList={{
          "bg-signal border-transparent": props.checked,
          "bg-raised border-line": !props.checked,
        }}
        onClick={() => props.onChange(!props.checked)}
      >
        {/* Size, seat and travel come from .switch-track's derived custom properties; the knob
            carries only its colour, which differs between the two states. */}
        <span class="switch-knob bg-white shadow-sm" classList={{ "bg-foreground/80": !props.checked }} />
      </button>
    </div>
  );
}
