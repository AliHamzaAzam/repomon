# Brief I: PDF viewer v2

The first PDF viewer (merged at 39d9904) is not good enough. From the operator's screenshot of
a 60 KB, one-page CV open in the center Editor mode at a 1400px-wide pane:

1. The viewer occupies roughly a 420px-wide column at the left of the pane; the remaining
   two thirds of the pane are empty. The iframe is not filling its container.
2. The page renders small in the middle of a tall white column with large white dead space above
   and below it; no fit-to-width, no zoom control from the app.
3. The status line still shows the code editor's items (Auto Language, Ln 1 Col 1, Spaces: 2,
   Wrap, Whitespace) for a PDF tab.
4. The toolbar is only the file name, size, and "Open in system viewer".

Branch: `feat/editor-pdf-viewer-v2` in a fresh worktree at `/private/tmp/repomon-feat-editor-pdf-v2`
from current main.

## Target

Replace the native-iframe approach with an in-app renderer based on `pdfjs-dist` (pin an
exact version in `apps/desktop/package.json`; load the worker through Vite's `?worker` or
`new URL(..., import.meta.url)` pattern so it is bundled, not fetched from a CDN). Keep the
asset-protocol streaming from v1 (`ensureWorktreeAssetsAllowed` + `convertFileSrc`) as the byte
source: `fetch(assetUrl)` -> `ArrayBuffer` -> `pdfjs.getDocument({ data })`. Keep the
"Open in system viewer" action.

`PdfViewer.tsx` becomes:
- **Layout**: fills the whole tab area (`h-full w-full min-h-0 min-w-0` through every ancestor;
  check the tab content container in `EditorWorkspace.tsx` and `FileEditorPanel.tsx`, which is
  where v1's iframe failed to stretch). Background `var(--background)`; pages drawn on a
  `var(--surface)` card with a 1px `var(--line)` border and a subtle shadow, centered, with
  `gap-3` between pages.
- **Rendering**: one `<canvas>` per page sized to the current zoom at `devicePixelRatio`, plus
  pdf.js's text layer for selection and search-in-page. Virtualize: render only pages within
  one viewport height of the visible area, release canvases far away, re-render on zoom or
  container resize (ResizeObserver, debounced 100 ms). Default zoom = fit width (recomputed on
  resize while the mode is "fit width").
- **Toolbar** (app style, no emoji, SVG icons from `icons.tsx`): file name, "Page N of M" with an
  editable page input, previous/next, zoom out, zoom percentage (click to reset), zoom in, fit
  width, fit page, and "Open in system viewer". Keyboard: PageUp/PageDown and Home/End move
  pages, Cmd/Ctrl plus `=`/`-`/`0` zoom, all while the viewer has focus.
- **Find in page**: Cmd/Ctrl-F inside the viewer opens a small find bar over the text layer
  (case-insensitive), highlights matches with `color-mix(in srgb, var(--attention) 35%, transparent)`,
  Enter/Shift-Enter cycles.
- **States**: loading (skeleton page), error (message and the open-externally button primary),
  password-protected (message, no prompt), and the Linux fallback is no longer needed since
  rendering is in-app; remove the user-agent branch.
- **Status line**: for a `pdf` tab show "PDF · N pages · size" on the left and the zoom on the
  right; hide language, Ln/Col, indent, wrap, whitespace. Make the status line component take
  the tab kind into account rather than assuming a code editor.
- **Memory**: destroy the pdf.js document and cancel render tasks on tab close or file switch;
  a request token guards against a slow load resolving after the tab changed.

## Tests

`PdfViewer.test.tsx` with `pdfjs-dist` mocked (fake document with 3 pages of fixed size):
renders three page slots and only the visible canvases, fit-width recomputes on resize, page
input navigates, zoom buttons change the scale and the label, find highlights and cycles,
error state shows the primary open-externally button, a stale load result is ignored after a
path change. Status line test: a pdf tab hides the code items and shows the page count.
Keep the v1 daemon tests as they are.

## Rules and gate

Run `/frontend-design` and `/impeccable` for the toolbar, page canvas presentation, find bar,
and states. Zero hex color literals; no emoji; no em-dashes. Work only in the worktree; never
touch the main checkout, `/Applications/Repomon.app`, or `/tmp/repomon-azaleas.sock`; never
kill processes by name pattern; never `git add -A`. Gate: `bun run check`, `bun run test`,
`bun run bindings:check` in `apps/desktop`; `cargo check -p repomon-desktop` at the worktree
root (the Rust side should not change). Bundle size: report the added size of the pdfjs chunk
from `bun run build` output. Commits: 1-line Conventional Commits, no co-author trailer. Do not
merge, push, or build the Tauri bundle. Report commit hashes, gate tails, and what you could
not verify without a screenshot.
