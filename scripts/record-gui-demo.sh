#!/usr/bin/env bash
# scripts/record-gui-demo.sh - records docs/gui-demo.gif, the desktop app's hero GIF.
#
#   Usage:  scripts/record-gui-demo.sh [--keep-sandbox] [--skip-build] [--still]
#
#   --still captures one PNG of the hero frame (the same sandbox fleet, before the tour) to
#   docs/preview.png instead of recording the GIF, then cleans up as usual.
#
# What it does, in order:
#   1. Builds the release daemon + desktop binaries if missing (or --skip-build to reuse).
#   2. Creates a throwaway sandbox: its own socket, config dir, and data dir, plus 4 fake
#      git repos with a commit history, branches, and two worktree lanes carrying
#      uncommitted changes - so the Git panel and fleet sidebar show real-looking content.
#      None of this touches your real ~/.config/repomon, real data dir, or real daemon.
#   3. Starts a sandboxed `repomond` on its own socket and registers the fake repos/lanes
#      over the daemon's JSON-RPC (repo.add, lane.create), plus two agents in two lanes:
#        - orbit-api's lane runs a fake "agent" - a shell script that loops forever printing
#          plausible tool-call-shaped output, NOT a real agent CLI.
#        - meadow-web's lane runs a REAL `claude` (Claude Code CLI) session, spawned with no
#          task so it sits on its own idle welcome screen - zero API calls - under a fake,
#          non-authenticated identity (demo@example.com / Demo User / Demo Org) that lives in
#          its own throwaway CLAUDE_CONFIG_DIR inside the sandbox (see section 3b below); your
#          real ~/.claude, ~/.claude.json, and Keychain entries are never read or written.
#      No API calls happen anywhere in this script. meadow-web's claude lane is pinned
#      (agent.pin) so it is deterministically the top-priority, auto-selected lane when the
#      GUI opens - the tour needs its idle welcome screen visible from frame one, and relying
#      on natural activity-sort ordering to put it there is not reliable. orbit-api's fake
#      agent keeps looping in the background the whole time, visible in the Fleet sidebar for
#      liveliness, but isn't the hero.
#   4. Tests screen-recording permission with a throwaway 2-second capture. If macOS blocks
#      it, the script prints what to grant and stops - it does not open the GUI, run the
#      AppleScript tour, or touch your real desktop. Grant Screen Recording to your terminal
#      app in System Settings > Privacy & Security > Screen Recording, then re-run.
#   5. If recording works: launches the desktop binary pointed at the sandbox (a second,
#      independent process - the app has no single-instance guard, and it's on a different
#      socket than your real Repomon.app, so your real fleet is never touched), drives a
#      short AppleScript tour centered on the real claude session's idle terminal (fleet +
#      claude hero shot -> git panel -> editor panel -> back to the claude terminal), records
#      ~48s of the full display (menu bar excluded), and converts it to an optimized GIF at
#      docs/gui-demo.gif. No keystroke in the tour is ever sent into the claude pane itself.
#   6. Cleans up: kills the sandbox daemon, the app, and this sandbox's own tmux server (which
#      takes the spawned claude process down with it), removes the sandbox temp dir. Verifies
#      your real daemon's PID is unchanged before and after.
#
# Idempotent: safe to re-run. Re-running rebuilds the sandbox from scratch each time (the
# fake repos are regenerated, not reused) so the recording is always fresh.
#
# Tour beats dropped as keyboard-unreachable (see the AppleScript block below for details):
#   - Hovering the lane roster for a tooltip: hover-only, no mouse automation in this script.
#   - Explicitly switching lanes: no keymap chord moves lane selection other than bare j/k
#     (needs focus on the Fleet nav landmark, itself only reachable by a mouse click or an
#     unverifiable chain of blind Tab presses through an alphabetically-sorted, dynamically-
#     sized sidebar) and mod+g (jumps only among lanes flagged "needs attention"). Not needed
#     anyway: meadow-web's claude lane is already the pinned, auto-selected hero on open (see
#     step 3 above). The Fleet sidebar is visible for the entire tour regardless, so the other
#     live lane (orbit-api's fake agent) is on screen throughout too.
#   - Opening a file in the Editor panel: the file tree's rows are plain buttons with only
#     click handlers, and there is no reliable, verifiable keyboard path onto a specific row
#     (Tab order into a lazily-loaded tree cannot be confirmed without visually running the
#     app). The beat shows the editor tab with the tree visible instead.

set -euo pipefail

# ---------------------------------------------------------------------------
# Paths and options
# ---------------------------------------------------------------------------

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

KEEP_SANDBOX=0
SKIP_BUILD=0
STILL=0
for arg in "$@"; do
  case "$arg" in
    --keep-sandbox) KEEP_SANDBOX=1 ;;
    --skip-build) SKIP_BUILD=1 ;;
    --still) STILL=1 ;;
    *) echo "unknown flag: $arg" >&2; exit 1 ;;
  esac
done

# A short, fixed socket path - macOS AF_UNIX paths cap at 104 bytes, and mktemp's
# /var/folders/... dirs regularly blow past that, so this deliberately does NOT live
# under $SANDBOX.
SOCK_PATH="/tmp/repomon-demo.sock"
PROD_SOCK_PATH="/tmp/repomon-${USER}.sock"

# The daemon's tmux session label (`tmux -L <label>`) is NOT namespaced by REPOMON_DATA_DIR or
# XDG_CONFIG_HOME - it defaults to the fixed name "repomon" (crates/repomon-core/src/config.rs,
# DEFAULT_TMUX_SESSION) unless config.toml overrides it, and that tmux server is deliberately
# long-lived (it survives daemon restarts - see crates/repomon-daemon/src/reap.rs). Left at the
# default, every sandboxed agent this script spawns would land as a window in the exact same
# tmux server your real, production repomond attaches to - the one piece of state the socket
# path, config dir, and data dir isolation above does not cover. Give this sandbox's daemon its
# own label via config.toml below, and kill that server on exit (regardless of --keep-sandbox)
# so repeated runs never accumulate orphaned windows either.
TMUX_SESSION_LABEL="repomon-gui-demo"

