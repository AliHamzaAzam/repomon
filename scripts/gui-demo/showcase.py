#!/usr/bin/env python3
"""Operator-run macOS showcase recorder. All mutations stay in the disposable fleet."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import pwd
import shutil
import signal
import subprocess as sp
import sys
import tempfile
import time

from fixtures import git, seed, seed_usage, write
from rpc import call
from connection_probe import DesktopProbe
import webview_probe

HELPERS = Path(__file__).resolve().parent
REPO = HELPERS.parent.parent


def log(message):
    print(f"[gui-demo] {message}", flush=True)


def run(args, **kwargs):
    return sp.run([str(a) for a in args], check=True, **kwargs)


def production_pids():
    # Read process metadata only. Never connect to the production daemon.
    output = sp.check_output(["/bin/ps", "-axo", "pid=,command="], text=True)
    return sorted(line.split(None, 1)[0] for line in output.splitlines()
                  if "repomond" in line and "repomon-gui-demo." not in line
                  and "--socket" in line and "/tmp/repomon-" in line)


def daemon_pids():
    output = sp.check_output(["/bin/ps", "-axo", "pid=,comm="], text=True)
    return sorted(int(line.split(None, 1)[0]) for line in output.splitlines()
                  if Path(line.split(None, 1)[1]).name == "repomond")


def verify_desktop(root, app, probe, baseline_pids):
    lanes = call(root, "lane.list")
    def connected():
        if app.poll() is not None:
            raise AssertionError(f"Demo app exited: see {root}/out/app.log")
        evidence = probe.evidence(lanes)
        if evidence:
            assert evidence['app_pid'] == app.pid
            return evidence
        return None
    evidence = eventually(connected, "desktop fetched all 5 repos and 8 lanes from the seeded daemon and selected a viewport", 30)
    after = daemon_pids()
    assert after == baseline_pids, f"repomond PID set changed during app launch: {baseline_pids} to {after}"
    evidence['repomond_pids_before_app'] = baseline_pids
    evidence['repomond_pids_after_app'] = after
    # Existing desktop binaries emit no successful-endpoint log. Inspect their native launch
    # log if one exists; distinguish it from our wire-level observer rather than inventing one.
    native_log = root / "data/logs/repomond.out.log"
    evidence['native_launch_log'] = str(native_log) if native_log.exists() else None
    evidence['native_launch_log_tail'] = native_log.read_text(errors='replace')[-4096:] if native_log.exists() else "No native launch log: the app did not launch a daemon."
    evidence['observed_rpc_log'] = str(probe.log_path)
    write(root / "out/desktop-connection.json", json.dumps(evidence, indent=2))
    log(f"PASS kernel peers: desktop {app.pid} via {probe.endpoint} to daemon {probe.daemon_pid} at {probe.backend}")
    log(f"PASS no second repomond: {baseline_pids} unchanged")
    log(f"Desktop endpoint evidence: {probe.log_path}; native launch log: {evidence['native_launch_log'] or 'absent'}")
    return evidence


def eventually(check, label, seconds=60):
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
        try:
            result = check()
            if result:
                log(f"PASS {label}")
                return result
        except (OSError, RuntimeError, KeyError) as error:
            last = error
        time.sleep(1)
    raise RuntimeError(f"Timed out: {label}; last error: {last}")


def prepare(root, bin_dir):
    for directory in ("bin", "data", "config/repomon", "cocoa", "cache", "xdg-data", "memory", "mail", "out", "claude"):
        (root / directory).mkdir(parents=True)
    # Cocoa resolves NSHomeDirectory through CFFIXED_USER_HOME. All Library stores are
    # inside the allowed disposable root, including WebKit data and container/cache paths.
    for directory in ("WebKit", "Caches", "Containers", "Application Support", "Preferences"):
        (root / "cocoa/Library" / directory).mkdir(parents=True)
    for name in ("repomond", "repomon-desktop", "repomon"):
        shutil.copy2(bin_dir / name, root / "bin" / name)
    for name in ("fake_agent.py", "rpc.py"):
        shutil.copy2(HELPERS / name, root / "bin" / name)
    (root / "bin" / "fake_agent.py").chmod(0o755)
    for name in ("claude", "codex", "agy", "opencode", "cursor-agent", "aider"):
        (root / "bin" / name).symlink_to("fake_agent.py")
    write(root / "bin" / "basic-memory", "#!/bin/sh\n# Demo export stays local; never starts a memory service.\nexit 0\n")
    (root / "bin" / "basic-memory").chmod(0o755)
    seed(root, REPO)
    personal_home = pwd.getpwuid(os.getuid()).pw_dir
    # No HOME/CODEX_HOME reassignment. Deny real home access at the OS boundary instead.
    write(root / "guard.sb", f'''(version 1)
(allow default)
(deny file-read* file-write* (subpath {json.dumps(personal_home)}))
(deny network-outbound (remote ip "*:*"))
''')
    write(root / "app-guard.sb", (root / "guard.sb").read_text() +
          f'(deny process-exec (literal {json.dumps(str(root / "bin/repomond"))}))\n')
    label = root.name.replace(".", "-")
    config = f'''socket_path = "{root}/app.sock"
theme = "dark"
accent = "brand"
tmux_session = "{label}"
auto_continue = true
notify_sound_needs_you = false
notify_sound_repomind_needs_you = false
[repomind]
home = "{root}/repomind"
basic_memory_config_dir = "{root}/memory"
[supervision]
enabled = true
[supervision.classes]
command_exec = "hold"
file_write = "hold"
push_remote = "hold"
[agents]
'''
    actors = [("hero", "claude", "hero"), ("permission", "claude", "permission"),
              ("api", "codex", "running"), ("web", "cursor-agent", "running"),
              ("console", "aider", "running"), ("encoding", "opencode", "running"),
              ("docs", "agy", "limited")]
    for actor, command, state in actors:
        config += f'{actor} = "REPOMON_DEMO_ACTOR={actor} REPOMON_DEMO_STATE={state} {root}/bin/{command}"\n'
    write(root / "config/repomon/config.toml", config)


def environment(root):
    return {
        "PATH": f"{root}/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "USER": pwd.getpwuid(os.getuid()).pw_name, "LOGNAME": pwd.getpwuid(os.getuid()).pw_name,
        "LANG": "en_US.UTF-8", "TERM": "xterm-256color", "SHELL": "/bin/sh",
        "TMUX_TMPDIR": str(root / "cache"), "TMPDIR": str(root / "cache"), "CFFIXED_USER_HOME": str(root / "cocoa"),
        "XDG_CONFIG_HOME": str(root / "config"), "XDG_CACHE_HOME": str(root / "cache"),
        "XDG_DATA_HOME": str(root / "xdg-data"), "REPOMON_DATA_DIR": str(root / "data"),
        "REPOMON_SOCKET": str(root / "demo.sock"), "REPOMON_MCP_SOCKET": str(root / "demo.sock"),
        "REPOMON_DEMO_ROOT": str(root), "BASIC_MEMORY_CONFIG_DIR": str(root / "memory"),
        "CLAUDE_CONFIG_DIR": str(root / "claude"),
        "REPOMON_CLAUDE_PROJECTS": str(root / "ledger/claude"),
        "REPOMON_CODEX_SESSIONS": str(root / "ledger/codex"),
        "REPOMON_ANTIGRAVITY_CACHE": str(root / "ledger/agy/cache/last_conversations.json"),
        "REPOMON_OPENCODE_DB": str(root / "ledger/opencode.db"),
        "REPOMON_ANTIGRAVITY_MCP_CONFIG": str(root / "config/agy-mcp.json"),
        "REPOMON_CURSOR_MCP_CONFIG": str(root / "config/cursor-mcp.json"),
        "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1",
        "PYTHONDONTWRITEBYTECODE": "1",
    }


def fleet(root):
    rpc = lambda method, **params: call(root, method, params)
    repos = {name: rpc("repo.add", path=str(root / "repos" / name))["id"]
             for name in ("orbit-api", "meadow-web", "forge-cli", "atlas-docs")}
    branches = {}
    for name, branch, actor in [("orbit-api", "feat/rate-limit-headers", "permission"),
                                 ("meadow-web", "fix/nav-focus-trap", "hero"),
                                 ("forge-cli", "fix/windows-console", "encoding")]:
        path = root / "worktrees" / branch.replace("/", "-")
        lane = rpc("lane.create", repo_id=repos[name], branch=branch, path=str(path))
        branches[actor] = lane["id"]
    lanes = rpc("lane.list")
    controller = next(lane for lane in lanes if lane.get("role") == "controller")
    rpc("agent.pin", lane_id=controller["id"], pinned=True)
    roots = {lane["repo"]["name"]: lane["id"] for lane in lanes if lane["worktree"]["branch"] == "main"}
    assignments = {**branches, "api": roots["orbit-api"], "web": roots["meadow-web"],
                   "console": roots["forge-cli"], "docs": roots["atlas-docs"]}
    hero = root / "worktrees/fix-nav-focus-trap"
    write(hero / "src/hooks/useMediaQuery.ts", 'export const isCompact = () => matchMedia("(max-width: 640px)").matches;\n')
    git(hero, "add", "--", "src/hooks/useMediaQuery.ts")
    git(hero, "commit", "-qm", "feat: detect compact navigation layouts")
    menu = hero / "src/components/MobileMenu.tsx"
    menu.write_text(menu.read_text().replace('  const [open, setOpen] = useState(false);',
        '  const [open, setOpen] = useState(false);\n  // Restore focus to the trigger after closing the menu.'))
    write(hero / "src/components/useFocusTrap.ts", 'export function useFocusTrap(root: HTMLElement) {\n  root.querySelector<HTMLElement>("a, button")?.focus();\n}\n')
    write(root / "worktrees/feat-rate-limit-headers/src/routes/headers.ts", 'export const REMAINING_HEADER = "X-RateLimit-Remaining";\n')
    seed_usage(root, REPO, [root / "repos/orbit-api", hero, root / "repos/forge-cli"])
    rpc("playbook.save", name="release-review", content="# Release review\n\n1. Read the lane diff.\n2. Check tests and usage.\n3. Ask the operator before publishing.\n")
    rpc("supervision.set", lane_id=assignments["permission"], enabled=True, classes={"command_exec": "hold", "push_remote": "hold"})
    for actor, lane in assignments.items():
        rpc("agent.spawn", lane_id=lane, agent=actor)
    rpc("agent.pin", lane_id=assignments["hero"], pinned=True)
    for sender, recipient, body in [("api", "hero", "Rate-limit headers are ready. Please review the API contract before wiring the menu state."),
                                    ("hero", "api", "Navigation coverage is green. The focus trap is ready for review in the Git panel."),
                                    ("encoding", "hero", "Windows console checks passed. UTF-8 output now matches macOS; release checklist is updated.")]:
        # First managed window in a lane is /1. The receiving identity is validated by message.send.
        write(root / "mail" / f"{sender}.json", json.dumps({"to": f"lane-{assignments[recipient]}/1", "body": body}))
    write(root / "out/assignments.json", json.dumps(assignments, indent=2))
    return assignments


def verify(root, assignments):
    expected = {"hero": ("claude-code", "idle"), "permission": ("claude-code", "waiting"),
                "api": ("codex", "running"), "web": ("cursor", "running"),
                "console": ("aider", "running"), "encoding": ("opencode", "running"),
                "docs": ("antigravity", "rate-limited")}

    def classified():
        lanes = call(root, "lane.list")
        write(root / "out/lane.list.json", json.dumps(lanes, indent=2))
        if len(lanes) != 8 or len({lane["repo"]["id"] for lane in lanes}) != 5:
            return False
        by_id = {lane["id"]: lane for lane in lanes}
        for actor, (kind, status) in expected.items():
            sessions = by_id[assignments[actor]]["agent_sessions"]
            if not sessions or sessions[0]["agent"] != kind or sessions[0]["status"] != status:
                return False
        return lanes

    lanes = eventually(classified, "lane.list: 5 repos, 8 lanes, 6 kinds and all four statuses")
    for lane in lanes:
        sessions = lane["agent_sessions"]
        agent = sessions[0] if sessions else {"agent": "none", "status": "idle"}
        log(f"lane-{lane['id']} | {lane['repo']['name']} / {lane['worktree']['branch']} | {agent['agent']} | {agent['status']}")
    diff = call(root, "lane.diff", {"lane_id": assignments["hero"], "include_patch": True})
    assert "Restore focus" in diff["patch"] and diff["untracked"] > 0
    write(root / "out/lane.diff.json", json.dumps(diff, indent=2))
    log("PASS lane.diff: committed history, dirty TypeScript patch and untracked hook")
    call(root, "usage.ingest_now")
    summary = call(root, "usage.summary", {"range": "week"})
    write(root / "out/usage.summary.json", json.dumps(summary, indent=2))
    log("usage.summary: " + json.dumps(summary))
    sessions = call(root, "usage.sessions", {"range": "week", "limit": 100})
    write(root / "out/usage.sessions.json", json.dumps(sessions, indent=2))
    assert len(sessions) == 42, f"Expected 42 synthetic sessions, got {len(sessions)}"
    assert summary["totals"]["total_tokens"] > 0 and summary["totals"]["cost_usd"] > 0
    assert len(summary["groups"]) == 2
    timeline = call(root, "usage.timeline", {"range": "week", "bucket": "day"})
    write(root / "out/usage.timeline.json", json.dumps(timeline, indent=2))
    assert len(timeline["series"]) == 2
    assert all(len(series["points"]) >= 7 for series in timeline["series"])
    assert len({point["total_tokens"] for point in timeline["series"][0]["points"]}) > 1
    log("PASS usage.summary: priced Claude and Codex totals; 42 synthetic sessions; varied seven-day chart")
    eventually(lambda: len(list((root / "mail").glob("*.sent"))) == 3, "three authenticated messages between fake lanes")
    audit = eventually(lambda: (rows if len(rows := call(root, "supervision.audit", {"lane_id": assignments['permission']})["entries"]) >= 2 else None), "two supervision hold audit rows")
    assert all(row["decision"] == "hold" for row in audit)
    write(root / "out/supervision.audit.json", json.dumps(audit, indent=2))
    mind = call(root, "repomind.status")
    write(root / "out/repomind.status.json", json.dumps(mind, indent=2))
    assert mind["home"] == str(root / "repomind"), mind
    assert mind["counts"]["active_plans"] == 2 and mind["counts"]["drafts"] == 1
    assert len(call(root, "playbook.list")) == 1
    assert any(lane.get("role") == "controller" and lane["pinned"] for lane in lanes)
    log("PASS throwaway Repomind home, two plans, one playbook draft, pinned controller")


def run_tour(root, tour, phase, children):
    """Tee AppleScript diagnostics to the terminal and retained sandbox evidence."""
    with (root / "out/tour.log").open("a") as tour_log:
        log(f"Starting AX phase: {phase}; log: {root}/out/tour.log")
        process = sp.Popen([str(arg) for arg in tour + [phase, root / "out"]],
                           cwd=root, stdout=sp.PIPE, stderr=sp.STDOUT, text=True, bufsize=1)
        children.append(process)
        for line in process.stdout:
            print(line, end="", flush=True)
            tour_log.write(line)
            tour_log.flush()
        code = process.wait()
        if code:
            raise RuntimeError(f"AX phase {phase} failed ({code}); see {root}/out/tour.log "
                               f"and {root}/out/ax-dump.txt")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dry-run", action="store_true", help="verify, launch, wait, clean up; never capture")
    parser.add_argument("--tour", action="store_true", help="also rehearse the tour during --dry-run")
    parser.add_argument("--still", action="store_true", help="capture the GIF opening hero frame")
    parser.add_argument("--keep-sandbox", action="store_true", help="keep fixtures and validation logs after stopping all demo processes")
    parser.add_argument("--diagnose-webview", action="store_true", help="compile a recorder-only console/DOM probe; verify WebKit storage without AX or capture")
    parser.add_argument("--no-guard", action="store_true", help="A/B only: disable the desktop OS guard; daemon and fake-agent guards remain active")
    parser.add_argument("--skip-build", action="store_true", help="compatibility flag; this script never builds the app")
    parser.add_argument("--bin-dir", type=Path, default=REPO / "target/release", help="directory containing matching repomond, repomon-desktop and repomon binaries")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("window recording and the tour require macOS")
    if args.still and args.dry_run:
        parser.error("--still and --dry-run are mutually exclusive")
    for name in ("repomond", "repomon-desktop", "repomon"):
        if not (args.bin_dir / name).is_file():
            parser.error(f"Missing {name}; pass --bin-dir with existing matching binaries. No builds are run.")
    for tool in ("tmux", "git", "python3") + (("xcrun",) if args.diagnose_webview else ()) + (() if args.dry_run else ("ffmpeg", "ffprobe", "swift")):
        if not shutil.which(tool):
            parser.error(f"Missing tool: {tool}")
    before = production_pids()
    log(f"Production daemon PIDs before: {before}")
    root = Path(tempfile.mkdtemp(prefix="repomon-gui-demo.", dir="/private/tmp")).resolve()
    log(f"Sandbox: {root}")
    children = []
    label = root.name.replace(".", "-")
    env = None
    completed = False
    desktop_probe = None
    app = None
    try:
        prepare(root, args.bin_dir.resolve())
        env = environment(root)
        guard = ["/usr/bin/sandbox-exec", "-f", root / "guard.sb"]
        # Prove both deny rules before launching a daemon. No real file contents are read.
        probe = """import errno, os, pwd, socket
