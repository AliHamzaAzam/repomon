# repomon

**Run a fleet of AI coding agents across all your repos — from one terminal.**

Many repos × many worktrees × many agents on one screen — durable in tmux, the ones waiting
on you float to the top, and you can approve a prompt from your phone.

<p>
  <a href="https://github.com/AliHamzaAzam/repomon/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/AliHamzaAzam/repomon?color=00b3b3&label=release"></a>
  <img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue">
  <img alt="Platforms: macOS · Linux" src="https://img.shields.io/badge/macOS%20%C2%B7%20Linux-555">
  <img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-Rust-orange">
  <img alt="For Claude Code · Codex · Aider" src="https://img.shields.io/badge/for-Claude%20Code%20%C2%B7%20Codex%20%C2%B7%20Aider-8A2BE2">
</p>

<!-- Hero demo GIF: docs/demo.gif -->
<p align="center">
  <img src="docs/demo.gif" alt="repomon — triaging a fleet of AI coding agents across repos" width="900">
</p>

Other tools run parallel agents in *one repo, many worktrees* (Claude Squad, Conductor,
Crystal, ccmanager). repomon is built for the developer juggling **5–15 active projects** with
a fleet of agents running at once: **many repos × many worktrees × many agents**, spawned and
steered from one place.

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

**Remote access**: an optional token-gated WebSocket bridge serves the same JSON-RPC API
over a private network (Tailscale). The daemon detects per-session state changes — including
interactive permission dialogs read from the pane — broadcasts them as `event.notification`,
and can push them to Apple devices via APNs directly (no relay). The bridge and protocol are
**open**, so any client can drive it today (see [docs/protocol.md](docs/protocol.md)). A
polished **iOS companion app** — fleet view, live conversations, and an Approve button for
pending dialogs — is built and ships once an Apple Developer account is in place.

## How it compares

|  | **repomon** | Claude Squad / ccmanager | GUI apps (Conductor, Crystal) | built-in `claude agents` |
|---|---|---|---|---|
| **Scope** | many repos × worktrees × agents | one repo, many worktrees | one repo, many worktrees | one tool, flat list |
| **Runtime** | durable tmux — survives close, reattach | tmux | app process | inside the CLI |
| **Triage** | needs-you float to top, `g` to jump | flat list | varies | grouped by state |
| **Usage limits** | live usage corner + auto-continue | — | — | — |
| **Remote** | open WebSocket bridge + APNs over Tailscale (iOS app soon) | — | — | — |
| **Lives in the terminal** | ✅ (4-zoom TUI) | ✅ | ❌ (GUI) | ✅ |

Honest take: if you work in **one** repo, Claude Squad/ccmanager or a GUI may be simpler.
repomon earns its keep once you're running agents across **several** projects at once.

## Architecture

A background daemon (`repomond`) owns SQLite, file watchers, the git layer, and the
tmux-backed agent runtime, exposing a JSON-RPC API over a Unix socket. The TUI (`repomon`)
is a thin client. Three crates:

- `repomon-core` — data model, gix git layer, SQLite store, watchers, agent runtime.
- `repomon-daemon` — `repomond`: the socket server and background services.
- `repomon-tui` — `repomon`: the terminal UI.

## Install

**One line, no deps** (macOS — prebuilt binaries, no Rust or Xcode):

```sh
curl -fsSL https://github.com/AliHamzaAzam/repomon/releases/latest/download/install.sh | sh
```

**Homebrew** (macOS):

```sh
brew install AliHamzaAzam/tap/repomon      # or: brew tap AliHamzaAzam/tap && brew install repomon
brew services start repomon                # optional: run the daemon at login
```

Or grab a tarball from the [latest release](https://github.com/AliHamzaAzam/repomon/releases/latest) —
per-arch (`aarch64`/`x86_64`) or the `universal` build — extract, and put `repomon` and `repomond`
on your `PATH`.

**From source** — Linux or macOS (needs the Rust toolchain). This is the Linux install path
(prebuilt binaries are macOS-only for now):

```sh
cargo install --git https://github.com/AliHamzaAzam/repomon repomon-tui repomon-daemon
```

repomon needs `tmux` (agents run in tmux) and `git` at runtime. Then enable cd-on-exit by adding to
your `~/.zshrc` (or `~/.bashrc`):

```sh
eval "$(repomon shell-init zsh)"
```

## Usage

```sh
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
force in-process always, or manage the daemon with
`repomon daemon start | stop | restart | status | logs | install | uninstall`.

> **Building from source?** After a rebuild, run `repomon daemon restart` so the new code is
> served (the daemon outlives the UI). The dev build runs from `./target/debug/repomon`.

## Shell integration (cd-on-exit)

Pressing `c` on a lane exits repomon and changes your shell into that worktree. repomon
writes the path to the file descriptor in `$REPOMON_CD_FD`; add the wrapper to your
`~/.zshrc` / `~/.bashrc` so the shell acts on it:

```sh
eval "$(repomon shell-init zsh)"   # bash: repomon shell-init bash · fish: repomon shell-init fish
```

## Remote access (open bridge over Tailscale)

The daemon serves the same JSON-RPC API over a token-gated WebSocket bridge, so you can drive
it from any client — the protocol is documented in [docs/protocol.md](docs/protocol.md). A
native **iOS companion app** (fleet view, live conversations, Approve button) is built and
ships once an Apple Developer account is in place; until then the bridge and `remote pair`
pairing work for any client you point at them. Bind it to your **private tailnet** address —
never a public interface; anyone holding the token can read your panes and type into your agents.

1. **Install [Tailscale](https://tailscale.com)** on the Mac (and any device you'll connect
   from), signed into the same tailnet, so it can reach the Mac at its `100.x.y.z` address.
2. **Enable the bridge**, then restart the daemon to apply:
   ```sh
   repomon remote enable     # detects the Tailscale IPv4, binds ws://<ip>:7878, mints a token
   repomon daemon restart
   ```
   No Tailscale detected? Pass the address yourself: `repomon remote enable --bind <ip:port>`.
3. **Pair a client:** `repomon remote pair` prints a QR (and a `repomon://<host:port>#<token>`
   link) for a client to connect.

Manage it with `repomon remote status` (shows the bind and a masked token),
`repomon remote enable --rotate-token` (mint a new token, then re-pair), and
`repomon remote disable` (stops serving; keeps the token). Each change needs a
`repomon daemon restart` to take effect.

## Documentation

- [docs/architecture.md](docs/architecture.md) — how the daemon, TUI, and core fit together.
- [docs/protocol.md](docs/protocol.md) — the JSON-RPC socket reference.
- [docs/agents.md](docs/agents.md) — how agents run and how status is detected.

## Status

The fleet view (lanes/today), the agent multiplexer (spawn, live output, input, attach,
babysit grid, multi-agent lanes), the history dashboard (timeline/sessions/search),
per-session notifications (pane-sniffed permission-dialog detection, fired as desktop popups
even when the TUI is closed or parked full-screen in an agent), and the remote access layer
(WebSocket bridge + APNs + pairing) are all in. The iOS companion app is built and ships once
an Apple Developer account is in place. Deferred: an embedded PTY renderer (vs the tmux pivot),
a web dashboard, and Windows support.

---

If repomon saves you a few context-switches a day, a ⭐ helps other people find it.

## License

Apache-2.0 © Ali Hamza Azam