SANDBOX="$(mktemp -d /tmp/repomon-gui-demo.XXXXXX)"
# Resolve /tmp's symlink (-> /private/tmp on macOS) once, right away: the daemon canonicalizes
# paths it stores (repo/worktree paths, tmux `-c` cwd), so anything that has to byte-for-byte
# match a worktree's cwd later - notably CLAUDE_DEMO_DIR's `projects` key, section 3b below -
# must be built from the same resolved form or it silently mismatches.
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
DATA_DIR="$SANDBOX/data"
CONFIG_HOME="$SANDBOX/config"
REPOS_DIR="$SANDBOX/repos"
WORKTREES_DIR="$SANDBOX/worktrees"
OUT_DIR="$SANDBOX/out"
mkdir -p "$DATA_DIR" "$CONFIG_HOME" "$REPOS_DIR" "$WORKTREES_DIR" "$OUT_DIR"

# Worktree paths, computed here (not down where the lanes are actually created) because the
# fake Claude identity file built in section 3 below needs meadow-web's exact worktree path
# as a `projects` key before the daemon (and its `lane.create` git-worktree-add) ever runs -
# the path just has to match byte-for-byte by the time the real `claude` session starts in it,
# not exist yet at the time this string is computed.
ORBIT_WT="$WORKTREES_DIR/orbit-api/rate-limit-headers"
MEADOW_WT="$WORKTREES_DIR/meadow-web/nav-focus-trap"

# A throwaway CLAUDE_CONFIG_DIR for the one real `claude` session this script spawns (see
# section 3 below) - entirely separate from the real ~/.claude*, never read or written here.
CLAUDE_DEMO_DIR="$SANDBOX/claude-demo"

DAEMON_BIN="$REPO_ROOT/target/release/repomond"
DESKTOP_BIN="$REPO_ROOT/target/release/repomon-desktop"

DAEMON_PID=""
APP_PID=""
PROD_PID_BEFORE=""
PROD_PID_AFTER=""

log() { echo "[gui-demo] $*"; }

cleanup() {
  local status=$?
  log "cleaning up..."
  if [[ -n "$APP_PID" ]] && kill -0 "$APP_PID" 2>/dev/null; then
    kill "$APP_PID" 2>/dev/null || true
    sleep 0.5
    kill -9 "$APP_PID" 2>/dev/null || true
  fi
  if [[ -n "$DAEMON_PID" ]] && kill -0 "$DAEMON_PID" 2>/dev/null; then
    kill "$DAEMON_PID" 2>/dev/null || true
    sleep 0.5
    kill -9 "$DAEMON_PID" 2>/dev/null || true
  fi
  # This sandbox's own tmux server (see the TMUX_SESSION_LABEL comment above) - unconditional,
  # like the daemon/app kills above, since --keep-sandbox only preserves files for inspection,
  # not live processes, and a leftover fake-agent tmux server has nothing worth keeping anyway.
  # This also takes down the real `claude` session spawned into it (section 3b/meadow-web) -
  # kill-server signals its pane's process group but doesn't block on exit, and a just-killed
  # claude process can still be flushing session state to CLAUDE_DEMO_DIR for a beat afterward,
  # so a `rm -rf` issued immediately can race an in-flight write and leave a non-empty leftover
  # dir behind (observed: claude-demo/ survived one cleanup while everything else was removed).
  # A short settle plus a couple of retries below absorbs that without slowing the common case.
  tmux -L "$TMUX_SESSION_LABEL" kill-server 2>/dev/null || true
  sleep 0.3
  rm -f "$SOCK_PATH"
  if [[ "$KEEP_SANDBOX" -eq 0 ]]; then
    for _ in 1 2 3; do
      rm -rf "$SANDBOX" 2>/dev/null || true
      [[ -e "$SANDBOX" ]] || break
      sleep 0.3
    done
    [[ -e "$SANDBOX" ]] && log "WARNING: could not fully remove sandbox at $SANDBOX - investigate."
  else
    log "kept sandbox at $SANDBOX"
  fi

  PROD_PID_AFTER="$(pgrep -f "repomond --socket $PROD_SOCK_PATH" | head -1 || true)"
  if [[ -n "$PROD_PID_BEFORE" && "$PROD_PID_BEFORE" != "$PROD_PID_AFTER" ]]; then
    log "WARNING: production daemon PID changed ($PROD_PID_BEFORE -> $PROD_PID_AFTER) - investigate."
  fi
  exit "$status"
}
trap cleanup EXIT

PROD_PID_BEFORE="$(pgrep -f "repomond --socket $PROD_SOCK_PATH" | head -1 || true)"
log "production daemon PID before: ${PROD_PID_BEFORE:-none}"

# ---------------------------------------------------------------------------
# 1. Build (if needed)
# ---------------------------------------------------------------------------

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  if [[ ! -x "$DAEMON_BIN" || ! -x "$DESKTOP_BIN" ]]; then
    log "building release binaries (repomon-daemon, repomon-desktop)..."
    ( cd apps/desktop && bun run build )
    cargo build --release -p repomon-daemon -p repomon-desktop
  else
    log "release binaries already present, skipping build (pass --skip-build to always skip)"
  fi
fi

# ---------------------------------------------------------------------------
# 2. Fake repos
# ---------------------------------------------------------------------------

DEMO_NAME="Demo User"
DEMO_EMAIL="demo@example.com"

# $1 name  $2 rel-days-ago  $3 message  (files must already be staged by the caller)
commit_at() {
  local days_ago="$1" msg="$2"
  local ts
  ts="$(date -u -v-"${days_ago}"d +%Y-%m-%dT%H:%M:%S 2>/dev/null || date -u -d "-${days_ago} days" +%Y-%m-%dT%H:%M:%S)"
  GIT_AUTHOR_NAME="$DEMO_NAME" GIT_AUTHOR_EMAIL="$DEMO_EMAIL" GIT_AUTHOR_DATE="$ts" \
  GIT_COMMITTER_NAME="$DEMO_NAME" GIT_COMMITTER_EMAIL="$DEMO_EMAIL" GIT_COMMITTER_DATE="$ts" \
    git commit -q -m "$msg"
}

