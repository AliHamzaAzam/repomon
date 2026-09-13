import { Show } from "solid-js";

/// Where a quota window sits. `unknown` is a window whose percentage the probe could not read:
/// it must never render as zero, because an unread limit and an unused one are opposite facts.
export type MeterLevel = "steady" | "tight" | "at-limit" | "unknown";

/// Both thresholds are the ones the sidebar's numbers have always used, so the bar's colour and
/// the number's weight cannot disagree about the same window.
const TIGHT_PCT = 75;
const AT_LIMIT_PCT = 95;

export function meterLevel(pct: number | null | undefined): MeterLevel {
  if (typeof pct !== "number" || !Number.isFinite(pct)) return "unknown";
  if (pct >= AT_LIMIT_PCT) return "at-limit";
  if (pct >= TIGHT_PCT) return "tight";
  return "steady";
}

/// The word that carries the same warning the colour carries, for anyone who cannot separate the
/// hues. Steady windows stay wordless so the words themselves mean something.
export function meterStateWord(level: MeterLevel): string | null {
  if (level === "at-limit") return "at limit";
  if (level === "tight") return "tight";
  return null;
}

const TONE: Record<MeterLevel, string> = {
  steady: "text-foreground",
  tight: "text-attention font-semibold",
  "at-limit": "text-fault font-semibold",
  unknown: "text-muted/70",
};

const FILL: Record<MeterLevel, string> = {
  steady: "bg-signal",
  tight: "bg-attention",
  "at-limit": "bg-fault",
  unknown: "",
};

/// A read percentage renders a solid track so an empty bar still looks like a bar; an unread one
/// renders a hatched track, so "nothing came back" cannot be mistaken for "nothing used".
const HATCH = "repeating-linear-gradient(135deg, var(--line) 0 2px, transparent 2px 5px)";

/// One labelled quota bar: the number for precision, the length for a glance, and a state word so
/// the warning survives without colour. This is the desktop's only meter; copy it rather than
/// styling a fresh div with a width.
export default function UsageMeter(props: {
  /// Visible label, and the progressbar's accessible name.
  label: string;
  /// Percent consumed (0-100). Null, undefined or a non-finite value means the probe read nothing.
  pct: number | null | undefined;
  title?: string;
}) {
  const level = () => meterLevel(props.pct);
  const known = () => level() !== "unknown";
  const value = () => Math.max(0, Math.min(100, Math.round(props.pct as number)));
  const word = () => meterStateWord(level());
  const valueText = () => (known() ? `${value()}%` : "no data");
  const speech = () => {
    if (!known()) return `${props.label}: no data`;
    const suffix = word();
    return suffix ? `${value()}% used, ${suffix}` : `${value()}% used`;
  };

  return (
    <div class="py-0.5" title={props.title}>
      <div class="flex items-baseline justify-between gap-2 font-mono text-[10px] text-muted">
        <span class="min-w-0 truncate text-muted/80">{props.label}</span>
        <span class="flex shrink-0 items-baseline gap-1">
          <Show when={word()}>{(state) => <span class={`${TONE[level()]} font-normal`}>{state()}</span>}</Show>
          <span class={`tabular-nums ${TONE[level()]}`}>{valueText()}</span>
        </span>
      </div>
      <div
        role="progressbar"
        aria-label={props.label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={known() ? value() : undefined}
        aria-valuetext={speech()}
        class={`mt-1 h-1 w-full overflow-hidden rounded-full ${known() ? "bg-line" : ""}`}
        style={known() ? undefined : { background: HATCH }}
      >
        <Show when={known() && value() > 0}>
          <div class={`h-full rounded-full ${FILL[level()]}`} style={{ width: `${value()}%` }} />
        </Show>
      </div>
    </div>
  );
}
