# repomon

**Run a fleet of AI coding agents across all your repos, from one terminal.**

Many repos × many worktrees × many agents on one screen. Durable across restarts, the ones
waiting on you float to the top, and you can approve a prompt from your phone.

<p>
  <a href="https://github.com/AliHamzaAzam/repomon/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/AliHamzaAzam/repomon?color=00b3b3&label=release"></a>
  <img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue">
  <img alt="Platforms: macOS · Linux · Windows" src="https://img.shields.io/badge/macOS%20%C2%B7%20Linux%20%C2%B7%20Windows-555">
  <img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-Rust-orange">
  <img alt="For Claude Code · Codex · Aider" src="https://img.shields.io/badge/for-Claude%20Code%20%C2%B7%20Codex%20%C2%B7%20Aider-8A2BE2">
</p>

<!-- Hero demo GIF: docs/demo.gif -->
<p align="center">
  <img src="docs/demo.gif" alt="repomon: triaging a fleet of AI coding agents across repos" width="900">
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

- **Fleet**: every agent on one screen; the ones waiting on you float to the top.
- **Split**: fleet sidebar + the selected agent's live output and an input line.
- **Babysit grid**: live tiles auto-sized to your window; watch and nudge several at once.
- **Focus**: one agent full-screen with full live terminal, input, and controls.

Arrow keys drive everything (`↵`/`→` zoom in, `esc`/`←` zoom out, `space` the grid). Agents
run in durable sessions (tmux on macOS/Linux, detached host processes on Windows), so they
survive closing the UI and reattach (`a`) with full scrollback. `⏸` flags an agent that needs
you; `g` jumps to the next one.

Beyond the live views, three Phase-3 dashboards (keys `2`/`3`/`4`): a per-repo **timeline**
of commit density with cross-repo correlations, detected **work sessions** (focused vs
parallel, exportable to Markdown), and global commit **search**.

Agents: Claude Code is first-class (rich status from its transcript); Codex and Aider also
run, with a tmux-alive fallback for any kind. See [docs/agents.md](docs/agents.md).

**Remote access**: an optional token-gated WebSocket bridge serves the same JSON-RPC API
over a private network (Tailscale). The daemon detects per-session state changes (including
interactive permission dialogs read from the pane), broadcasts them as `event.notification`,
and can push them to Apple devices via APNs directly (no relay). The bridge and protocol are
**open**, so any client can drive it today (see [docs/protocol.md](docs/protocol.md)). A
polished **iOS companion app** (fleet view, live conversations, and an Approve button for
pending dialogs) is built and ships once an Apple Developer account is in place.

## How it compares

|  | **repomon** | Claude Squad / ccmanager | GUI apps (Conductor, Crystal) | built-in `claude agents` |
|---|---|---|---|---|
| **Scope** | many repos × worktrees × agents | one repo, many worktrees | one repo, many worktrees | one tool, flat list |
| **Runtime** | durable tmux: survives close, reattach | tmux | app process | inside the CLI |
| **Triage** | needs-you float to top, `g` to jump | flat list | varies | grouped by state |
| **Usage limits** | live usage corner + auto-continue | ✗ | ✗ | ✗ |
| **Remote** | open WebSocket bridge + APNs over Tailscale (iOS app soon) | ✗ | ✗ | ✗ |
| **Lives in the terminal** | ✅ (4-zoom TUI) | ✅ | ❌ (GUI) | ✅ |

Honest take: if you work in **one** repo, Claude Squad/ccmanager or a GUI may be simpler.
repomon earns its keep once you're running agents across **several** projects at once.

## Architecture

A background daemon (`repomond`) owns SQLite, file watchers, the git layer, and the agent
runtime, exposing a JSON-RPC API over a local transport (Unix socket on macOS/Linux, named
pipe on Windows). The agent runtime sits behind a `SessionBackend` trait: tmux on macOS/Linux,
and per-agent host processes on Windows. The TUI (`repomon`) is a thin client. Five crates:

- `repomon-core`: data model, gix git layer, SQLite store, watchers, agent runtime (`SessionBackend`).
- `repomon-daemon`: the `repomond` socket/pipe server and background services.
- `repomon-tui`: the `repomon` terminal UI.
- `repomon-mcp`: repomind's MCP server (`repomond mcp`), exposing the fleet to an orchestrator agent over stdio.
- `repomon-host`: `repomon-agent-host.exe`, the per-agent ConPTY host that gives Windows tmux-style durability (Windows only).

## Install

**One line, no deps** (macOS & Linux, x86_64 / aarch64, incl. WSL2; prebuilt binaries, no Rust or Xcode):

```sh
curl -fsSL https://github.com/AliHamzaAzam/repomon/releases/latest/download/install.sh | sh
```

