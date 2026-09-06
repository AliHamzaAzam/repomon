# Repomon Windows icon

Built from `../../repo-logo-final.svg` without changing its paths, stroke widths, center square, or colors. The entire approved mark is uniformly scaled to 86% about its center.

- Source charcoal: **#253B47**
- Source orange: **#EF7846**
- Flat tile: **#F6F5F0**
- Transparent outer padding: **6.25%** on every side
- Editable composition: `repomon-windows.svg`
- PNG sizes: 16, 20, 24, 32, 40, 48, 64, 96, 128, 256, 512, 1024 px
- Windows ICO: `repomon.ico`, with 32-bit RGBA PNG frames at 16, 20, 24, 32, 40, 48, 64, 96, 128, 256 px
- Inspection board: `preview.png` (small images displayed at native size)

The ICO uses PNG-compressed frames for modern Windows. The master SVG's geometry is preserved at every size; no alternate small-size glyph is substituted. No production assets are changed by this build.

Rebuild with `python3 docs/brand/final/tools/build-windows.py` from the repository root. Requires `rsvg-convert` on PATH.

Install the ICO, shared app PNGs, and existing Windows Store PNGs into `apps/desktop/src-tauri/icons/` with `python3 docs/brand/final/tools/install-windows.py`. The installer rebuilds first, retains Store asset dimensions, and leaves `icon.icns` to the separate macOS workflow.

Validation: every PNG is the requested square size with 8-bit RGBA channels, and every ICO directory entry resolves to its matching PNG payload.

Approved source SHA-256: `47bfa2d04bb6abb64d0b6a9456dd33157565eb626b6af538bd8a908602378ea4`
