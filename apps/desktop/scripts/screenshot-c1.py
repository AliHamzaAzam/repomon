"""Capture C1's real app fixtures with the existing isolated headless Chrome harness.

Run vite.screenshot.config.ts on localhost:4178 first. Screenshots stay ignored in qa/.
An optional scenario argument captures just that scenario for iteration.
"""
import pathlib
import subprocess
import sys
import tempfile
import time

root = pathlib.Path(__file__).resolve().parents[3]
output = root / "qa" / "design-round4"
output.mkdir(parents=True, exist_ok=True)
chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
scenarios = {
    "home-operator": "home=1&fleet=operator",
    "home-real": "home=1&fleet=real",
    "home-raw": "home=1&fleet=raw",
    "home-ordinary": "home=1&fleet=ordinary",
    "home-rich": "home=1&fleet=rich",
    "conversation-operator": "surface=conversation&case=operator",
    "conversation-rich": "surface=conversation&case=rich",
    "conversation-focused": "surface=conversation&case=dull&focus=reply",
    "conversation-diff": "surface=conversation&case=diff",
    "conversation-streaming": "surface=conversation&case=streaming",
    "conversation-dull": "surface=conversation&case=dull",
    "conversation-broken": "surface=conversation&case=broken",
    "conversation-no-source": "surface=conversation&case=no-source",
    "conversation-dialog": "surface=conversation&case=dialog",
    "conversation-attachments": "surface=conversation&case=attachments",
    "conversation-notices": "surface=conversation&case=notices",
    "settings-notices": "surface=settings&home=1&fleet=real&notices=1",
    "settings-agents": "surface=settings&home=1&fleet=real",
}
if len(sys.argv) > 1:
    scenarios = {name: scenarios[name] for name in sys.argv[1:]}
for name, query in scenarios.items():
    for theme in ("light", "dark"):
        for width, height in ((1440, 900), (1040, 680), (2000, 1000)):
            path = output / f"{name}-{theme}-{width}x{height}.png"
            path.unlink(missing_ok=True)
            with tempfile.TemporaryDirectory(prefix="repomon-c1-shot-") as profile:
                process = subprocess.Popen([
                    chrome, "--headless", "--disable-gpu", "--no-first-run",
                    "--no-default-browser-check", f"--user-data-dir={profile}",
                    "--hide-scrollbars", f"--window-size={width},{height}",
                    "--force-device-scale-factor=1", "--virtual-time-budget=5000",
                    f"--screenshot={path}", f"http://127.0.0.1:4178/?{query}&theme={theme}",
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
                    if process.poll() is None:
                        process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
                print(path, flush=True)
