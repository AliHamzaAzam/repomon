interface BrandMarkProps {
  /// Rendered size in pixels. The selected mark uses 22-unit strokes on a 256-unit canvas.
  /// Pass `tight` at small sizes to crop the source padding and keep the paths legible.
  size?: number;
  /// Crop to the glyph's stroke bounds instead of its padded 256-unit canvas. At the title
  /// bar's 16px size this keeps each stroke about 1.9 CSS pixels wide rather than 1.4.
  tight?: boolean;
  title?: string;
  class?: string;
}

/// The selected paths' stroke bounds, including the 11-unit half-stroke around the outer edges.
const TIGHT_VIEW_BOX = "37 37 182 182";

/// Renders the selected mark with its original geometry and theme-token colors.
export default function BrandMark(props: BrandMarkProps) {
  const size = () => props.size ?? 26;
  return (
    <svg
      width={size()}
      height={size()}
      viewBox={props.tight ? TIGHT_VIEW_BOX : "0 0 256 256"}
      fill-rule="evenodd"
      clip-rule="evenodd"
      class={props.class}
      role={props.title ? "img" : "presentation"}
      aria-label={props.title}
      aria-hidden={props.title ? undefined : "true"}
    >
      <g fill="none" fill-rule="nonzero" stroke="var(--brand-ink)" stroke-width="22">
        <path d="M108,48L56,48C50.667,48 48,50.667 48,56L48,120C48,125.333 50.667,128 56,128L80,128C85.333,128 88,130.667 88,136L88,168.125" />
        <path d="M87.727,88.04L144,88.04C149.333,88.04 152,85.373 152,80.04L152,56C152,50.667 154.667,48 160,48L200,48C205.333,48 208,50.667 208,56L208,200C208,205.333 205.333,208 200,208L160,208" />
        <path d="M157.091,128L208,128" />
        <path d="M48,172L48,200C48,205.333 50.667,208 56,208L120,208C125.333,208 128,205.333 128,200L128,176.125C128,170.792 130.667,168.125 136,168.125L168.188,168.125" />
      </g>
      <rect x="112" y="112" width="32" height="32" fill="var(--signal)" />
    </svg>
  );
}
