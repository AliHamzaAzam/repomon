# Brief D1: full UI/UX pass, responsive layout, and the new brand colors as the default theme

Operator request (2026-09-06): "Do a full ui/ux pass ensuring responsive design and also I updated
the logo so use that and its colors for Repomon default view." Plus a screenshot of an overflow:
the Repomind panel header at a narrow width, where the "Expand" button overlaps the status pill
("Idle"/"Start" row, `RepomindPanel.tsx` header) instead of wrapping or collapsing.

Branch `design/responsive-brand-pass` in a fresh worktree `/private/tmp/repomon-design-pass` from
current main (781f9be or later, which contains the new logo: `docs/brand/final/` with
`manifest.json`, `preview.svg`, the macOS Icon Composer bundle and Windows assets;
`apps/desktop/src/components/BrandMark.tsx`; `apps/desktop/public/favicon.svg`; exploration in
`design/repomon-logo/` is untracked and not needed). Run `/frontend-design` and `/impeccable`
(the `audit`, `adapt`, `layout`, and `polish` commands are the relevant ones) before editing and
apply them throughout. Operate mode: this is a dense desktop tool; scanability and consistency
outrank expression.

## Part A. Brand colors become the default theme

1. Derive the palette from the shipped logo (`docs/brand/final/preview.svg` and the Icon Composer
   layer SVGs, plus `BrandMark.tsx`): the primary accent, its tints, and the neutral ground. Map
   them onto the existing CSS custom properties in `apps/desktop/src/index.css` and `theme.ts`
   (the app already has a token system: `--signal`, `--attention`, `--surface`, `--raised`,
   `--line`, `--foreground`, `--muted`, the dataviz palette, and the tint system with the tint
   preview). The DEFAULT theme/tint (what a fresh install shows) uses the logo's accent as the
   signal color and its ground as the surface family, in both light and dark. Existing tints stay
   selectable; do not remove any. Contrast: every text/background pair at WCAG AA (4.5:1 body,
   3:1 large and UI), checked with a script in `qa/` that reads the tokens and prints the ratios.
2. Brand lockup and window chrome (`BrandLockup.tsx`, `WindowChrome.tsx`, onboarding Welcome,
   the About/Settings header, the repomind row/toolbar dot) use the new mark and the same accent.
   No hex literals in components; everything through tokens.
3. The Usage view and Repomind panel charts (dataviz palette) are re-derived from the new accent so
   series colors sit in the same family; run the dataviz palette validator if present.

## Part B. Responsive and overflow pass, whole app

4. Establish breakpoints the app actually hits as a desktop window: narrow (about 900px wide),
   medium (1200), wide (1600+), and short (height under 720). Every top-level view (Git, Editor,
   Usage, Control, Multitasking, Extensions, Supervision, Repomail, Repomind, Settings modal,
   Onboarding, the fleet sidebar, the right panel host, the terminal bay, toolbar and header) must
   render without horizontal page scroll, without overlapping controls, and with truncation only
   where designed (title attributes on truncated text). Fix: header/toolbar groups collapse to
   icons with tooltips before they overflow; panel headers wrap actions into an overflow menu
   (the Repomind "Expand" case); tables scroll inside their own container; two-column cards stack;
   sidebar min widths hold; modals cap to the viewport and scroll internally.
5. Density and rhythm: one spacing scale, consistent card padding and section headers, consistent
   button sizes and icon sizes, consistent empty states; remove one-off margins.
6. Keyboard and focus: visible focus rings everywhere with the new accent, tab order through the
   toolbar and panels, Escape closes overlays in a consistent order.
7. Verification: add responsive tests where the codebase has patterns (jsdom width mocks, class
   assertions), and produce screenshots at the four sizes for the main views using the existing
   Vitest browser/Playwright setup if present, saved under `qa/` (untracked). If no screenshot
   tooling exists, say so and list what you checked by reading.

## Rules and gate

Worktree only; never edit the main checkout; never bind a daemon to `/tmp/repomon-azaleas.sock`
or copy the production database; never kill processes by name pattern; never `git add -A`. Zero
hex color literals in components (tokens only; the token definitions in `index.css` may carry
hex), no emoji (SVG icons), no em-dashes in code, comments, or copy. Keep behavior unchanged:
this is visual and layout work. Gate: in `apps/desktop` `bun run check`, `bun run test
--maxWorkers=2`, `bun run bindings:check`. Commits: 1-line Conventional Commits per numbered item
where sensible, no co-author trailer. Do not merge, push, or build the bundle. Report commit
hashes, per-item summary, the palette you derived (token name to value), contrast results, test
names, gate tails, screenshots produced, and what could not be verified without the live app.
