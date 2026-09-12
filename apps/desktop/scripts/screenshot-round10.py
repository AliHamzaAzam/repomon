"""Capture Round 10 native-commands and pending-message screenshots.

Run vite.screenshot.config.ts on localhost:4178 first. Screenshots stay ignored in qa/.
The `pending-long-before` scenario needs real scrollbars visible (it demonstrates the nested-
scrollbar defect), so it is the one entry that does not pass --hide-scrollbars.
"""
import os
import pathlib
import subprocess
import sys
import tempfile
import time

root = pathlib.Path(__file__).resolve().parents[3]
output = root / "qa" / os.environ.get("REPOMON_SCREENSHOT_OUTPUT", "round10")
output.mkdir(parents=True, exist_ok=True)
chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
sizes_default = ((1440, 900), (1040, 680))
sizes_breakpoints = ((1440, 900), (1040, 680), (800, 700), (480, 700))
scenarios = {
    "native-model": ("surface=conversation&case=native-model", sizes_default, True),
    "native-palette": ("surface=conversation&case=native-palette", sizes_default, True),
    "native-empty": ("surface=conversation&case=native-empty", sizes_default, True),
    "pending-long-before": ("surface=conversation&case=pending-long", sizes_default, False),
    "pending-long-after": ("surface=conversation&case=pending-long", sizes_breakpoints, True),
}
if len(sys.argv) > 1:
    scenarios = {name: scenarios[name] for name in sys.argv[1:]}
for name, (query, sizes, hide_scrollbars) in scenarios.items():
    for theme in ("light", "dark"):
        for width, height in sizes:
            path = output / f"{name}-{theme}-{width}x{height}.png"
            path.unlink(missing_ok=True)
            with tempfile.TemporaryDirectory(prefix="repomon-r10-shot-") as profile:
                args = [
                    chrome, "--headless", "--disable-gpu", "--no-first-run",
                    "--no-default-browser-check", f"--user-data-dir={profile}",
                    f"--window-size={width},{height}",
                    "--force-device-scale-factor=1", "--virtual-time-budget=5000",
                ]
                if hide_scrollbars:
                    args.append("--hide-scrollbars")
                args += [f"--screenshot={path}", f"http://localhost:4178/?{query}&theme={theme}"]
                process = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                try:
                    deadline = time.monotonic() + 45
                    while time.monotonic() < deadline:
                        if path.exists() and path.read_bytes().endswith(b"IEND\xaeB`\x82"):
                            break
                        if process.poll() is not None:
                            raise SystemExit(f"Chrome exited {process.returncode}: {path}")
                        time.sleep(0.2)
                    else:
                        raise SystemExit(f"Capture timed out: {path}")
                finally:
                    if process.poll() is None:
                        process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
                print(path, flush=True)