**Homebrew** (macOS):

```sh
brew install AliHamzaAzam/tap/repomon      # or: brew tap AliHamzaAzam/tap && brew install repomon
brew services start repomon                # optional: run the daemon at login
```

Or grab a tarball from the [latest release](https://github.com/AliHamzaAzam/repomon/releases/latest):
per-arch (`aarch64`/`x86_64`) or the `universal` build, then extract, and put `repomon` and `repomond`
on your `PATH`.

**From source**: any platform with the Rust toolchain (anywhere without a prebuilt binary):

```sh
cargo install --git https://github.com/AliHamzaAzam/repomon repomon-tui repomon-daemon
```

On macOS and Linux repomon needs `tmux` (agents run in tmux) and `git` at runtime.
Don't have `tmux`? Install it: `brew install tmux` (macOS), `sudo apt install tmux` (Debian / Ubuntu / WSL2), `sudo dnf install tmux` (Fedora), `sudo pacman -S tmux` (Arch).

**Windows** (native, no WSL, no tmux):

Native Windows support has landed. There is currently a **preview build on the
`release/windows-preview` branch** and **no published GitHub release yet**, so the
`irm | iex` one-liner below will only work once a Windows release is tagged.

Until then, build from source with the Rust toolchain (edition 2024, toolchain 1.95.0) and
a Git for Windows install:

```powershell
git clone https://github.com/AliHamzaAzam/repomon
cd repomon
git switch release/windows-preview
cargo build --release
# repomon.exe, repomond.exe and repomon-agent-host.exe land in target\release\.
# Copy all three into one directory on your PATH (they must live together).
```

Once a release is tagged, the installer downloads prebuilt binaries (no Rust toolchain
needed) and puts the three exes in `%LOCALAPPDATA%\Programs\repomon` on your user PATH:

```powershell
irm https://github.com/AliHamzaAzam/repomon/releases/latest/download/install.ps1 | iex
```

Env overrides mirror `install.sh`: `REPOMON_INSTALL_DIR` (install location) and
`REPOMON_VERSION` (a tag to pin instead of latest).

Then enable cd-on-exit by adding to your PowerShell profile (`$PROFILE`):

```powershell
repomon shell-init powershell | Out-String | Invoke-Expression
```

Then enable cd-on-exit by adding to your `~/.zshrc` (or `~/.bashrc`):

```sh
eval "$(repomon shell-init zsh)"
```

### Run the daemon as a service (optional)

The TUI auto-starts `repomond` on demand, so a service is never required. To keep the daemon
(and its notifications) alive across logins:

```sh
repomon daemon install     # macOS: launchd LaunchAgent · Linux: systemd user unit
```

On Linux this writes `~/.config/systemd/user/repomon.service`; run
`loginctl enable-linger` if you want `repomond` to survive logout.

### Linux platform notes

- Desktop notifications use `notify-send` (libnotify); the chime plays through
  `canberra-gtk-play` or `paplay` when present.
- Clipboard copy uses `wl-copy` (Wayland) or `xclip` (X11); inside tmux, drag-select falls
  back to OSC52 when neither is installed. Image paste needs `wl-paste` or `xclip`.
- Click-to-focus notifications are macOS-only (`terminal-notifier`).

### Windows platform notes

- **No tmux, no WSL.** On Windows repomon runs natively. Each agent runs in its own detached
  host process (`repomon-agent-host.exe`, a ConPTY + server-side terminal emulator) that plays
  exactly the durability role tmux plays on Unix: agents survive daemon restarts and re-adopt
  with full scrollback. The daemon talks to the TUI and to the hosts over named pipes instead
  of Unix sockets.
- **Windows Terminal recommended.** repomon runs in any modern console, but Windows Terminal
  gives the best rendering and is where the pop-out attach opens a new tab.
- **ConPTY floor: Windows 10 1809.** The host relies on ConPTY, which requires Windows 10
  version 1809 or newer (Windows 11 fully supported). Claude Code on native Windows needs Git
  for Windows.
- **Keep the three exes together.** `repomon.exe`, `repomond.exe`, and `repomon-agent-host.exe`
  must live in the **same directory**; the daemon spawns the host by looking next to itself.
  `install.ps1` and the release zip already place all three together.
- **Unsigned binaries → SmartScreen.** The release binaries are not yet code-signed, so
  Windows SmartScreen may warn on first run ("Windows protected your PC"). Choose *More info →
  Run anyway*, or unblock the zip before extracting (`Unblock-File`). Signing is on the
  roadmap.

## Usage

```sh
repomon                                # just run it: starts the daemon if needed, then the TUI
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

## repomind — fleet orchestrator

repomind is an orchestrator agent for the fleet: a `claude` session wired to repomon's own
MCP server, so it can read every lane's status and act on your behalf — spawn workers, answer
their permission prompts, and merge finished work — while you supervise or check in only when
it needs you.

```sh
repomon orchestrate [--autonomy read-only|supervised|autonomous] [--max-agents N] [--model m] [prompt]
```

This makes sure the daemon is up, starts (or reuses) the single daemon-owned `orchestrator`
tmux window running `claude`, and attaches you to it. `prompt` is an optional initial goal.

**TUI command-center** (`O` key, or `6`): a pinned fleet row plus a dashboard for repomind,
reachable like any other zoom level. The row and header escalate the moment repomind needs
you — a permission/decision dialog, or an end-of-turn wait — and fire a "repomind needs you"
desktop notification when the TUI isn't already looking at it. Press `i` to type straight to
repomind without leaving the view (mediated `send-keys`); `↵`/`→` attaches to its real tmux
pane instead.

**Guardrails.** By product decision, `--autonomy` defaults to `autonomous` — repomind may
create, merge, and delete lanes and run a goal end-to-end without asking first — bounded by a
few hard caps enforced server-side (not just requested in the prompt): a per-session action
cap (100 actions by default), a concurrent-agent cap (`--max-agents`, default 4), a 15s dedupe
on sending the same text to the same lane twice in a row, and a two-phase human-confirmation
flow for lane deletion (the first call only returns an impact summary and a token; the delete
only happens once that token comes back). Pass `--autonomy supervised` to have it propose lane
creation for you to confirm instead, or `--autonomy read-only` to keep it to observing.

Before merging a lane's work, repomind is expected to verify it: `lane_diff` (commits ahead of
base with diffstat, plus uncommitted changes) before `merge_lane` lands them.

## Shell integration (cd-on-exit)

Pressing `c` on a lane exits repomon and changes your shell into that worktree. repomon
writes the path to the file descriptor in `$REPOMON_CD_FD`; add the wrapper to your
`~/.zshrc` / `~/.bashrc` so the shell acts on it:

```sh
eval "$(repomon shell-init zsh)"   # bash: repomon shell-init bash · fish: repomon shell-init fish
```

On **Windows / PowerShell** the wrapper reads the path from a temp file (`$REPOMON_CD_FILE`)
instead of an inherited file descriptor; add it to your `$PROFILE`:

```powershell
repomon shell-init powershell | Out-String | Invoke-Expression
```

## Remote access (open bridge over Tailscale)

The daemon serves the same JSON-RPC API over a token-gated WebSocket bridge, so you can drive
it from any client; the protocol is documented in [docs/protocol.md](docs/protocol.md). A
native **iOS companion app** (fleet view, live conversations, Approve button) is built and
ships once an Apple Developer account is in place; until then the bridge and `remote pair`
pairing work for any client you point at them. Bind it to your **private tailnet** address,
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

- [docs/architecture.md](docs/architecture.md): how the daemon, TUI, and core fit together.
- [docs/protocol.md](docs/protocol.md): the JSON-RPC socket reference.
- [docs/agents.md](docs/agents.md): how agents run and how status is detected.
- [docs/windows-validation.md](docs/windows-validation.md): the manual Windows 11 end-to-end validation gate.
- [crates/repomon-host/PROTOCOL.md](crates/repomon-host/PROTOCOL.md): the frozen Windows agent-host control protocol.

## Status

The fleet view (lanes/today), the agent multiplexer (spawn, live output, input, attach,
babysit grid, multi-agent lanes), the history dashboard (timeline/sessions/search),
per-session notifications (pane-sniffed permission-dialog detection, fired as desktop popups
even when the TUI is closed or parked full-screen in an agent), the remote access layer
(WebSocket bridge + APNs + pairing), and repomind (the MCP-driven fleet orchestrator —
`repomon orchestrate` and the TUI command-center) are all in, on macOS, Linux, and Windows.
Each platform has native paths for the service, notifications, clipboard, and process/agent
liveness. **Native Windows support has landed** (code-complete and CI-green on
`x86_64-pc-windows-msvc`): a `SessionBackend` trait with a tmux backend on Unix and a
host-process backend on Windows, named-pipe IPC, and `repomon-agent-host.exe` for
durability parity. It still awaits a physical Windows 11 end-to-end pass and binary signing
before a Windows release is tagged (see [docs/windows-validation.md](docs/windows-validation.md)).
The iOS companion app is built and ships once an Apple Developer account is in place.
Deferred: a web dashboard.

---

If repomon saves you a few context-switches a day, a ⭐ helps other people find it.

## License

Apache-2.0 © Ali Hamza Azam