init_repo() {
  local dir="$1"
  mkdir -p "$dir"
  ( cd "$dir" && git init -q -b main && git config user.name "$DEMO_NAME" \
      && git config user.email "$DEMO_EMAIL" && git config commit.gpgsign false )
}

log "generating fake repo: orbit-api"
ORBIT="$REPOS_DIR/orbit-api"
init_repo "$ORBIT"
mkdir -p "$ORBIT/src/routes"
cat > "$ORBIT/README.md" <<'EOF'
# orbit-api

Internal HTTP API. Node + TypeScript, Express-style routing.
EOF
cat > "$ORBIT/package.json" <<'EOF'
{
  "name": "orbit-api",
  "version": "0.3.0",
  "scripts": { "dev": "tsx src/server.ts", "test": "vitest run" }
}
EOF
cat > "$ORBIT/src/server.ts" <<'EOF'
import { createServer } from "./app";

const port = Number(process.env.PORT ?? 8080);
createServer().listen(port, () => {
  console.log(`orbit-api listening on :${port}`);
});
EOF
(cd "$ORBIT" && git add -A && commit_at 12 "Initial API scaffold")

cat > "$ORBIT/src/routes/health.ts" <<'EOF'
import type { Router } from "express";

export function registerHealth(router: Router) {
  router.get("/healthz", (_req, res) => res.json({ ok: true }));
}
EOF
(cd "$ORBIT" && git add -A && commit_at 9 "Add health check endpoint")

cat > "$ORBIT/src/routes/rateLimit.ts" <<'EOF'
import type { RequestHandler } from "express";

const WINDOW_MS = 60_000;
const MAX_REQUESTS = 120;
const hits = new Map<string, number[]>();

export const rateLimit: RequestHandler = (req, res, next) => {
  const now = Date.now();
  const key = req.ip ?? "unknown";
  const recent = (hits.get(key) ?? []).filter((t) => now - t < WINDOW_MS);
  recent.push(now);
  hits.set(key, recent);
  if (recent.length > MAX_REQUESTS) {
    return res.status(429).json({ error: "rate limit exceeded" });
  }
  next();
};
EOF
(cd "$ORBIT" && git add -A && commit_at 6 "Add rate limiting middleware")

cat >> "$ORBIT/src/routes/health.ts" <<'EOF'

export function registerReady(router: Router) {
  router.get("/readyz", (_req, res) => res.json({ ready: true }));
}
EOF
(cd "$ORBIT" && git add -A && commit_at 3 "Add readiness probe alongside health check")

log "generating fake repo: meadow-web"
MEADOW="$REPOS_DIR/meadow-web"
init_repo "$MEADOW"
mkdir -p "$MEADOW/src/components"
cat > "$MEADOW/README.md" <<'EOF'
# meadow-web

Customer-facing web app. React + Vite.
EOF
cat > "$MEADOW/package.json" <<'EOF'
{
  "name": "meadow-web",
  "version": "1.4.0",
  "scripts": { "dev": "vite", "build": "vite build" }
}
EOF
cat > "$MEADOW/src/App.tsx" <<'EOF'
import { Nav } from "./components/Nav";

export function App() {
  return (
    <div className="app">
      <Nav />
      <main>{/* routed content */}</main>
    </div>
  );
}
EOF
(cd "$MEADOW" && git add -A && commit_at 14 "Scaffold web app")

cat > "$MEADOW/src/components/Nav.tsx" <<'EOF'
export function Nav() {
  return (
    <nav>
      <a href="/">Home</a>
      <a href="/pricing">Pricing</a>
      <a href="/docs">Docs</a>
    </nav>
  );
}
EOF
(cd "$MEADOW" && git add -A && commit_at 10 "Add nav bar")

cat > "$MEADOW/src/components/MobileMenu.tsx" <<'EOF'
import { useState } from "react";

export function MobileMenu() {
  const [open, setOpen] = useState(false);
  return (
    <div className="mobile-menu" data-open={open}>
      <button onClick={() => setOpen((v) => !v)}>Menu</button>
    </div>
  );
}
EOF
(cd "$MEADOW" && git add -A && commit_at 5 "Add mobile menu toggle")

cat > "$MEADOW/package-lock-notes.md" <<'EOF'
Bumped React to 18.3, Vite to 5.2. No breaking changes observed.
EOF
(cd "$MEADOW" && git add -A && commit_at 2 "Update dependencies")

log "generating fake repo: forge-cli"
FORGE="$REPOS_DIR/forge-cli"
init_repo "$FORGE"
mkdir -p "$FORGE/src"
cat > "$FORGE/README.md" <<'EOF'
# forge-cli

Scaffolding CLI for internal service templates.
EOF
cat > "$FORGE/Cargo.toml" <<'EOF'
[package]
name = "forge-cli"
version = "0.2.1"
edition = "2021"

[dependencies]
clap = { version = "4", features = ["derive"] }
EOF
cat > "$FORGE/src/main.rs" <<'EOF'
fn main() {
    println!("forge: scaffold a new service with `forge new <name>`");
}
EOF
(cd "$FORGE" && git add -A && commit_at 20 "Init CLI skeleton")

cat > "$FORGE/src/config.rs" <<'EOF'
pub struct Config {
    pub template_dir: String,
    pub verbose: bool,
}
EOF
(cd "$FORGE" && git add -A && commit_at 15 "Add config subcommand")

cat >> "$FORGE/src/main.rs" <<'EOF'

// TODO: wire --verbose through to the template renderer.
EOF
(cd "$FORGE" && git add -A && commit_at 8 "Add --verbose flag")

