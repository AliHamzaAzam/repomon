# repomon

**Mission control for parallel AI coding agents — across many repos, branches, and
worktrees, from one terminal.**

Other tools run parallel agents in *one repo, many worktrees* (Claude Squad, Conductor,
Crystal, ccmanager, …). repomon is built for the developer with 5–15 active projects and a
fleet of agents running at once: **many repos × many worktrees × many agents**, on one
screen, spawned and steered from one place.

```
REPOMON                                              14:02 fri 29 may 2026
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
FLEET   8 agents · 4 repos · 3 need you                    ↑ sorted: needs-you
─────────────────────────────────────────────────────────────────────────

  pos-saas ────────────────────────────────────────────────────────────
  ⏸ wt-checkout  hotfix/checkout-bug     claude  needs you   89↻   3m
  ▶ main         feat/supabase-migration claude  running    142↻  18m
  ○ wt-ui        spike/new-pos-ui                idle              2h

  montage-ai ──────────────────────────────────────────────────────────
  ⏸ wt-mcp       spike/mcp-batch         codex   needs you   44↻   8m
  ▶ main         phase-2-studio-floor    claude  running    201↻   2m

  ↑↓ select   ↵/→ open   spc babysit   n new-lane   / filter   g needs-you   q
```

repomon is one tool with four **zoom levels**, one selection that follows you the whole way:

- **Fleet** — every agent on one screen; the ones waiting on you float to the top.
- **Split** — fleet sidebar + the selected agent's live output and an input line.
- **Babysit grid** — live tiles auto-sized to your window; watch and nudge several at once.
- **Focus** — one agent full-screen with full live terminal, input, and controls.

Arrow keys drive everything. Agents run in durable tmux sessions, so they survive closing
the UI and reattach with full scrollback.

## Architecture

A background daemon (`repomond`) owns SQLite, file watchers, the git layer, and the
tmux-backed agent runtime, exposing a JSON-RPC API over a Unix socket. The TUI (`repomon`)
is a thin client. Three crates:

- `repomon-core` — data model, gix git layer, SQLite store, watchers, agent runtime.
- `repomon-daemon` — `repomond`: the socket server and background services.
- `repomon-tui` — `repomon`: the terminal UI.

## Usage

```sh
cargo build --release                  # builds repomond + repomon

repomon daemon start                   # install + load the launchd daemon (macOS)
repomon add ~/code/pos-saas            # register a repo
repomon discover ~/code --add          # or find and register many at once
repomon                                # launch the TUI

# headless / scripting
repomon lane list
repomon lane new --repo pos-saas --branch feat/inventory --source main
repomon lane delete feat/inventory --delete-branch
repomon daemon status | logs | stop | uninstall
```

For development without launchd, run the daemon inline with `repomon --embedded`, or
start `repomond` directly.

## Shell integration (cd-on-exit)

Pressing `c` on a lane exits repomon and changes your shell into that worktree. repomon
writes the path to the file descriptor in `$REPOMON_CD_FD`; add this wrapper to your
`~/.zshrc` / `~/.bashrc` so the shell acts on it:

```sh
repomon() {
  local tmp; tmp=$(mktemp)
  REPOMON_CD_FD=3 command repomon "$@" 3>"$tmp"
  local dir; dir=$(cat "$tmp"); rm -f "$tmp"
  [ -n "$dir" ] && [ -d "$dir" ] && cd "$dir"
}
```

## Status

🚧 Early development. The Observatory foundation and the agent multiplexer are landing
milestone by milestone; see the build plan for what's next (timeline/sessions dashboard,
more agent kinds).

## License

MIT © Ali Hamza Azam