try:
    os.listdir(pwd.getpwuid(os.getuid()).pw_dir)
except PermissionError:
    pass
else:
    raise SystemExit("Real home deny guard is not active")
with socket.socket() as client:
    assert client.connect_ex(("127.0.0.1", 9)) == errno.EPERM, "Network deny guard is not active"
"""
        run(guard + [sys.executable, "-c", probe], env=env, cwd=root)
        log("PASS OS guard denies real home reads and IP network connections")
        run(guard + ["tmux", "-L", label, "-f", "/dev/null", "new-session", "-d", "-s", label, "/bin/sh"], env=env, cwd=root)
        run(["tmux", "-L", label, "set-option", "-g", "default-shell", "/bin/sh"], env=env)
        daemon_log = (root / "out/daemon.log").open("w")
        daemon = sp.Popen([str(a) for a in guard + [root / "bin/repomond", "--socket", root / "demo.sock"]], env=env, cwd=root, stdout=daemon_log, stderr=sp.STDOUT)
        children.append(daemon)
        eventually(lambda: (root / "demo.sock").is_socket(), "private daemon socket", 20)
        assignments = fleet(root)
        verify(root, assignments)
        app_guard = ["/usr/bin/sandbox-exec", "-f", root / "app-guard.sb"]
        spawn_probe = """import subprocess, sys