(cd "$FORGE" && git checkout -q -b spike/plugin-system)
cat > "$FORGE/src/plugin.rs" <<'EOF'
// Early sketch: dynamic plugin loading via a `forge.toml` [plugins] table.
pub struct PluginSpec {
    pub name: String,
    pub entry: String,
}
EOF
(cd "$FORGE" && git add -A && commit_at 4 "Sketch plugin loading")
(cd "$FORGE" && git checkout -q main)

log "generating fake repo: atlas-docs"
ATLAS="$REPOS_DIR/atlas-docs"
init_repo "$ATLAS"
mkdir -p "$ATLAS/docs"
cat > "$ATLAS/README.md" <<'EOF'
# atlas-docs

Internal docs site (mkdocs).
EOF
cat > "$ATLAS/mkdocs.yml" <<'EOF'
site_name: Internal Docs
nav:
  - Home: index.md
  - Getting Started: getting-started.md
EOF
cat > "$ATLAS/docs/index.md" <<'EOF'
# Internal Docs

Start here.
EOF
(cd "$ATLAS" && git add -A && commit_at 18 "Initial docs scaffold")

cat > "$ATLAS/docs/getting-started.md" <<'EOF'
# Getting Started

1. Clone the repo.
2. Run `mkdocs serve`.
3. Edit files under `docs/`.
EOF
(cd "$ATLAS" && git add -A && commit_at 11 "Add getting started guide")

cat > "$ATLAS/docs/api-reference.md" <<'EOF'
# API Reference

Stub - see orbit-api's OpenAPI spec once published.
EOF
(cd "$ATLAS" && git add -A && commit_at 4 "Add API reference stub")

# ---------------------------------------------------------------------------
# 3. Fake agent script (no real CLI, no API calls - just plausible output)
# ---------------------------------------------------------------------------

FAKE_AGENT="$SANDBOX/fake_agent.sh"
cat > "$FAKE_AGENT" <<'EOF'
#!/usr/bin/env bash
# A stand-in for a coding agent: loops forever, printing tool-call-shaped lines, a short
# diff, and status beats on 1-2s delays, so the terminal pane always looks alive no matter
# how much wall-clock time passes between spawn and whenever screencapture actually starts
# (build + sandbox setup + permission checks can eat tens of seconds). Does not call any
# model or network API. Deliberately generic - it only ever touches paths inside this
# sandbox's own fake orbit-api repo, never a real project name.
task="${1:-Investigate the flaky rate-limit test}"
echo "> $task"
sleep 1

while true; do
  echo "  reading src/routes/rateLimit.ts"
  sleep 1.5
  echo "  reading test/rateLimit.test.ts"
  sleep 1.5
  echo "  found: window reset uses Date.now() directly, no fake-timer support"
  sleep 2
  echo "  editing src/routes/rateLimit.ts"
  sleep 1
  echo "    - if (recent.length > MAX_REQUESTS) {"
  sleep 1
  echo "    + const remaining = Math.max(0, MAX_REQUESTS - recent.length);"
  sleep 1
  echo "    + if (remaining <= 0) {"
  sleep 1.5
  echo "  running tests..."
  sleep 1
  echo "  Waiting for tests..."
  sleep 2
  echo "  12 passed, 0 failed"
  sleep 1.5
  echo "  done - ready for review"
  sleep 2
done
EOF
chmod +x "$FAKE_AGENT"

# ---------------------------------------------------------------------------
# 3b. A REAL `claude` session, with a fake, non-authenticated identity
# ---------------------------------------------------------------------------
#
# meadow-web's lane (below) runs the actual Claude Code CLI - not fake_agent.sh - spawned
# with no task, so it sits on its own idle welcome screen and makes zero API calls. To keep
# it from ever showing this machine's real Claude account, it gets its own CLAUDE_CONFIG_DIR,
# fully separate from ~/.claude and ~/.claude.json. What was verified (read-only, against the
# real config - nothing here reads or writes it) before writing this:
#
#   - CLAUDE_CONFIG_DIR relocates Claude Code's ENTIRE per-account state, including the
#     top-level identity file, to "$CLAUDE_CONFIG_DIR/.claude.json" - confirmed by inspecting
#     an existing second account on this machine (~/.claude-work), which keeps its own
#     ~/.claude-work/.claude.json with its own `oauthAccount` block, structurally identical
#     to ~/.claude.json's.
#   - The OAuth token itself lives in the macOS Keychain under a service name keyed to the
#     config dir ("Claude Code-credentials" or "...-<hash>"), independent of anything in the
#     config dir's files. A brand-new config dir has no matching Keychain entry, so a session
#     using it is NOT authenticated - confirmed by launching one and reading `/status`, which
#     reported "Auth token: none". That's exactly what's wanted here: zero possibility of a
#     real API call ever succeeding, on top of the tour never typing anything into this pane
#     (see the tour's keystroke review near the bottom of this script).
#   - Despite having no valid token, Claude Code's welcome screen reads the *cached* account
#     fields (displayName/organizationName/emailAddress) straight out of .claude.json to
#     render its "Welcome back <name>!" card and the "<plan> · <org>" line under it - no
#     network call needed. So writing fake values there (via Python's json module, never
#     sed/string-templating a JSON file) makes the *displayed* identity fake, while the
#     "Not logged in · Run /login" status-bar badge confirms no real auth is attached.
#   - hasCompletedOnboarding + theme skip the first-run theme-picker prompt; a `projects`
#     entry keyed by this exact worktree path (must match byte-for-byte, hence computing
#     MEADOW_WT up in the paths section above rather than down where lanes are created) with
#     hasTrustDialogAccepted skips the "do you trust this folder" dialog - both would
#     otherwise leave the pane short of the idle screen and need typed input the tour never
#     sends.
mkdir -p "$CLAUDE_DEMO_DIR"
python3 - "$CLAUDE_DEMO_DIR/.claude.json" "$MEADOW_WT" <<'PYEOF'
import json
import sys

