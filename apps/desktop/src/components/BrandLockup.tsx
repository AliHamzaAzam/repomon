import { Dynamic } from "solid-js/web";

import BrandMark from "./BrandMark";

export interface BrandLockupProps {
  /// Draw only the mark and keep the wordmark for assistive technology. The macOS title bar sits
  /// under a menu bar that already prints the app name, so the wordmark there is redundant.
  markOnly?: boolean;
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

/// Renders the shared mark and wordmark, using CSS capitalization so assistive technology reads the
/// name rather than an initialism.
export default function BrandLockup(props: BrandLockupProps) {
  const size = () => props.size ?? 16;

  const content = () => (
    <>
      <BrandMark size={size()} tight class="shrink-0" />
      <Dynamic
        component={props.heading ? "h1" : "span"}
        class={`font-mono text-[11px] font-semibold uppercase leading-none tracking-[0.09em] text-foreground ${
          props.markOnly ? "sr-only" : ""
        }`}
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
