interface BrandMarkProps {
  /// Rendered size in pixels. The glyph is a 1254-unit grid with ~95-unit bars, so below about
  /// 20px the bars fall under 1.5 device pixels and turn to mush. Keep it 24 or larger, or pass
  /// `tight`, which crops the icon padding and buys the bar weight back at a smaller box.
  size?: number;
  /// Crop the viewBox to the glyph's own bounding box instead of the icon's padded 1254 canvas.
  ///
  /// The full canvas is what the app icon needs: the mark occupies the middle ~52% of it and the
  /// rest is the margin a rounded app tile wants around its artwork. In a 35px title bar that
  /// margin is dead weight, so a `size={24}` mark draws only 12.5px of ink and reads soft and
  /// undersized. Cropping to the ink (x 300..955, y 280..968, squared about the glyph's own
  /// centre) makes the requested size the size of the *mark*: at 16px a bar lands on 2.2 CSS
  /// pixels instead of 1.2, which is crisp on a 1x display and sharp on a retina one.
  tight?: boolean;
  title?: string;
  class?: string;
}

/// The glyph's bounding box, squared on its taller axis and centred on the ink. The ink spans
/// 655 x 688 units, so the 688-wide box starts 16.5 units left of it.
const TIGHT_VIEW_BOX = "283.5 280 688 688";

/// The Repomon mark, the same geometry as the app icon
/// (`design/repomon-logo/command-mesh-faithful/command-mesh-master.svg`), flattened from that
/// file's matrix-stretched rects into plain ones. The flattened form was checked pixel-identical
/// to the original at 1024px before it landed here.
///
/// It is drawn from theme tokens rather than the source file's fixed palette, so it follows the
/// selected theme (and the accent) instead of staying locked to the dark-mode colors. The mapping
/// is exact: the source's teal is `--signal`, its amber pip is `--attention`. Unlike the app icon,
/// this mark renders on a transparent background so it sits directly on the title bar.
export default function BrandMark(props: BrandMarkProps) {
  const size = () => props.size ?? 26;
  return (
    <svg
      width={size()}
      height={size()}
      viewBox={props.tight ? TIGHT_VIEW_BOX : "0 0 1254 1254"}
      class={props.class}
      role={props.title ? "img" : "presentation"}
      aria-label={props.title}
      aria-hidden={props.title ? undefined : "true"}
    >
      <g fill="var(--signal)">
        <rect x="300" y="280" width="304" height="97" />
        <rect x="650" y="280" width="305" height="97" />
        <rect x="300" y="280" width="95" height="393" />
        <rect x="650" y="280" width="94" height="253" />
        <rect x="860" y="280" width="95" height="688" />
        <rect x="458" y="437" width="286" height="96" />
        <rect x="300" y="578" width="217" height="95" />
        <rect x="738" y="578" width="217" height="95" />
        <rect x="300" y="722" width="94" height="246" />
        <rect x="428" y="578" width="89" height="240" />
        <rect x="571" y="722" width="234" height="96" />
        <rect x="571" y="722" width="94" height="246" />
        <rect x="300" y="873" width="365" height="95" />
        <rect x="721" y="873" width="234" height="95" />
      </g>
      <rect x="572" y="571" width="110" height="110" fill="var(--attention)" />
    </svg>
  );
}
