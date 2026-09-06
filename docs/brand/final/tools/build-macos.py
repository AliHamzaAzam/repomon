#!/usr/bin/env python3
"""Compile the saved Icon Composer bundle without resetting user artwork or settings."""
from pathlib import Path
import hashlib
import json
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[4]
OUT = ROOT / "docs/brand/final/macos"
MASTER = ROOT / "docs/brand/repo-logo-final.svg"
BUNDLE = OUT / "Repomon.icon"

def hashes(directory):
    return {str(p.relative_to(directory)):hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(directory.rglob("*")) if p.is_file()}

if not (BUNDLE / "icon.json").is_file():
    raise SystemExit(f"Save the approved Icon Composer document at {BUNDLE} first.")
before = hashes(BUNDLE)
json.loads((BUNDLE / "icon.json").read_text())
developer = Path(subprocess.check_output(["xcode-select","-p"],text=True).strip())
ictool = developer.parent / "Applications/Icon Composer.app/Contents/Executables/ictool"
renditions = [("Default","repomon-macos-1024.png"),("Dark","repomon-macos-dark-1024.png"),
              ("ClearLight","repomon-macos-clear-light-1024.png"),("ClearDark","repomon-macos-clear-dark-1024.png"),
              ("TintedLight","repomon-macos-tinted-light-1024.png"),("TintedDark","repomon-macos-tinted-dark-1024.png")]
# Compile a consistent snapshot; the open Composer document is never rewritten.
with tempfile.TemporaryDirectory(prefix="repomon-icon-build-") as temp:
    snapshot = Path(temp) / "Repomon.icon"
    shutil.copytree(BUNDLE,snapshot)
    if hashes(snapshot) != before:
        raise SystemExit("Icon Composer document changed during snapshot; rerun after saving.")
    for rendition, filename in renditions:
        subprocess.run([str(ictool),str(snapshot),"--export-image","--output-file",str(OUT/filename),
                        "--platform","macOS","--rendition",rendition,"--width","1024","--height","1024","--scale","1"],check=True)
    catalog = OUT / "Empty.xcassets"
    catalog.mkdir(exist_ok=True)
    (catalog / "Contents.json").write_text('{"info":{"author":"xcode","version":1}}\n')
    compiled = OUT / "compiled"
    compiled.mkdir(exist_ok=True)
    subprocess.run(["xcrun","actool","--compile",str(compiled),"--app-icon","Repomon",
                    "--output-partial-info-plist",str(compiled/"partial.plist"),"--platform","macosx",
                    "--minimum-deployment-target","11.0","--include-all-app-icons",str(snapshot),str(catalog)],check=True)
    iconset = Path(temp) / "Repomon.iconset"
    subprocess.run(["iconutil","--convert","iconset","--output",str(iconset),str(compiled/"Repomon.icns")],check=True)
    shutil.copy2(iconset/"icon_128x128@2x.png",OUT/"repomon-macos-legacy-256.png")
if hashes(BUNDLE) != before:
    raise SystemExit("Icon Composer document changed during compilation; rerun before installing.")
(OUT / "provenance.json").write_text(json.dumps({
    "source":"docs/brand/repo-logo-final.svg",
    "source_sha256":hashlib.sha256(MASTER.read_bytes()).hexdigest(),
    "icon_source":"docs/brand/final/macos/Repomon.icon",
    "icon_source_sha256":before,
    "settings":"Compiled exactly as saved in Icon Composer; source bundle was not modified.",
    "geometry":"Closed filled outline layers retained as saved.",
    "renderer":"Apple Icon Composer 2.0, Xcode 27 toolchain",
    "compiled_catalog_name":"Repomon", "appearances":[r for r,_ in renditions]
},indent=2)+"\n")
print(f"Compiled saved native macOS icon: {OUT}")
print("Verified all source bundle files remained byte-identical.")
