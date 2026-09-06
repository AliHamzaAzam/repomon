#!/usr/bin/env python3
"""Regenerate only SVG layers from the master; preserve Icon Composer settings."""
from pathlib import Path
import json
import subprocess
import tempfile
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[4]
OUT = ROOT / "docs/brand/final/macos"
MASTER = ROOT / "docs/brand/repo-logo-final.svg"
BUNDLE = OUT / "Repomon.icon"
ASSETS = BUNDLE / "Assets"
ASSETS.mkdir(parents=True, exist_ok=True)
ET.register_namespace("", "http://www.w3.org/2000/svg")
source = ET.parse(MASTER).getroot()
ns = {"s": "http://www.w3.org/2000/svg"}
paths = source.findall(".//s:path", ns)
stroke_inputs = []
for path in paths:
    style = dict(item.split(":", 1) for item in path.attrib["style"].split(";") if item)
    assert style["fill"] == "none" and style["stroke"] == "rgb(37,59,71)"
    stroke_inputs.append({"d":path.attrib["d"], "width":float(style["stroke-width"].removesuffix("px"))})
with tempfile.TemporaryDirectory(prefix="repomon-outline-") as temp:
    executable = Path(temp) / "outline-strokes"
    subprocess.run(["xcrun","swiftc","-module-cache-path",str(Path(temp)/"modules"),
                    str(Path(__file__).with_name("outline-strokes.swift")),"-o",str(executable)],check=True)
    outlines = json.loads(subprocess.check_output([str(executable)],input=json.dumps(stroke_inputs),text=True))
    assert len(outlines) == len(paths)
    for outline in outlines:
        # A fill override must not implicitly close an open source centerline.
        assert outline.endswith("Z") and outline.count("M") == outline.count("Z")
for name, tag in [("01-command-mesh", "path"), ("02-command-node", "rect")]:
    layer = ET.Element("{http://www.w3.org/2000/svg}svg", {"width":"1024", "height":"1024", "viewBox":"0 0 256 256"})
    if tag == "path":
        for d in outlines:
            ET.SubElement(layer,"{http://www.w3.org/2000/svg}path",{
                "d":d,"fill":"#253B47","fill-rule":"nonzero"})
    else:
        for element in source.findall(f".//s:{tag}", ns):
            layer.append(element)
    ET.ElementTree(layer).write(ASSETS / f"{name}.svg", encoding="unicode", xml_declaration=True)

print(f"Updated outlined SVG layers in {ASSETS}; icon.json was preserved.")
