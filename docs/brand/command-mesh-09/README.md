# Repomon — Command Mesh evolution

The user's latest direction is to take inspiration from the current logo. This exploratory round is based on the actual shipped vector in `apps/desktop/public/favicon.svg` and the current icon in `docs/logo.png`. It preserves the recognizable asymmetric interlocks, long right edge, unequal inner shelves, square footprint, and isolated central node.

1. Source Mesh: the original 14 source rectangles and central node, normalized to a 256-unit canvas and recolored for comparison.
2. Soft Mesh: extracted union contours of the original rectangles, rounded consistently while preserving their path arrangement.
3. Open Mesh: a related arrangement with wider channels and a uniform stroke, softened bends and straight terminals.
4. Stepped Mesh: a bolder grid interpretation with square corners and a larger central square.

These are new exploratory files. The shipped app artwork remains unchanged.

Files: `overview.png` and `overview.svg` compare the original and variations; `review-board.html` presents larger studies; numbered `.svg` / `-mono.svg` files are transparent symbol masters. `build.py` derives the source and constructs the new geometry reproducibly. Study wordmarks are provisional Helvetica Neue with Arial/sans-serif fallback.

Direct vector construction; no ImageGen. Final logo and macOS Liquid Glass / Windows icon production follow selection.
