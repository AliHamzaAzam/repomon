#!/usr/bin/env python3
"""Terminal-only actors. Only the workflow permission actor accepts Enter; no real tool is invoked."""
import datetime as dt
import json
import os
from pathlib import Path
import sys
import select
import tty
import time

from rpc import call

root = Path(os.environ["REPOMON_DEMO_ROOT"])
kind = Path(sys.argv[0]).name
if "--version" in sys.argv or "--help" in sys.argv:
    print(f"{kind} 1.0.0 (Repomon demo mock)")
    sys.exit(0)
state = os.environ.get("REPOMON_DEMO_STATE", "running")
started = time.monotonic()
initial_state = state
actor = os.environ['REPOMON_DEMO_ACTOR']
if sys.stdin.isatty():
    tty.setcbreak(sys.stdin.fileno())
count = 0
sent = False
workflow_history = False
sys.stdout.write("\033[?1049h")
while True:
    # The token stays in memory and is sent only to the sandbox socket. Never log it.
    outbox = root / "mail" / f"{os.environ['REPOMON_DEMO_ACTOR']}.json"
    if not sent and outbox.exists():
        message = json.loads(outbox.read_text())
        message["identity_token"] = os.environ["REPOMON_MCP_IDENTITY_TOKEN"]
        receipt = call(root, "message.send", message)
        (outbox.with_suffix(".sent")).write_text(receipt["id"])
        sent = True
    workflow_file = root / "out/workflow-state.json"
    if workflow_file.exists():
        state = json.loads(workflow_file.read_text()).get(actor, initial_state)
        if actor == "permission" and state == "permission" and select.select([sys.stdin], [], [], 0)[0]:
            received = os.read(sys.stdin.fileno(), 128)
            if received and received.strip(b"\r\n") == b"":
                (root / "out/workflow-answer.txt").write_text("Accepted by terminal Enter; no command executed\n")
        if actor == "permission" and (root / "out/workflow-answer.txt").exists():
            state = "running"
    count += 1
    title = {"claude": "Claude Code", "codex": "OpenAI Codex", "agy": "Antigravity",
             "opencode": "OpenCode", "cursor-agent": "Cursor Agent", "aider": "aider"}[kind]
    screen = f"\033[2J\033[H\033[38;5;180m{title}\033[0m  |  Demo session\n\n"
    if state == "hero":
        screen += ("Welcome back, Morgan\n\n"
                   "  meadow-web / fix/nav-focus-trap\n"
                   "  Accessible navigation for the next release\n\n"
                   "  Updated MobileMenu.tsx and added useFocusTrap.ts\n"
                   "  Verified keyboard traversal and focus restoration\n"
                   "  24 tests passed. Ready for your review.\n\n"
                   "  > \n")
    elif state == "permission":
        # Different question summaries yield two genuine hold audit rows.
        question = "Do you want to proceed?" if time.monotonic() - started < 12 else "Do you want to run this command?"
        screen += ("Bash command\n\n  git push origin feat/rate-limit-headers\n"
                   "  Publish the reviewed rate-limit headers\n\n"
                   f"{question}\n"
                   "❯ 1. Yes\n  2. Yes, and don't ask again for git push\n"
                   "  3. No, and tell Claude what to do\n")
    elif state == "limited":
        screen += ("Reviewed 8 documentation pages.\n\n"
                   f"Usage limit reached. Your limit will reset at {(dt.datetime.now() + dt.timedelta(hours=2)).strftime('%H:%M')}.\n"
                   "Waiting for the next quota window.\n")
    else:
        tasks = {"codex": ("Inspecting request middleware", "src/routes/rateLimit.ts", "18 checks passed"),
                 "cursor-agent": ("Polishing responsive navigation", "src/components/MobileMenu.tsx", "24 checks passed"),
                 "aider": ("Testing console output on Windows", "src/main.rs", "12 checks passed"),
                 "opencode": ("Checking UTF-8 console rendering", "src/main.rs", "9 checks passed")}
        task, file, tests = tasks.get(kind, ("Reviewing changes", "README.md", "12 checks passed"))
        screen += (f"{task}\n\n  Read {file}\n  + Add regression coverage\n"
                   f"  {tests}\n\nWorking... pass {count:04d} \nesc to cancel\n")
    if workflow_file.exists() and state != "permission":
        screen += "\nSession retained: workflow-demo-local\n"
        if actor == "permission" and (root / "out/workflow-answer.txt").exists():
            screen += "Permission answered. Continuing the demo task.\n"
    if workflow_file.exists() and actor == "hero":
        # Leave the full showcase's alternate screen once, then accumulate real tmux
        # scrollback. The unique earlier line must survive the desktop restart.
        if not workflow_history:
            screen = "\033[?1049l\033[2J\033[H" + screen.replace("\033[2J\033[H", "")
            screen += "Earlier result: navigation regression tests passed before app restart.\n"
            workflow_history = True
        else:
            screen = f"Working... review pass {count:04d} (esc to interrupt)\n"
    sys.stdout.write(screen)
    sys.stdout.flush()
    time.sleep(0.7)
