#!/usr/bin/env python3
"""Rebuild and install Repomon's Windows/shared PNG packaging assets.

Only writes the named PNG/ICO assets in apps/desktop/src-tauri/icons/.
The macOS icon.icns is managed separately and is never opened or written.
"""
from __future__ import annotations

from pathlib import Path
import shutil
import struct
import subprocess
import sys

TOOLS = Path(__file__).resolve().parent
REPO = TOOLS.parents[3]
WINDOWS = TOOLS.parent / "windows"
DEST = REPO / "apps" / "desktop" / "src-tauri" / "icons"
PNG_COPIES = {
    "32x32.png": 32,
    "64x64.png": 64,
    "128x128.png": 128,
    "128x128@2x.png": 256,
    "icon.png": 1024,
}
STORE_ASSETS = {
    "StoreLogo.png": 50,
    "Square30x30Logo.png": 30,
    "Square44x44Logo.png": 44,
    "Square71x71Logo.png": 71,
    "Square89x89Logo.png": 89,
    "Square107x107Logo.png": 107,
    "Square142x142Logo.png": 142,
    "Square150x150Logo.png": 150,
    "Square284x284Logo.png": 284,
    "Square310x310Logo.png": 310,
}


def check_png(path: Path, size: int) -> None:
    data = path.read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", path
    assert struct.unpack(">IIBB", data[16:26]) == (size, size, 8, 6), path


def main() -> None:
    # Preflight packaging names/dimensions before modifying any asset.
    for name in [*PNG_COPIES, *STORE_ASSETS, "icon.ico"]:
        if not (DEST / name).is_file():
            raise SystemExit(f"Expected packaging asset missing: {DEST / name}")
    for name, size in STORE_ASSETS.items():
        check_png(DEST / name, size)

    subprocess.run([sys.executable, str(TOOLS / "build-windows.py")], check=True)
    shutil.copyfile(WINDOWS / "repomon.ico", DEST / "icon.ico")
    for name, size in PNG_COPIES.items():
        shutil.copyfile(WINDOWS / f"repomon-{size}.png", DEST / name)
    for name, size in STORE_ASSETS.items():
        subprocess.run(
            ["rsvg-convert", "--width", str(size), "--height", str(size),
             "--output", str(DEST / name), str(WINDOWS / "repomon-windows.svg")],
            check=True,
        )

    for name, size in (PNG_COPIES | STORE_ASSETS).items():
        check_png(DEST / name, size)
    assert (DEST / "icon.ico").read_bytes() == (WINDOWS / "repomon.ico").read_bytes()
    for name, size in PNG_COPIES.items():
        assert (DEST / name).read_bytes() == (WINDOWS / f"repomon-{size}.png").read_bytes()
    print(f"Installed and verified {len(PNG_COPIES) + len(STORE_ASSETS)} PNGs and 1 ICO in {DEST}")


if __name__ == "__main__":
    main()
