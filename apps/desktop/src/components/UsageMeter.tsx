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

/// A steady window fills its track in ink, not in colour. Colour on this row means a warning, so
/// four quiet quotas leave the sidebar with no hue in it at all and a tight one is seen at once.
const FILL: Record<MeterLevel, string> = {
  steady: "bg-muted/50",
  tight: "bg-attention",
  "at-limit": "bg-fault",
  unknown: "",
};

/// A read percentage renders a solid track so an empty bar still looks like a bar; an unread one
/// renders a hatched track, so "nothing came back" cannot be mistaken for "nothing used".
const HATCH = "repeating-linear-gradient(135deg, var(--line) 0 2px, transparent 2px 5px)";

/// One quota on one line: the label, the state word where the warning belongs, a short fixed
/// track, and the number in its own right-hand column. The track and the number are fixed widths
/// so four of these stack into two clean columns instead of a ragged block. This is the desktop's
/// only meter; copy it rather than styling a fresh div with a width.
export default function UsageMeter(props: {
  /// Visible label. Short enough to survive the narrow sidebar beside a state word.
  label: string;
  /// The progressbar's accessible name, where the row has more to say than it can draw. Defaults
  /// to the visible label.
  name?: string;
  /// Percent consumed (0-100). Null, undefined or a non-finite value means the probe read nothing.
  pct: number | null | undefined;
  title?: string;
}) {
  const name = () => props.name ?? props.label;
  const level = () => meterLevel(props.pct);
  const known = () => level() !== "unknown";
  const value = () => Math.max(0, Math.min(100, Math.round(props.pct as number)));
  const word = () => meterStateWord(level());
  const valueText = () => (known() ? `${value()}%` : "no data");
  const speech = () => {
    if (!known()) return `${name()}: no data`;
    const suffix = word();
    return suffix ? `${value()}% used, ${suffix}` : `${value()}% used`;
  };

  return (
    <div class="flex items-center gap-2 py-0.5 font-mono text-[10px]" title={props.title}>
      <span class="flex min-w-0 flex-1 items-baseline gap-1.5">
        <span class="min-w-0 truncate text-muted/80">{props.label}</span>
        <Show when={word()}>{(state) => <span class={`shrink-0 ${TONE[level()]} font-normal`}>{state()}</span>}</Show>
      </span>
      <div
        role="progressbar"
        aria-label={name()}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={known() ? value() : undefined}
        aria-valuetext={speech()}
        class={`h-1 w-12 shrink-0 overflow-hidden rounded-full ${known() ? "bg-line" : ""}`}
        style={known() ? undefined : { background: HATCH }}
      >
        {/* A one-percent window still puts something on the track, so "barely used" and "unused"
            stay different at this length. */}
        <Show when={known() && value() > 0}>
          <div class={`h-full rounded-full ${FILL[level()]}`} style={{ width: `${value()}%`, "min-width": "3px" }} />
        </Show>
      </div>
      <span class={`w-[3.1rem] shrink-0 text-right tabular-nums ${TONE[level()]}`}>{valueText()}</span>
    </div>
  );
}
