# Brief J: Image viewer v2

From the operator's screenshot of a 432 x 900 PNG in the center Editor mode at a 1400px pane:
the image occupies a ~520px column on the left, the remaining pane is empty; the toolbar is only
name, dimensions, size, and a "Fit" button; the status line shows code items (Auto Language,
Ln/Col, Spaces, Wrap, Whitespace). Root cause is the same as the PDF viewer v1 (fixed in
1355bb5): the tab content container is a row-direction flex box that never stretches a child's
width; the viewer must size itself `h-full w-full min-h-0 min-w-0`.

Branch: `feat/editor-image-viewer-v2` in a fresh worktree at `/private/tmp/repomon-feat-editor-image-v2`
from current main (1355bb5 or later).

## Target (`apps/desktop/src/components/ImageViewer.tsx`, both editor surfaces)

- **Byte source**: stream through the asset protocol like the PDF viewer (`ensureWorktreeAssetsAllowed`
  plus `convertFileSrc` from `apps/desktop/src/ipc/assets.ts`) instead of `file.read_raw`
  base64, so there is no 8 MiB cap and no memory doubling. Keep `file.read_raw` as the fallback
  only if the asset URL fails to load (listen for the `img` `error` event).
- **Layout**: fills the tab; a `var(--background)` stage with a subtle dot or checkerboard
  pattern built from CSS variables (transparent PNGs must read correctly in every theme); the
  image centered; no dead columns.
- **Zoom and pan**: modes fit, 1:1, and free zoom from 10% to 800%; Cmd/Ctrl plus wheel zooms
  around the cursor, plain wheel scrolls/pans when zoomed, drag pans when larger than the
  stage, double-click toggles fit and 1:1. `image-rendering: pixelated` above 200% so pixel
  art and icons stay crisp. Fit is recomputed on stage resize while the mode is fit.
- **Toolbar** (app style, SVG icons only): file name, pixel dimensions, size, zoom out, zoom
  percentage (click resets to fit), zoom in, fit, 1:1, and "Open in system viewer". Keyboard
  while focused: Cmd/Ctrl plus `=`, `-`, `0` (fit), `1` (1:1).
- **Animated GIF and WebP**: play as usual (the `img` element handles it); show "animated" in the
  status line when the file is a GIF.
- **SVG preview**: `.svg` tabs stay text (editable) and gain a "Preview" toggle in the status
  line (and `mod+shift+v`, the same chord as markdown preview, contextual by tab kind) that shows
  the rendered SVG beside the editor in the same split component the markdown preview uses,
  updating live from the buffer (render from the buffer text via a `Blob` URL, sanitized to
  strip `<script>` and `on*` attributes; never from disk, so unsaved edits preview). Reuse the
  split ratio persistence.
- **Status line** for image tabs: "PNG · 432 x 900 · 69.5 KB" (plus "animated" for GIFs) on the
  left and the zoom on the right; code items hidden (the PDF v2 change already made the status
  line kind-aware; extend it).
- **States**: loading skeleton, load error with the open-externally button primary, and a
  "too large to decode" guard: above 50 MB show the error state with the external button
  instead of trying to decode.
- Request token on load so a slow image cannot land on a different tab.

## Tests

`ImageViewer.test.tsx`: renders the asset URL; zoom buttons and keyboard change the scale and
label; fit recomputes on resize; 1:1 and fit toggle on double-click; error state shows the
primary external button; stale load ignored after a path change. `EditorWorkspace.test.tsx`:
image tab status line shows kind, dimensions, size, zoom and hides code items; svg tab shows the
Preview toggle and the split renders sanitized markup (script and on* stripped).

## Rules and gate

Run `/frontend-design` and `/impeccable` for the stage, toolbar, and states. Zero hex color
literals; no emoji; no em-dashes. Work only in the worktree; never touch the main checkout,
`/Applications/Repomon.app`, or `/tmp/repomon-azaleas.sock`; never kill by name pattern; never
`git add -A`. Gate: `bun run check`, `bun run test`, `bun run bindings:check` in `apps/desktop`;
`cargo check -p repomon-desktop` at the worktree root (no Rust changes expected). Commits:
1-line Conventional Commits, no co-author trailer. Do not merge, push, or build the bundle.
Report commit hashes, gate tails, and what you could not verify without a screenshot.
