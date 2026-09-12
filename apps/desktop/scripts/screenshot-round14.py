"""Capture Round 14 model-panel screenshots: the Claude-Desktop-style picker, dull vs. rich,
plus the "unconfirmed one-shot" informational state defect ONE required.

Run vite.screenshot.config.ts on localhost:4178 first. Screenshots stay ignored in qa/.
"""
import os
import pathlib
import subprocess
import sys
import tempfile
import time

root = pathlib.Path(__file__).resolve().parents[3]
output = root / "qa" / os.environ.get("REPOMON_SCREENSHOT_OUTPUT", "round14")
output.mkdir(parents=True, exist_ok=True)
chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
sizes = ((1440, 900), (1040, 680))
scenarios = {
    "native-model": "surface=conversation&case=native-model",
    "native-model-dull": "surface=conversation&case=native-model-dull",
    "native-model-unconfirmed": "surface=conversation&case=native-model-unconfirmed",
}
if len(sys.argv) > 1:
    scenarios = {name: scenarios[name] for name in sys.argv[1:]}
for name, query in scenarios.items():
    for theme in ("light", "dark"):
        for width, height in sizes:
            path = output / f"{name}-{theme}-{width}x{height}.png"
            path.unlink(missing_ok=True)
            with tempfile.TemporaryDirectory(prefix="repomon-r14-shot-") as profile:
                args = [
                    chrome, "--headless", "--disable-gpu", "--no-first-run",
                    "--no-default-browser-check", f"--user-data-dir={profile}",
                    f"--window-size={width},{height}",
                    "--force-device-scale-factor=1", "--virtual-time-budget=5000",
                    "--hide-scrollbars",
                    f"--screenshot={path}", f"http://localhost:4178/?{query}&theme={theme}",
                ]
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
