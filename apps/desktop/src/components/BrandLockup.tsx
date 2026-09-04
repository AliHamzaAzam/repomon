import { Dynamic } from "solid-js/web";

import BrandMark from "./BrandMark";

export interface BrandLockupProps {
  /// Render the wordmark as the page's `h1` rather than a plain span. The mission control shell
  /// passes this; the setup wizard does not, so the two never fight over the document outline.
  heading?: boolean;
  /// What activating the lockup does. Without it the lockup is inert text rather than a button
  /// that looks pressable and answers to nothing.
  onActivate?: () => void;
  /// Accessible name for the interactive form. Ignored when `onActivate` is absent.
  activateLabel?: string;
  /// Mark size in pixels, cropped to the glyph itself (see `BrandMark`'s `tight`).
  size?: number;
  class?: string;
}

/// The Repomon lockup: the command-mesh mark plus the wordmark, set as one unit.
///
/// Both full-window headers draw this, so the app is identified the same way whether the mission
/// control shell or the setup wizard owns the window. Keeping it in one component is also what
/// keeps the two in step when the mark or its metrics change.
///
/// Proportions: the mark is cropped to its ink (16px of glyph, not 16px of icon canvas) and the
/// wordmark is set at the section-label's tracking, so the pair reads as lettering cut from the
/// same grid as the mark's bars rather than as an icon with a caption. `leading-none` on the
/// wordmark makes its box the cap height, which is what lets plain vertical centring land the
/// mark optically level with the letters instead of floating above them.
///
/// The DOM text stays "Repomon" and the capitals come from CSS, so assistive technology reads a
/// name rather than an initialism.
export default function BrandLockup(props: BrandLockupProps) {
  const size = () => props.size ?? 16;

  const content = () => (
    <>
      <BrandMark size={size()} tight class="shrink-0" />
      <Dynamic
        component={props.heading ? "h1" : "span"}
        class="font-mono text-[11px] font-semibold uppercase leading-none tracking-[0.09em] text-foreground"
      >
        Repomon
      </Dynamic>
    </>
  );

  return (
    <Dynamic
      component={props.onActivate ? "button" : "div"}
      type={props.onActivate ? "button" : undefined}
      onClick={props.onActivate}
      aria-label={props.onActivate ? (props.activateLabel ?? "About Repomon") : undefined}
      title={props.onActivate ? (props.activateLabel ?? "About Repomon") : undefined}
      data-brand-lockup
      class={`-mx-1.5 flex items-center gap-2 rounded px-1.5 py-1 ${
        props.onActivate ? "focus-ring cursor-pointer transition-colors hover:bg-line/40" : ""
      } ${props.class ?? ""}`}
    >
      {content()}
    </Dynamic>
  );
}
