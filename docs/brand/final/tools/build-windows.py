#!/usr/bin/env python3
"""Build deterministic Windows icons from the approved Repomon SVG.

Requires Python 3 and rsvg-convert. No third-party Python packages.
"""
from __future__ import annotations

import base64
import hashlib
from pathlib import Path
import re
import shutil
import struct
import subprocess
import xml.etree.ElementTree as ET

BRAND = Path(__file__).resolve().parents[2]
SOURCE = BRAND / "repo-logo-final.svg"
OUT = BRAND / "final" / "windows"
SIZES = (16, 20, 24, 32, 40, 48, 64, 96, 128, 256, 512, 1024)
ICO_SIZES = tuple(size for size in SIZES if size <= 256)


def render(svg: Path, png: Path, width: int, height: int | None = None) -> None:
    subprocess.run(
        ["rsvg-convert", "--width", str(width), "--height", str(height or width),
         "--output", str(png), str(svg)], check=True
    )


def png_image(size: int, x: int, y: int, display: int | None = None) -> str:
    encoded = base64.b64encode((OUT / f"repomon-{size}.png").read_bytes()).decode()
    extent = display or size
    return (f'<image x="{x}" y="{y}" width="{extent}" height="{extent}" '
            f'href="data:image/png;base64,{encoded}"/>')


def main() -> None:
    if not shutil.which("rsvg-convert"):
        raise SystemExit("rsvg-convert is required")
    original = SOURCE.read_text()
    root = ET.fromstring(original)
    ns = {"s": "http://www.w3.org/2000/svg"}
    assert root.attrib["viewBox"] == "0 0 256 256"
    assert len(root.findall(".//s:path", ns)) == 4
    assert len(root.findall(".//s:rect", ns)) == 1
    group = re.search(r"<g>.*</g>", original, re.S)
    assert group, "Expected the approved SVG's untransformed group"
    OUT.mkdir(parents=True, exist_ok=True)
    source_svg = OUT / "repomon-windows.svg"
    source_svg.write_text(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" '
        'viewBox="0 0 256 256" fill-rule="evenodd" clip-rule="evenodd">\n'
        '  <title>Repomon Windows app icon</title>\n'
        '  <rect x="16" y="16" width="224" height="224" rx="44" fill="#F6F5F0"/>\n'
        '  <g transform="translate(17.92 17.92) scale(0.86)">\n'
        + group.group(0) + '\n  </g>\n</svg>\n'
    )
    for size in SIZES:
        render(source_svg, OUT / f"repomon-{size}.png", size)

    # Modern Windows accepts PNG-compressed RGBA icon frames losslessly.
    frames = [(size, (OUT / f"repomon-{size}.png").read_bytes()) for size in ICO_SIZES]
    offset = 6 + 16 * len(frames)
    entries, payload = [], []
    for size, data in frames:
        assert data[:8] == b"\x89PNG\r\n\x1a\n"
        width, height, depth, color = struct.unpack(">IIBB", data[16:26])
        assert (width, height, depth, color) == (size, size, 8, 6), (
            size, width, height, depth, color
        )
        entries.append(struct.pack("<BBBBHHII", size % 256, size % 256,
                                   0, 0, 1, 32, len(data), offset))
        payload.append(data)
        offset += len(data)
    ico = struct.pack("<HHH", 0, 1, len(frames)) + b"".join(entries + payload)
    (OUT / "repomon.ico").write_bytes(ico)

    # Validate each ICO directory entry against its original PNG payload.
    for index, (size, data) in enumerate(frames):
        entry = struct.unpack_from("<BBBBHHII", ico, 6 + index * 16)
        assert (entry[0] or 256, entry[1] or 256) == (size, size)
        assert ico[entry[7]:entry[7] + entry[6]] == data

    preview = [
        '<svg xmlns="http://www.w3.org/2000/svg" width="1280" height="820" viewBox="0 0 1280 820">',
        '<rect width="1280" height="820" fill="#E8E8E5"/>',
        '<g font-family="Helvetica Neue,Arial,sans-serif" fill="#253B47">',
        '<text x="56" y="61" font-size="30" font-weight="600">Repomon · Windows icon</text>',
        '<text x="58" y="91" font-size="16">Approved symbol · flat warm white tile</text>',
        '<rect x="664" y="124" width="560" height="460" rx="24" fill="#253B47"/>',
        png_image(512, 56, 110, 472),
        png_image(512, 740, 150, 408),
        '<text x="56" y="626" font-size="15" font-weight="600">Native-size PNGs</text>',
    ]
    for size, x in ((16, 64), (20, 145), (24, 226), (32, 310), (40, 400),
                    (48, 498), (64, 606), (96, 738), (128, 908)):
        preview.append(png_image(size, x, 655 + (128 - size) // 2))
        preview.append(f'<text x="{x + size / 2}" y="804" text-anchor="middle" font-size="13">{size}</text>')
    preview.append('</g></svg>')
    preview_svg = OUT / "preview.svg"
    preview_svg.write_text("\n".join(preview))
    render(preview_svg, OUT / "preview.png", 1280, 820)

    digest = hashlib.sha256(SOURCE.read_bytes()).hexdigest()
    (OUT / "README.md").write_text(f"""# Repomon Windows icon

Built from `../../repo-logo-final.svg` without changing its paths, stroke widths, center square, or colors. The entire approved mark is uniformly scaled to 86% about its center.

- Source charcoal: **#253B47**
- Source orange: **#EF7846**
- Flat tile: **#F6F5F0**
- Transparent outer padding: **6.25%** on every side
- Editable composition: `repomon-windows.svg`
- PNG sizes: {', '.join(map(str, SIZES))} px
- Windows ICO: `repomon.ico`, with 32-bit RGBA PNG frames at {', '.join(map(str, ICO_SIZES))} px
- Inspection board: `preview.png` (small images displayed at native size)

The ICO uses PNG-compressed frames for modern Windows. The master SVG's geometry is preserved at every size; no alternate small-size glyph is substituted. No production assets are changed by this build.

Rebuild with `python3 docs/brand/final/tools/build-windows.py` from the repository root. Requires `rsvg-convert` on PATH.

Install the ICO, shared app PNGs, and existing Windows Store PNGs into `apps/desktop/src-tauri/icons/` with `python3 docs/brand/final/tools/install-windows.py`. The installer rebuilds first, retains Store asset dimensions, and leaves `icon.icns` to the separate macOS workflow.

Validation: every PNG is the requested square size with 8-bit RGBA channels, and every ICO directory entry resolves to its matching PNG payload.

Approved source SHA-256: `{digest}`
""")
    print(f"Created {len(SIZES)} PNGs, {len(frames)}-frame ICO, source SVG, and preview in {OUT}")
    print(f"Source SHA-256: {digest}")


if __name__ == "__main__":
    main()
