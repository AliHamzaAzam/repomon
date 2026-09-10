"""Capture the real HomeScreen against both screenshot fixtures, never a live daemon.

Start vite.screenshot.config.ts on port 4178, then run this with `before` or `after`.
Outputs stay in the worktree's ignored qa/home-pass directory.
"""
import pathlib
import subprocess
import sys
import tempfile
import time

phase = sys.argv[1]
if phase not in ("before", "after"):
    raise SystemExit("Usage: screenshot-home.py before|after")
root = pathlib.Path(__file__).resolve().parents[3]
output = root / "qa" / "home-pass"
output.mkdir(parents=True, exist_ok=True)
chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
for fleet in ("ordinary", "rich"):
    sizes = ((1440, 900), (1040, 680), (2000, 1000)) if fleet == "ordinary" else ((1440, 900), (1040, 680))
    for theme in ("light", "dark"):
        for width, height in sizes:
            path = output / f"{phase}-{fleet}-{theme}-{width}x{height}.png"
            path.unlink(missing_ok=True)
            with tempfile.TemporaryDirectory(prefix="repomon-home-shot-") as profile:
                process = subprocess.Popen([
                    chrome, "--headless", "--disable-gpu", "--no-first-run",
                    "--no-default-browser-check", f"--user-data-dir={profile}",
                    "--hide-scrollbars", f"--window-size={width},{height}",
                    "--force-device-scale-factor=1", "--virtual-time-budget=4000",
                    f"--screenshot={path}",
                    f"http://127.0.0.1:4178/?home=1&fleet={fleet}&theme={theme}",
                ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
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
                    # Chrome can linger after writing the PNG. Reap only this owned process.
                    if process.poll() is None:
                        process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
                print(path, flush=True)