try:
    subprocess.run([sys.argv[1], "--version"], check=True, capture_output=True)
except PermissionError:
    pass
else:
    raise SystemExit("Desktop guard allowed a second repomond execution")
"""
        if args.no_guard:
            app_guard = []
            log("A/B mode: desktop OS guard disabled; disposable environment and daemon guard retained")
        else:
            run(app_guard + [sys.executable, "-c", spawn_probe, root / "bin/repomond"], env=env, cwd=root)
            log("PASS desktop guard denies spawning repomond")
        desktop_probe = DesktopProbe(root, daemon.pid)
        app_env = {**env, "REPOMON_SOCKET": str(desktop_probe.endpoint)}
        baseline_pids = daemon_pids()
        write(root / "out/launch.json", json.dumps({
            "app_endpoint": app_env["REPOMON_SOCKET"], "daemon_endpoint": str(root / "demo.sock"),
            "config": str(root / "config/repomon/config.toml"),
            "desktop_guard": not args.no_guard, "diagnose_webview": args.diagnose_webview,
            "cocoa_home": env["CFFIXED_USER_HOME"],
            "binary_sha256": {name: hashlib.sha256((root / "bin" / name).read_bytes()).hexdigest()
                              for name in ("repomon-desktop", "repomond")},
        }, indent=2))
        command = webview_probe.launch_command(root, HELPERS) if args.diagnose_webview else [root / "bin/repomon-desktop"]
        app_log = (root / "out/app.log").open("w")
        app = sp.Popen([str(a) for a in app_guard + command], env=app_env, cwd=root, stdout=app_log, stderr=sp.STDOUT)
        children.append(app)
        desktop_probe.expect_app(app.pid)
        verify_desktop(root, app, desktop_probe, baseline_pids)
        log(f"PASS isolated app launched (PID {app.pid})")
        if args.diagnose_webview:
            result = webview_probe.verify(root, app)
            log(f"PASS WebKit localStorage and IndexedDB; Cocoa Library: {result['library']}")
            log(f"PASS WebKit DOM: 8 rendered lane buttons; chips {result['chips']}; see {root}/out/webview-check.json")
        if args.dry_run and not args.tour:
            log("Dry run complete. No screen capture or permission probe was performed.")
            completed = True
            return
        tour = ["osascript", HELPERS / "tour.applescript", str(app.pid)]
        run_tour(root, tour, "opening", children)
        if args.dry_run:
            run_tour(root, tour, "tour", children)
            log("PASS tour rehearsal completed without screen capture")
            completed = True
            return
        window_id = sp.check_output(["swift", str(HELPERS / "window.swift"), str(app.pid)], text=True).strip()
        assert window_id.isdecimal(), "No unique demo window ID; refusing capture"
        capture(root, window_id, tour, args.still, children)
        completed = True
    finally:
        if args.diagnose_webview and app:
            try:
                status = webview_probe.collect_denials(root, app.pid)
                log(f"Sandbox denial query exit: {status.get('exit_code', 'unavailable')}; see {root}/out/sandbox-denials-status.json")
            except OSError as error:
                log(f"Could not retain denial-query diagnostics: {error}")
            log(f"WebKit console and DOM: {root}/out/webview.jsonl")
        for child in reversed(children):
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=5)
                except sp.TimeoutExpired:
                    child.kill()
                    child.wait()
        if desktop_probe:
            desktop_probe.close()
        if env:
            sp.run(["tmux", "-L", label, "kill-server"], env=env, stdout=sp.DEVNULL, stderr=sp.DEVNULL)
        (root / "demo.sock").unlink(missing_ok=True)
        after = production_pids()
        log(f"Production daemon PIDs after: {after}")
        if before != after:
            log("WARNING: production daemon PID set changed externally during the run")
        else:
            log("PASS production daemon PID set unchanged")
        if args.keep_sandbox or not completed:
            log(f"Retained fixtures and logs: {root} (demo processes stopped)")
        else:
            shutil.rmtree(root)


def capture(root, window_id, tour, still, children):
    raw = root / "out" / ("window.png" if still else "window.mov")
    destination = REPO / "docs" / ("preview.png" if still else "gui-demo.gif")
    # Window-ID-only input. Normalize Retina scale, then take the top-left content rectangle.
    # There is deliberately no whole-display fallback on a permission or window failure.
    content = "scale=1440:-1:flags=lanczos,setsar=1"
    if still:
        run(["screencapture", "-x", "-o", "-l", window_id, raw])
        run(["ffmpeg", "-y", "-v", "error", "-i", raw, "-vf", content, "-frames:v", "1", root / "out/content.png"])
        shutil.copy2(root / "out/content.png", destination)
    else:
        recorder = sp.Popen(["screencapture", "-v", "-V", "94", "-x", "-o", "-l", window_id, str(raw)])
        children.append(recorder)
        time.sleep(2)
        if recorder.poll() is not None:
            raise RuntimeError("Window capture failed. Run from a terminal with Screen Recording permission.")
        run_tour(root, tour, "tour", children)
        recorder.wait(timeout=20)
        if recorder.returncode:
            raise RuntimeError("Window recording failed")
        candidate = root / "out/showcase.gif"
        for fps, colors in ((12, 256), (10, 256), (10, 128), (10, 96)):
            graph = f"{content},fps={fps},scale=1200:750:flags=lanczos,split[a][b];[a]palettegen=max_colors={colors}:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle"
            run(["ffmpeg", "-y", "-v", "error", "-i", raw, "-filter_complex", graph, "-loop", "0", candidate])
            log(f"GIF candidate: {fps} fps, {colors} colors, {candidate.stat().st_size / 1_000_000:.2f} MB")
            if candidate.stat().st_size < 15_000_000:
                shutil.copy2(candidate, destination)
                break
        else:
            raise RuntimeError(f"GIF exceeds 15 MB; recording retained at {raw} for tuning.")
    log(f"Wrote {destination} ({destination.stat().st_size / 1_000_000:.2f} MB)")


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    try:
        main()
    except (RuntimeError, AssertionError, sp.CalledProcessError) as error:
        log(f"FAILED: {error}")
        sys.exit(1)
