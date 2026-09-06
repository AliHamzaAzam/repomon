#!/usr/bin/env python3
"""Install compiled macOS icons into packaging and optionally an existing Repomon app."""
from pathlib import Path
import argparse
import datetime
import hashlib
import json
import os
import plistlib
import shutil
import tempfile

ROOT = Path(__file__).resolve().parents[4]
SOURCE = ROOT / "docs/brand/final/macos/compiled"
DEST = ROOT / "apps/desktop/src-tauri"
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--app",type=Path,help="Also replace only the icon resources in an installed Repomon.app")
args = parser.parse_args()

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

icon_source = SOURCE.parent / "Repomon.icon"
provenance = json.loads((SOURCE.parent / "provenance.json").read_text())
saved = {str(p.relative_to(icon_source)):digest(p) for p in sorted(icon_source.rglob("*")) if p.is_file()}
if provenance.get("icon_source_sha256") != saved:
    raise SystemExit("Saved icon changed since compilation. Run build-macos.py before installing.")
for name in ["Assets.car", "Repomon.icns"]:
    assert (SOURCE / name).is_file() and (SOURCE / name).stat().st_size > 0, name
for source, dest in [(SOURCE / "Assets.car",DEST / "macos/Assets.car"),
                     (SOURCE / "Repomon.icns",DEST / "icons/icon.icns")]:
    shutil.copy2(source,dest)
    assert source.read_bytes() == dest.read_bytes()
    print(f"Installed {dest.relative_to(ROOT)}")

if args.app:
    app = args.app.resolve(strict=True)
    info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
    if (info.get("CFBundleIdentifier"),info.get("CFBundleIconName"),info.get("CFBundleIconFile")) != ("com.repomon.desktop","Repomon","icon.icns"):
        raise SystemExit("The selected app does not use the expected Repomon icon resources.")
    executable = app / "Contents/MacOS" / info["CFBundleExecutable"]
    executable_before = digest(executable)
    resources = app / "Contents/Resources"
    backup = SOURCE.parent / "installed-backups" / datetime.datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    backup.mkdir(parents=True)
    mapping = [(SOURCE/"Assets.car",resources/"Assets.car"),
               (SOURCE/"Repomon.icns",resources/"icon.icns")]
    for _,dest in mapping:
        shutil.copy2(dest,backup/dest.name)
    try:
        for source,dest in mapping:
            # Replace each resource atomically; never touch or restart the executable.
            with tempfile.NamedTemporaryFile(dir=resources,prefix=".repomon-icon-",delete=False) as temp:
                staged = Path(temp.name)
            try:
                shutil.copy2(source,staged)
                staged.replace(dest)
            finally:
                staged.unlink(missing_ok=True)
            assert digest(source)==digest(dest)
        assert digest(executable)==executable_before
    except Exception:
        for _,dest in mapping:
            shutil.copy2(backup/dest.name,dest)
        raise
    os.utime(app,None)
    receipt = {"app":str(app),"backup":str(backup),"executable_sha256":executable_before,
               "executable_unchanged":True,"resources_sha256":{dest.name:digest(dest) for _,dest in mapping}}
    (SOURCE.parent/"installed-app-receipt.json").write_text(json.dumps(receipt,indent=2)+"\n")
    print(f"Updated icon resources in {app}; executable unchanged.")
    print(f"Previous icon resources saved in {backup}")