out_path, cwd = sys.argv[1], sys.argv[2]
config = {
    "numStartups": 1,
    "autoUpdates": False,
    "theme": "dark",
    "hasCompletedOnboarding": True,
    "oauthAccount": {
        "accountUuid": "00000000-0000-0000-0000-000000000000",
        "emailAddress": "demo@example.com",
        "organizationUuid": "00000000-0000-0000-0000-000000000001",
        "hasExtraUsageEnabled": False,
        "billingType": "stripe_subscription",
        "accountCreatedAt": "2026-01-01T00:00:00.000000Z",
        "subscriptionCreatedAt": "2026-01-01T00:00:00.000000Z",
        "ccOnboardingFlags": {},
        "claudeCodeTrialEndsAt": None,
        "claudeCodeTrialDurationDays": None,
        "seatTier": None,
        "displayName": "Demo User",
        "profileFetchedAt": 0,
        "organizationRole": "admin",
        "workspaceRole": None,
        "organizationName": "Demo Org",
        "organizationType": "claude_max",
        "organizationRateLimitTier": "default_claude_max_5x",
        "userRateLimitTier": None,
    },
    "projects": {
        cwd: {
            "allowedTools": [],
            "mcpContextUris": [],
            "mcpServers": {},
            "enabledMcpjsonServers": [],
            "disabledMcpjsonServers": [],
            "hasTrustDialogAccepted": True,
            "projectOnboardingSeenCount": 1,
            "hasClaudeMdExternalIncludesApproved": False,
            "hasClaudeMdExternalIncludesWarningShown": False,
        }
    },
}
with open(out_path, "w") as f:
    json.dump(config, f, indent=2)
PYEOF
log "built fake, non-authenticated Claude identity for the demo session (demo@example.com / Demo Org) at $CLAUDE_DEMO_DIR - real ~/.claude* never touched"

mkdir -p "$CONFIG_HOME/repomon"
cat > "$CONFIG_HOME/repomon/config.toml" <<EOF
tmux_session = "$TMUX_SESSION_LABEL"

[agents]
demo-agent = "$FAKE_AGENT"
demo-claude = "CLAUDE_CONFIG_DIR=$CLAUDE_DEMO_DIR claude"
EOF

# ---------------------------------------------------------------------------
# 4. Start sandboxed daemon, register repos + lanes over RPC
# ---------------------------------------------------------------------------

rm -f "$SOCK_PATH"
log "starting sandboxed daemon on $SOCK_PATH (data dir: $DATA_DIR)"
XDG_CONFIG_HOME="$CONFIG_HOME" REPOMON_DATA_DIR="$DATA_DIR" \
  "$DAEMON_BIN" --socket "$SOCK_PATH" \
  > "$OUT_DIR/daemond.log" 2>&1 &
DAEMON_PID=$!

for _ in $(seq 1 100); do
  [[ -S "$SOCK_PATH" ]] && break
  sleep 0.1
done
if [[ ! -S "$SOCK_PATH" ]]; then
  log "sandbox daemon never bound its socket - see $OUT_DIR/daemond.log"
  exit 1
fi
log "sandbox daemon up (pid $DAEMON_PID)"

RPC_HELPER="$SANDBOX/rpc.py"
cat > "$RPC_HELPER" <<'PYEOF'
"""Minimal framed JSON-RPC client (see crates/repomon-core/src/protocol.rs)."""
import json, os, socket, struct, sys

SOCK_PATH = os.environ["REPOMON_MCP_SOCKET"]


def call(method, params=None, timeout=10.0):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    s.connect(SOCK_PATH)
    req = {"jsonrpc": "2.0", "id": 1, "method": method}
    if params is not None:
        req["params"] = params
    payload = json.dumps(req).encode()
    s.sendall(struct.pack("<I", len(payload)) + payload)
    while True:
        header = _recv_exact(s, 4)
        n = struct.unpack("<I", header)[0]
        body = _recv_exact(s, n)
        msg = json.loads(body)
        if msg.get("id") is not None:
            s.close()
            return msg


def _recv_exact(s, n):
    buf = b""
    while len(buf) < n:
        chunk = s.recv(n - len(buf))
        if not chunk:
            raise ConnectionError("socket closed mid-frame")
        buf += chunk
    return buf


if __name__ == "__main__":
    method = sys.argv[1]
    params = json.loads(sys.argv[2]) if len(sys.argv) > 2 else None
    result = call(method, params)
    if result.get("error"):
        print(json.dumps(result), file=sys.stderr)
        sys.exit(1)
    print(json.dumps(result["result"]))
PYEOF

