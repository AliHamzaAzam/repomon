"""Capture the copied app's WebKit console and DOM without Accessibility or screen capture."""
import json
from pathlib import Path
import shutil
import subprocess as sp
import time


TITLES = {"Repomind", "orbit-api", "feat-rate-limit-headers", "meadow-web",
          "fix-nav-focus-trap", "forge-cli", "fix-windows-console", "atlas-docs"}


def launch_command(root, helpers, executable, tour=False):
    library = root / "bin/webview-probe.dylib"
    sp.run(["xcrun", "clang", "-dynamiclib", "-fobjc-arc", "-framework", "Cocoa",
            "-framework", "WebKit", str(helpers / "webview_probe.m"), "-o", str(library)], check=True)
    script = root / "bin/webview-probe.js"
    shutil.copy2(helpers / "webview_probe.js", script)
    if tour:
        with script.open('a') as output:
            output.write('\n' + (helpers / 'webview_tour.js').read_text())
            output.write('\n' + (helpers / 'workflow_tour.js').read_text())
    # Set DYLD variables after sandbox-exec: macOS strips them at protected system executables.
    return ["/usr/bin/env", f"DYLD_INSERT_LIBRARIES={library}",
            f"REPOMON_WEBVIEW_SCRIPT={script}", f"REPOMON_WEBVIEW_LOG={root}/out/webview.jsonl",
            *([f"REPOMON_WEBVIEW_TOUR_COMMAND={root}/out/tour-command"] if tour else []),
            str(executable)]


def run_tour(root, phase):
    command = root / 'out/tour-command'
    path = root / 'out/webview.jsonl'
    seen = len(path.read_text().splitlines()) if path.exists() else 0
    command.write_text(phase)
    started = time.monotonic()
    with (root / 'out/tour.log').open('a') as log:
        while time.monotonic() - started < 300:
            path = root / 'out/webview.jsonl'
            lines = path.read_text().splitlines() if path.exists() else []
            for line in lines[seen:]:
                try:
                    row = json.loads(line)
                except json.JSONDecodeError:
                    break
                seen += 1
                body = row.get('body', row)
                if body.get('phase') != phase or not body.get('event', '').startswith('tour-'):
                    continue
                text = f"[webview {phase} +{time.monotonic() - started:.2f}s] {json.dumps(body)}"
                print(text, flush=True)
                log.write(text + '\n')
                log.flush()
                if body['event'] == 'tour-complete':
                    return
                if body['event'] == 'tour-error':
                    raise RuntimeError(f"WebKit tour failed: {body}; see {root}/out/webview.jsonl")
            time.sleep(0.2)
    raise RuntimeError(f"WebKit tour {phase} timed out; see {root}/out/webview.jsonl")


def rows(root, pid):
    path = root / "out/webview.jsonl"
    result = []
    if path.exists():
        for line in path.read_text().splitlines():
            try:
                row = json.loads(line)
                if row.get("pid") == pid:
                    result.append(row)
            except json.JSONDecodeError:
                pass  # The app may still be appending the last record.
    return result


def evidence(root, pid):
    records = rows(root, pid)
    native = next((r for r in records if r["event"] == "webview-init"), None)
    if not native:
        return None
    expected_home = (root / "cocoa").resolve()
    assert Path(native["home"]).resolve() == expected_home, native
    assert native["library"] and all(Path(p).resolve().is_relative_to(expected_home) for p in native["library"]), native
    messages = [r["body"] for r in records if r["event"] == "javascript"]
    errors = [r for r in messages if r["event"] in ("error", "unhandledrejection", "storage-error")]
    assert not errors, f"WebKit errors: {errors}; see {root}/out/webview.jsonl"
    storage = {r["event"]: r.get("value") for r in messages if r["event"] in ("localStorage", "indexedDB")}
    snapshots = [r for r in messages if r["event"] == "dom"]
    if storage != {"localStorage": "ok", "indexedDB": "ok"} or not snapshots:
        return None
    dom = snapshots[-1]
    visible = [b for b in dom["buttons"] if b["shown"]]
    titles = {b["text"].split("\n")[0] for b in visible}
    chips = {b["text"].split("\n")[0]: b["text"].split("\n")[-1] for b in visible
             if b["text"].split("\n")[0] in ("Needs you", "Running")}
    if not TITLES <= titles or not all(chips.get(k, "0").isdigit() and int(chips.get(k, "0")) > 0 for k in ("Needs you", "Running")):
        return None
    return {"app_pid": pid, "cocoa_home": native["home"], "library": native["library"],
            "storage": storage, "lane_buttons": sorted(TITLES), "chips": chips,
            "viewport": dom["viewport"], "errors": errors,
            "console_log": str(root / "out/webview.jsonl"),
            "limitation": "DOM text and computed layout verified; native AX exposure and screen pixels are not tested."}


def verify(root, app):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if app.poll() is not None:
            raise RuntimeError(f"Demo app exited; see {root}/out/app.log")
        result = evidence(root, app.pid)
        if result:
            (root / "out/webview-check.json").write_text(json.dumps(result, indent=2))
            return result
        time.sleep(1)
    raise RuntimeError(f"WebKit storage/DOM probe incomplete after 30 s; see {root}/out/webview.jsonl and app.log. The supplied app may reject diagnostic library loading.")


def collect_denials(root, pid):
    # This is read-only, scoped to this app PID/root. Empty output is not proof of no denials.
    predicate = ('(process == "sandboxd" OR process == "kernel") AND '
                 f'(eventMessage CONTAINS "repomon-desktop({pid})" OR '
                 f'eventMessage CONTAINS "{root}")')
    path = root / "out/sandbox-denials.json"
    try:
        result = sp.run(["/usr/bin/log", "show", "--last", "10m", "--style", "json", "--predicate", predicate],
                        capture_output=True, text=True, timeout=15)
        path.write_text(result.stdout)
        status = {"exit_code": result.returncode, "stderr": result.stderr,
                  "predicate": predicate, "log": str(path),
                  "limitation": "macOS may omit sandbox reports; an empty log does not prove no access was denied."}
    except (OSError, sp.TimeoutExpired) as error:
        status = {"error": str(error), "log": str(path)}
    (root / "out/sandbox-denials-status.json").write_text(json.dumps(status, indent=2))
    return status
