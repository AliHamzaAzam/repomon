"""Capture Settings > System with all eight Coding Agents & Tooling rows in frame.

The panel's defect is how eight rows read together, so every shot is the whole card at both
required window sizes in both themes; a crop of one row would prove nothing. The fixture's
`system.doctor` carries six detected and two absent, and scrolls the card into view itself.

Run vite.screenshot.config.ts on localhost:4183 first, then:
  python3 scripts/screenshot-agents-panel.py before
"""
import os
import pathlib
import subprocess
import sys
import tempfile
import time

label = sys.argv[1] if len(sys.argv) > 1 else "after"
root = pathlib.Path(__file__).resolve().parents[3]
output = root / "qa" / os.environ.get("REPOMON_SCREENSHOT_OUTPUT", "agents-panel") / label
output.mkdir(parents=True, exist_ok=True)
chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
port = os.environ.get("REPOMON_SHOT_PORT", "4183")
sizes = ((1440, 900), (1040, 680))
query = "surface=settings&tab=System"

failures = []
for theme in ("light", "dark"):
    for width, height in sizes:
        path = output / f"agents-panel-{theme}-{width}x{height}.png"
        path.unlink(missing_ok=True)
        with tempfile.TemporaryDirectory(prefix="repomon-agents-shot-", ignore_cleanup_errors=True) as profile:
            args = [
                chrome, "--headless", "--disable-gpu", "--no-first-run",
                "--no-default-browser-check", f"--user-data-dir={profile}",
                f"--window-size={width},{height}",
                "--force-device-scale-factor=1", "--virtual-time-budget=6000",
                "--hide-scrollbars",
                f"--screenshot={path}", f"http://localhost:{port}/?{query}&theme={theme}",
            ]
            process = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                deadline = time.monotonic() + 45
                while time.monotonic() < deadline:
                    if path.exists() and path.read_bytes().endswith(b"IEND\xaeB`\x82"):
                        break
                    if process.poll() is not None:
                        time.sleep(0.5)
                        break
                    time.sleep(0.25)
            finally:
                if process.poll() is None:
                    process.terminate()
                process.wait(timeout=10)
        if path.exists() and path.read_bytes().endswith(b"IEND\xaeB`\x82"):
            print(f"wrote {path.relative_to(root)}")
        else:
            failures.append(str(path.relative_to(root)))
            print(f"FAILED {path.relative_to(root)}")

if failures:
    sys.exit(f"{len(failures)} screenshot(s) did not complete")