rpc() {
  local method="$1"
  shift
  if [[ $# -gt 0 ]]; then
    REPOMON_MCP_SOCKET="$SOCK_PATH" python3 "$RPC_HELPER" "$method" "$1"
  else
    REPOMON_MCP_SOCKET="$SOCK_PATH" python3 "$RPC_HELPER" "$method"
  fi
}

log "registering repos..."
ORBIT_ID="$(rpc repo.add "{\"path\": \"$ORBIT\"}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
MEADOW_ID="$(rpc repo.add "{\"path\": \"$MEADOW\"}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
rpc repo.add "{\"path\": \"$FORGE\"}" > /dev/null
rpc repo.add "{\"path\": \"$ATLAS\"}" > /dev/null
log "repos registered: orbit-api=$ORBIT_ID meadow-web=$MEADOW_ID forge-cli atlas-docs"

log "creating worktree lanes..."
LANE1_JSON="$(rpc lane.create "{\"repo_id\": $ORBIT_ID, \"branch\": \"feat/rate-limit-headers\", \"path\": \"$ORBIT_WT\"}")"
LANE1_ID="$(echo "$LANE1_JSON" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"

LANE2_JSON="$(rpc lane.create "{\"repo_id\": $MEADOW_ID, \"branch\": \"fix/nav-focus-trap\", \"path\": \"$MEADOW_WT\"}")"
LANE2_ID="$(echo "$LANE2_JSON" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
log "lanes created: orbit-api#$LANE1_ID meadow-web#$LANE2_ID"

log "seeding uncommitted changes in each lane..."
cat >> "$ORBIT_WT/src/routes/rateLimit.ts" <<'EOF'

// TODO: expose remaining-requests via a response header.
EOF
cat > "$ORBIT_WT/src/routes/rateLimitHeaders.ts" <<'EOF'
// New: X-RateLimit-Remaining header (uncommitted work-in-progress).
export const REMAINING_HEADER = "X-RateLimit-Remaining";
EOF

sed -i.bak 's/const \[open, setOpen\] = useState(false);/const [open, setOpen] = useState(false);\n  \/\/ TODO: trap focus while open/' "$MEADOW_WT/src/components/MobileMenu.tsx"
rm -f "$MEADOW_WT/src/components/MobileMenu.tsx.bak"
cat > "$MEADOW_WT/src/components/useFocusTrap.ts" <<'EOF'
// New: focus-trap hook for the mobile menu (uncommitted work-in-progress).
export function useFocusTrap() {
  // ...
}
EOF

# One commit ahead of main in the hero lane (meadow-web - see the pin decision below), so the
# Git panel's Branch section shows a real "ahead" commit list instead of "Nothing ahead of main".
mkdir -p "$MEADOW_WT/src/hooks"
cat > "$MEADOW_WT/src/hooks/useMediaQuery.ts" <<'EOF'
// Small hook used by the nav-focus-trap fix to detect mobile breakpoints.
export function useMediaQuery(query: string): boolean {
  return typeof window !== "undefined" && window.matchMedia(query).matches;
}
EOF
git -C "$MEADOW_WT" add src/hooks/useMediaQuery.ts
git -C "$MEADOW_WT" -c user.name="Demo User" -c user.email="demo@example.com" \
  commit -q -m "Add useMediaQuery hook for mobile breakpoint checks"

log "seeded worktree status ($ORBIT_WT):"
git -C "$ORBIT_WT" status --porcelain | sed 's/^/[gui-demo]   /'
log "seeded worktree status ($MEADOW_WT):"
git -C "$MEADOW_WT" status --porcelain | sed 's/^/[gui-demo]   /'

# The Git panel's working-tree section gates its "clean" empty state on the daemon's CACHED
# DirtyState counts, which refresh on the daemon's own sync cadence - not on the live diff.
# Wait until the daemon has actually noticed the dirty files before recording, or the panel
# films as "Working tree clean" despite the seeds above. Checks the hero lane (meadow-web,
# LANE2 - see the pin decision below) since that's the one the Git panel beat films.
log "waiting for daemon to notice dirty worktree..."
for _ in $(seq 1 30); do
  DIRTY_TOTAL="$(rpc lane.list '{}' | python3 -c '
import json, sys
lanes = json.load(sys.stdin)
lanes = lanes if isinstance(lanes, list) else lanes.get("lanes", [])
for l in lanes:
    if l.get("id") == '"$LANE2_ID"':
        d = (l.get("state") or {}).get("dirty") or {}
        print(int(d.get("staged", 0)) + int(d.get("unstaged", 0)) + int(d.get("untracked", 0)))
        break
else:
    print(0)
')"
  [[ "${DIRTY_TOTAL:-0}" -gt 0 ]] && break
  sleep 1
done
log "daemon dirty count for hero lane: ${DIRTY_TOTAL:-0}"
if [[ "${DIRTY_TOTAL:-0}" -eq 0 ]]; then
  log "WARNING: daemon never registered the seeded dirty files - the Git panel's working-tree"
  log "WARNING: beat will film as 'Working tree clean'. Ctrl-C now and re-run if that matters."
  sleep 5
fi

log "spawning fake agent in orbit-api lane..."
rpc agent.spawn "{\"lane_id\": $LANE1_ID, \"agent\": \"demo-agent\", \"task\": \"Investigate the flaky rate-limit test\"}" > /dev/null

# Spawn a REAL `claude` in the meadow-web lane - the actual Claude Code CLI, under the fake,
# non-authenticated identity built in section 3b above. No "task" key: with it absent (rather
# than an empty string), the daemon's `p.task.filter(|t| !t.is_empty())` never appends a
# prompt, so this session lands on its idle welcome screen and never calls the model API.
log "spawning a real (unauthenticated, fake-identity) claude session in meadow-web lane..."
rpc agent.spawn "{\"lane_id\": $LANE2_ID, \"agent\": \"demo-claude\"}" > /dev/null

# Pin the REAL claude lane so it sorts first (stores/fleet.ts byPriority: pinned lanes always
# win) and is therefore the lane auto-selected the moment the GUI's first lane.list poll
# lands. The tour depends on this: it never sends a lane-selection keystroke, because there is
# no reliable, keyboard-only way to land on a specific lane (see the header comment above), so
# the hero shot has to already be pointed at the right lane on open. meadow-web's idle welcome
# screen ("Welcome back Demo User!") is the recognizable showcase here - a real Claude Code
# session, not a lookalike - so it's the hero, not orbit-api's fake streaming agent. The fake
# agent keeps running in orbit-api's lane regardless (unpinned but still visible in the Fleet
# sidebar the whole time) purely for sidebar liveliness.
log "pinning meadow-web's claude lane so it's the default selection..."
rpc agent.pin "{\"lane_id\": $LANE2_ID, \"pinned\": true}" > /dev/null

log "sandbox ready: 4 repos, 2 lanes with uncommitted changes, 1 looping fake agent + 1 real idle claude session (pinned)"

# ---------------------------------------------------------------------------
# 5. Screen recording permission check
# ---------------------------------------------------------------------------

PERM_TEST="$OUT_DIR/perm-test.png"
rm -f "$PERM_TEST"
if ! screencapture -x "$PERM_TEST" 2>"$OUT_DIR/perm-test.err" || [[ ! -s "$PERM_TEST" ]]; then
  log "----------------------------------------------------------------------"
  log "Screen recording permission is NOT granted - cannot record the GUI demo."
  log "The sandbox above is fully set up and verified (repos/lanes/agent all"
  log "registered over RPC), but the GUI has NOT been launched and nothing was"
  log "recorded, so your real desktop was never touched."
  log ""
  log "To fix: System Settings > Privacy & Security > Screen Recording, grant"
  log "access to your terminal app (Terminal.app, iTerm, etc - whichever hosts"
  log "this shell), then re-run this script."
  log "----------------------------------------------------------------------"
  exit 2
fi
log "screen recording permission OK"

# ---------------------------------------------------------------------------
# 6. Launch the desktop app on the sandbox and record a tour
# ---------------------------------------------------------------------------

log "launching desktop app against sandbox socket..."
XDG_CONFIG_HOME="$CONFIG_HOME" REPOMON_DATA_DIR="$DATA_DIR" REPOMON_SOCKET="$SOCK_PATH" \
  "$DESKTOP_BIN" > "$OUT_DIR/desktop.log" 2>&1 &
APP_PID=$!
sleep 4

# Full-screen recording: put the app window into native macOS full screen (AXFullScreen), which
# hides the menu bar entirely, and record the whole display. The user's own status items
# (clock, battery, widgets) never appear because full-screen mode covers them. If AXFullScreen
# is not honored (attribute missing or denied), fall back to filling the display below the
# menu bar and recording that region instead.
MENUBAR_H=25
read -r SCREEN_W SCREEN_H < <(osascript -e 'tell application "Finder" to get bounds of window of desktop' | awk -F", " '{print $3, $4}')
if [[ -z "${SCREEN_W:-}" || -z "${SCREEN_H:-}" ]]; then
  SCREEN_W=1440 SCREEN_H=925  # conservative fallback
fi
FS_OK="$(osascript <<OSA
tell application "System Events"
  set frontmost of first process whose unix id is $APP_PID to true
  delay 0.3
  try
    set value of attribute "AXFullScreen" of front window of (first process whose unix id is $APP_PID) to true
    return "yes"
  on error
    return "no"
  end try
end tell
OSA
)"
if [[ "$FS_OK" == "yes" ]]; then
  # Native full screen: the space-switch animation takes a moment; record the full display.
  sleep 2.5
  WIN_X=0 WIN_Y=0 WIN_W=$SCREEN_W WIN_H=$SCREEN_H
  log "window in native full screen (${SCREEN_W}x${SCREEN_H})"
else
  log "AXFullScreen not honored; falling back to menu-bar-excluded fill"
  WIN_X=0 WIN_Y=$MENUBAR_H WIN_W=$SCREEN_W WIN_H=$((SCREEN_H - MENUBAR_H))
  osascript <<OSA
tell application "System Events"
  set frontmost of first process whose unix id is $APP_PID to true
  delay 0.3
  try
    set position of front window of (first process whose unix id is $APP_PID) to {$WIN_X, $WIN_Y}
    set size of front window of (first process whose unix id is $APP_PID) to {$WIN_W, $WIN_H}
  end try
end tell
OSA
fi
sleep 1

# Tour length: sum of every sleep below plus the pre-roll, ~46.5s. REC_SECONDS gives a small
# buffer over that so screencapture never cuts the final hold short; keep the two in step if
# you change the beats below.
REC_SECONDS=50

MOV_PATH="$OUT_DIR/gui-demo-raw.mov"

# Record the Repomon window ONLY (by CGWindowID), so nothing else on the display can ever
# appear in the demo - not the desktop, not notifications, not other windows. Window-ID
# lookup needs pyobjc's Quartz (present in the recording user's python3); if unavailable,
# fall back to the display-region capture of the (full-screened) window.
DEMO_WIN_ID="$(python3 - "$APP_PID" <<'PYEOF' 2>/dev/null || true
import sys
try:
    import Quartz
except ImportError:
    sys.exit(1)
pid = int(sys.argv[1])
wins = Quartz.CGWindowListCopyWindowInfo(
    Quartz.kCGWindowListOptionOnScreenOnly | Quartz.kCGWindowListExcludeDesktopElements, Quartz.kCGNullWindowID)
best = None
for w in wins or []:
    if w.get("kCGWindowOwnerPID") != pid:
        continue
    b = w.get("kCGWindowBounds", {})
    area = b.get("Width", 0) * b.get("Height", 0)
    if best is None or area > best[1]:
        best = (w.get("kCGWindowNumber"), area)
if best:
    print(best[0])
PYEOF
)"
if [[ "$STILL" == 1 ]]; then
  # One frame of the hero shot: the pinned claude lane's idle terminal with the fleet beside it.
  sleep 3
  STILL_RAW="$OUT_DIR/preview-raw.png"
  if [[ -n "${DEMO_WIN_ID:-}" ]]; then
    screencapture -x -o -l "$DEMO_WIN_ID" "$STILL_RAW"
  else
    screencapture -x -o -R "${WIN_X},${WIN_Y},${WIN_W},${WIN_H}" "$STILL_RAW"
  fi
  [[ -s "$STILL_RAW" ]] || { log "still capture failed (screen-recording permission?)"; exit 1; }
  # Retina capture is 2x; bring it to the window's logical width so the README stays light.
  sips --resampleWidth 1440 "$STILL_RAW" --out "$REPO_ROOT/docs/preview.png" >/dev/null
  log "wrote docs/preview.png ($(stat -f%z "$REPO_ROOT/docs/preview.png") bytes)"
  exit 0
fi
log "recording ~${REC_SECONDS}s to $MOV_PATH"
if [[ -n "${DEMO_WIN_ID:-}" ]]; then
  log "capturing window id $DEMO_WIN_ID (window-only recording, no shadow)"
  screencapture -v -V "$REC_SECONDS" -x -o -l "$DEMO_WIN_ID" "$MOV_PATH" &
else
  log "Quartz window-id lookup unavailable; capturing display region instead"
  screencapture -v -V "$REC_SECONDS" -x -o -R "${WIN_X},${WIN_Y},${WIN_W},${WIN_H}" "$MOV_PATH" &
fi
REC_PID=$!
sleep 2.0

tour_key() {
  # $1 = key spec for `keystroke`, $2 = using-modifiers clause. Matches BINDINGS in
  # apps/desktop/src/keymap.ts - verify against that file before changing any chord here.
  osascript -e "tell application \"System Events\" to keystroke \"$1\" using {$2}"
}

tour_keycode() {
  # $1 = numeric key code, no modifiers (Tab=48, Return=36, Escape=53).
  osascript -e "tell application \"System Events\" to key code $1"
}

# Beat 1 (HERO SHOT): fleet sidebar with meadow-web's nav-focus-trap lane already selected
# (pinned during sandbox setup above, so no keystroke is needed to get here) and a REAL Claude
# Code session sitting on its idle welcome screen ("Welcome back Demo User!") in the terminal
# bay - a genuine claude session, not a lookalike, under a fake, non-authenticated identity
# (see section 3b). orbit-api's fake streaming agent is still visible and looping in the Fleet
# sidebar for liveliness. This is the point of the product - hold it the longest.
sleep 9.0

# Beat 2: Git panel (mod+3) on that same lane, whose working tree is seeded dirty (an edited
# MobileMenu.tsx, an untracked useFocusTrap.ts) and one commit ahead of main (useMediaQuery.ts)
# - no lane switch needed, it's already the selected lane. (Dropped: switching to another lane
# first - see header comment for why.)
tour_key "3" "command down"
sleep 2.0
sleep 10.0

# Beat 3: Editor panel (mod+7) on the same lane. Shows the file tree for real (including the
# uncommitted useFocusTrap.ts once src/components/ is expanded) but does not open a file -
# TreeEntryRow's rows are plain onClick buttons with no verified keyboard path onto a specific
# row (see header comment). Reading the tour, not clicking, is still the point of this beat.
tour_key "7" "command down"
sleep 2.0
sleep 6.0

# Beat 4: Settings (mod+,) -> System tab, for the bundled-tmux badge and System Health view.
# There is no chord straight to the System tab, but Modal.tsx's focus trap is deterministic:
# on open, focus lands on the dialog's first focusable element in DOM order, which is the
# header's Close button (the tab strip is rendered inside the content div, after the header).
# Tab once -> the "General" tab button (first tab, DOM order). Tab again -> "System" (second
# tab). Return activates it like a click, since Enter/Space both fire a focused <button>'s
# click handler natively. Verified by reading Modal.tsx and SettingsModal.tsx's TABS array
# (General, System, Agents, Notifications, Appearance, Automation, Keyboard) - not by running
# the GUI, which this script cannot do in this environment.
# (Settings beat removed: the Tab-order route to the System tab proved unreliable in the
# recorded run - it landed on General and the closing Escape leaked a ^[ into the terminal.
# The reclaimed ~9s went to the Git panel and the final agent hold instead.)

# Beat 5: back to the claude terminal for the close. rightPanelTab is still "editor" from beat
# 3 (Settings is a separate modal and never touched it), and openPanelTab's own toggle rule is
# "already open on this tab -> close" - so pressing mod+7 again collapses the right rail
# instead of doing nothing, handing the full terminal bay back to the idle claude welcome
# screen for the final hold. No keystroke here or anywhere in this tour is ever sent into the
# claude pane itself (all chords target repomon's own UI, via System Events on the app process)
# - it stays untouched and idle for the whole recording.
tour_key "7" "command down"
sleep 1.5
sleep 12.5

wait "$REC_PID" 2>/dev/null || true
log "recording done"

# ---------------------------------------------------------------------------
# 7. Convert to an optimized GIF
# ---------------------------------------------------------------------------

DOCS_GIF="$REPO_ROOT/docs/gui-demo.gif"
DOCS_MOV="$REPO_ROOT/docs/gui-demo.mov"
PALETTE="$OUT_DIR/palette.png"

if [[ -s "$MOV_PATH" ]]; then
  # screencapture -V records at the display's full backing resolution (2880x1800 on a 2x
  # Retina screen for this 1440x900 window), so this step downscales ~2.4x to 1200px wide.
  # `flags=lanczos` rings (overshoots) at high-contrast edges, which on this app's
  # near-black theme shows up as a dark halo hugging light text and icon edges - the "black
  # outline" that is not present in the live app, only in a scaled-down raster of it.
  # `bicubic` has a much gentler falloff and does not ring, at a small cost in sharpness that
  # does not matter at GIF size. If a fringe is still visible after this, it is baked into
  # the .mov itself (screencapture's own H264 encode, which its CLI does not expose a
  # quality/bitrate knob for) rather than introduced by this conversion step.
  log "building palette..."
  ffmpeg -y -i "$MOV_PATH" -vf "fps=12,scale=1200:-1:flags=bicubic,palettegen" "$PALETTE" \
    -loglevel error
  log "encoding gif..."
  ffmpeg -y -i "$MOV_PATH" -i "$PALETTE" \
    -filter_complex "fps=12,scale=1200:-1:flags=bicubic[x];[x][1:v]paletteuse" \
    "$DOCS_GIF" -loglevel error

  gif_size=$(stat -f%z "$DOCS_GIF" 2>/dev/null || stat -c%s "$DOCS_GIF")
  log "gif size: $((gif_size / 1024 / 1024)) MB"

  mov_size=$(stat -f%z "$MOV_PATH" 2>/dev/null || stat -c%s "$MOV_PATH")
  if [[ "$mov_size" -lt $((15 * 1024 * 1024)) ]]; then
    cp "$MOV_PATH" "$DOCS_MOV"
    log "kept source .mov at docs/gui-demo.mov ($((mov_size / 1024 / 1024)) MB)"
  else
    log "source .mov too large ($((mov_size / 1024 / 1024)) MB), discarding (gif only)"
  fi
else
  log "no recording produced, skipping gif conversion"
fi

log "done."
