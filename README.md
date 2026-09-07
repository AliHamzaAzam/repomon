<p align="center">
  <img alt="repomon" src="docs/logo.png" width="96">
</p>

<h1 align="center">repomon</h1>

<p align="center">
  Run a fleet of AI coding agents across all your repositories, from one screen.
</p>

<p align="center">
  <a href="https://github.com/AliHamzaAzam/repomon/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/AliHamzaAzam/repomon?color=orange&label=release"></a>
  <img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue">
  <img alt="Platforms" src="https://img.shields.io/badge/macOS%20%C2%B7%20Linux%20%C2%B7%20Windows-555">
  <img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-Rust-orange">
</p>

<p align="center">
  <img alt="Repomon desktop app" src="docs/gui-demo.gif" width="860">
</p>

<p align="center"><sub>If Repomon is useful to you, a <a href="https://github.com/AliHamzaAzam/repomon">star on GitHub</a> helps others find it.</sub></p>

Repomon is a desktop app and a terminal UI for people who run coding agents (Claude Code, Codex,
Antigravity, OpenCode, Cursor, Aider) in several projects at once. A local daemon owns the fleet:
every repository, every worktree lane, every agent session, and the git state behind them. Agent
sessions run inside tmux (macOS, Linux) or a ConPTY host (Windows), so they survive closing the app
and reattach on the next launch. The agents that need a human float to the top.

## Features

- **Fleet view.** Repos, worktree lanes, and agents with live status: running, needs you, idle,
  rate limited. Filter, jump to the next agent that needs attention, pin what matters.
- **Terminals and multitasking.** Real terminals for each agent, focused or tiled in a grid, with
  keyboard-first navigation and a command palette.
- **Git explorer and editor.** Status, diffs, commits, and an in-app editor with syntax
  highlighting, search, file finder, and PDF and image viewers, per lane.
- **Usage and cost.** A local ledger reads every agent's transcripts, counts tokens per message,
  prices them with published rates (LiteLLM, refreshed daily) or your own overrides, and shows
  cost per agent, model, repo, lane, and session. Nothing leaves the machine.
- **Repomail and supervision.** Durable mail between agents and you, policy-based handling of
  permission dialogs, stall nudges, and a complete audit trail.
- **Repomind.** A controller lane with its own memory that can plan, run playbooks, and steer the
  rest of the fleet through the same daemon API.
- **Remote.** An optional WebSocket bridge and push notifications over Tailscale, so you can
  approve a prompt from your phone.
- **Same fleet, two clients.** The desktop app and the `repomon` TUI talk to the same daemon and
  can run side by side.

## Install

### Desktop app

The app bundles the daemon (and a portable tmux on macOS and Linux). Download from the
[latest release](https://github.com/AliHamzaAzam/repomon/releases/latest):

| Platform | Asset | Notes |
|---|---|---|
| macOS | `Repomon_<version>_universal.dmg` | Apple silicon and Intel. First launch: right-click, Open (unsigned). |
| Windows | `Repomon_<version>_x64-setup.exe` | Self-contained; no Visual C++ runtime needed. |
| Linux | `.AppImage`, `.deb`, or `.rpm` | Needs `git` and `tmux` on the PATH. |

Open the app, add a repository, and pick or create a lane. The daemon starts with the app.

### Command line

The `repomon` CLI and TUI can be installed from the app (Settings > System > Command-line tools)
or directly:

```sh
# macOS and Linux
curl -fsSL https://github.com/AliHamzaAzam/repomon/releases/latest/download/install.sh | sh

# macOS with Homebrew
brew install AliHamzaAzam/tap/repomon
brew services start repomon        # optional: run the daemon at login
```

```powershell
# Windows
irm https://github.com/AliHamzaAzam/repomon/releases/latest/download/install.ps1 | iex
```

From source, with a Rust toolchain: `cargo install --git https://github.com/AliHamzaAzam/repomon repomon-tui`.

## Quick start

```sh
repomon                     # open the TUI; it starts the daemon if needed
repomon add ~/src/app       # register a repository
repomon lane list           # lanes across every repo
repomon usage week          # tokens and cost for the last seven days
repomon daemon install      # optional: run the daemon as a user service
```

In the desktop app the same actions live in the fleet sidebar and the command palette
(`Cmd K` on macOS, `Ctrl K` elsewhere). Press `?` for the shortcut guide.

## Terminal UI

The `repomon` TUI is the original interface and stays a first-class client: the same fleet, the
same daemon, usable over SSH or alongside the app.

<p align="center">
  <img alt="Repomon terminal UI" src="docs/demo.gif" width="860">
</p>

## Documentation

| Guide | Covers |
|---|---|
| [Desktop](docs/desktop.md) | Every view, setting, and shortcut of the app |
| [Agents](docs/agents.md) | Supported agent kinds, status detection, fleet mail wiring |
| [Architecture](docs/architecture.md) | Daemon, clients, crates, data flow |
| [Daemon protocol](docs/protocol.md) | The JSON-RPC API for writing your own client |
| [Messaging](docs/messaging.md) | Repomail addresses, delivery, and the MCP tools |
| [Supervision](docs/agent-supervision.md) | Policies, dialog handling, stall nudges, audit |
| [Host protocol](crates/repomon-host/PROTOCOL.md) | The Windows agent-host control contract |

## How it compares

Tools such as Claude Squad, ccmanager, Conductor, and Crystal run parallel agents in one
repository. Repomon is for the case where you have several active projects: it spans repos,
worktrees, and agent kinds, keeps sessions alive across restarts, surfaces the ones waiting on
you, tracks usage limits and cost, and offers both a desktop app and a terminal UI over the same
fleet. If you work in a single repository, a single-repo tool may be simpler.

## Architecture

A background daemon, `repomond`, owns SQLite, file watchers, the git layer, and the agent
runtime, and exposes a JSON-RPC API over a Unix socket (macOS, Linux) or a named pipe
(Windows). The desktop app, the TUI, and the iOS companion are thin clients over that API.

| Crate or app | Role |
|---|---|
| `repomon-core` | Data model, git layer (gix), SQLite store, watchers, usage ledger, agent runtime |
| `repomon-daemon` | The `repomond` server and its background services |
| `repomon-tui` | The `repomon` terminal UI and CLI |
| `repomon-mcp` | The MCP server (`repomond mcp`) that exposes the fleet to agents |
| `repomon-host` | The per-agent ConPTY host that gives Windows tmux-style durability |
| `apps/desktop` | The Tauri desktop app, bundling the daemon and, on macOS and Linux, tmux |

## Development

```sh
cargo test --workspace                           # Rust
cd apps/desktop && bun install && bun run test   # desktop frontend
cd apps/desktop && bun tauri dev                 # run the app against your local daemon
```

Local preview bundles use `apps/desktop/src-tauri/tauri.preview.conf.json`. See
[apps/desktop/README.md](apps/desktop/README.md) for packaging and signing.

## Status

Mission Control, the TUI, usage and cost tracking, supervision, Repomind, and the remote layer
ship on macOS, Linux, and Windows. Windows is the newest platform: it ships in every release with
its own agent-host runtime, and its manual validation pass is still catching up. The iOS
companion is built and waits on an Apple Developer account.

## License

Apache-2.0. Contributions are welcome; open an issue first for anything larger than a fix.
