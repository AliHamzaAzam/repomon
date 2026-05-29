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

Arrow keys drive everything (`↵`/`→` zoom in, `esc`/`←` zoom out, `space` the grid). Agents
run in durable tmux sessions, so they survive closing the UI and reattach (`a`) with full
scrollback. `⏸` flags an agent that needs you; `g` jumps to the next one.

Beyond the live views, three Phase-3 dashboards (keys `2`/`3`/`4`): a per-repo **timeline**
of commit density with cross-repo correlations, detected **work sessions** (focused vs
parallel, exportable to Markdown), and global commit **search**.

Agents: Claude Code is first-class (rich status from its transcript); Codex and Aider also
run, with a tmux-alive fallback for any kind. See [docs/agents.md](docs/agents.md).

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
# optional: install both on PATH
cargo install --path crates/repomon-tui && cargo install --path crates/repomon-daemon

repomon                                # just run it — starts the daemon if needed, then the TUI
repomon add ~/code/pos-saas            # register a repo
repomon discover ~/code --add          # or find and register many at once

# headless / scripting (also auto-start the daemon)
repomon lane list
repomon lane new --repo pos-saas --branch feat/inventory --source main
repomon lane delete feat/inventory --delete-branch
```

**`repomon` is the single command.** With no daemon running it launches a detached
`repomond` (which then survives across UI sessions), connects, and opens the TUI. If the
`repomond` binary can't be found it falls back to an in-process daemon. Use `--embedded` to
force in-process always, or manage a launchd service explicitly with
`repomon daemon install | start | stop | status | logs | uninstall` (macOS).

Testing it: `cargo build` (so both binaries exist), then `./target/debug/repomon` — or after
`cargo build --release`, `./target/release/repomon`.

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

## Documentation

- [docs/architecture.md](docs/architecture.md) — how the daemon, TUI, and core fit together.
- [docs/protocol.md](docs/protocol.md) — the JSON-RPC socket reference.
- [docs/agents.md](docs/agents.md) — how agents run and how status is detected.

## Status

The Observatory (fleet/lanes/today), the agent multiplexer (spawn, live output, input,
attach, babysit grid), and the history dashboard (timeline/sessions/search) are all in.
Deferred follow-ups: a SwiftUI menu-bar companion, an embedded PTY renderer (vs the tmux
pivot), a web dashboard, and Windows support.

## License

MIT © Ali Hamza Azam
