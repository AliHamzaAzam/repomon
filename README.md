# repomon

**Download Repomon to run a fleet of AI coding agents across your repositories.**
Many repos, worktrees and agents share one desktop or terminal view; sessions survive app and daemon restarts.

**You are here:** install the app, try one lane, then open the guide for your next task.

<p align="center">
  <img alt="repomon" src="docs/logo.png" width="104">
</p>

<p>
  <a href="https://github.com/AliHamzaAzam/repomon/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/AliHamzaAzam/repomon?color=orange&label=release"></a>
  <img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue">
  <img alt="Platforms: macOS · Linux · Windows" src="https://img.shields.io/badge/macOS%20%C2%B7%20Linux%20%C2%B7%20Windows-555">
  <img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-Rust-orange">
  <img alt="For Claude Code · Codex · Antigravity · OpenCode · Cursor · Aider" src="https://img.shields.io/badge/for-Claude%20Code%20%C2%B7%20Codex%20%C2%B7%20Antigravity%20%C2%B7%20OpenCode%20%C2%B7%20Cursor%20%C2%B7%20Aider-8A2BE2">
</p>

## Install in 3 steps

Choose your OS below. **Time estimates assume you already have Git and an agent CLI installed; allow another 5-10 minutes if either is missing.**

### macOS

Download the universal app, then open it.

**Time: 3-5 minutes.**

1. Download `Repomon_<version>_universal.dmg` from the [latest release](https://github.com/AliHamzaAzam/repomon/releases/latest). The universal build supports Apple silicon and Intel.
2. Open the DMG and drag Repomon to Applications.
3. Open Repomon and add a repository in onboarding.

**You know it worked when:** the repository and its main lane appear in the fleet.

### Windows

Download the x64 installer, then follow its prompts.

**Time: 3-5 minutes.**

1. Download `Repomon_<version>_x64-setup.exe` from the [latest release](https://github.com/AliHamzaAzam/repomon/releases/latest).
2. Run the installer and open Repomon.
3. Add a repository in onboarding.

**You know it worked when:** the repository appears and the connection indicator shows a connected daemon.

### Linux

Choose the package format your system supports.

**Time: 3-5 minutes.**

1. Download an AppImage, `.deb`, or `.rpm` from the [latest release](https://github.com/AliHamzaAzam/repomon/releases/latest).
2. Install the package, or run the AppImage commands below. Replace the filename with your downloaded file.

   ```sh
   chmod +x ./Repomon_<version>_amd64.AppImage
   ./Repomon_<version>_amd64.AppImage
   ```

   For a Debian package instead:

   ```sh
   sudo apt install ./Repomon_<version>_amd64.deb
   ```

3. Open Repomon and add a repository.

**You know it worked when:** the repository and its main lane appear in the fleet.

<details>
<summary>Details: bundled tools and updates</summary>

The bundle carries its own daemon, its own portable `tmux` (macOS and Linux; Windows uses the
built-in ConPTY host instead), and the `repomon` command line.

Git and your chosen agent CLI must be available; onboarding identifies missing tools.

Windows binaries link the C runtime statically, so no Visual C++ redistributable is needed.

The app updates itself after the first download (**Settings > General > Check for updates**).

See [docs/desktop.md](docs/desktop.md).

</details>

## First five minutes

Start one agent and inspect its result.

**Time: 5 minutes, excluding the agent’s work.**

1. Add one repository through onboarding or the sidebar’s **Add repository** button.
2. Select its main lane, or use **New Lane** to create a worktree for a branch.
3. Choose an installed agent and give it one task.
4. Open **Git** with `mod+1` to inspect the changes.
5. Use **Needs you** to find an agent waiting for input and answer in its pane.

**You know it worked when:** the fleet shows your repo, the agent’s state, and the resulting diff. `mod` is Cmd on macOS and Ctrl elsewhere.

<p align="center">
  <img src="docs/gui-demo.gif" alt="Repomon desktop demonstration with sample repositories" width="900">
</p>

## Choose your next task

Open the guide for the feature you want to use.

| Task | What it does | Guide |
|---|---|---|
| Triage the fleet | Groups repo/worktree lanes, shows agent rosters and floats work needing you | [Desktop sidebar](docs/desktop.md#the-sidebar) |
| Inspect Git changes | Shows branch status, per-file stats, history and full commit patches | [Git explorer](docs/desktop.md#git-explorer) |
| Edit files | Shares a themed CodeMirror editor, file tree, dirty tabs and conflict-safe saves | [Editor](docs/desktop.md#editor-workspace) |
| Track tokens and costs | Reads a local ledger, edits model rates and recounts older transcripts | [Usage](docs/desktop.md#usage) |
| Recover the daemon | Checks dependencies, manages the daemon and restores orphaned agents | [System settings](docs/desktop.md#settings) |
| Send fleet mail | Addresses an agent, a lane or the fleet with per-recipient results | [Messaging](docs/messaging.md#cli-and-terminal-ui) |
| Supervise a lane | Applies bounded permission rules and records an audit trail | [Supervision recipes](docs/agent-supervision.md#live-recipes) |
| Coordinate with Repomind | Runs a cross-kind orchestrator; functional, still work in progress | [Repomind](docs/desktop.md#repomind) |
| Use native Windows controls | Provides one title bar, ConPTY sessions and bundled CLI installation | [Windows validation](docs/windows-validation.md) |
| Pair a remote client | Uses a private Tailscale WebSocket bridge and APNs | [Remote setup](#remote-access-open-bridge-over-tailscale) |

## Contributing

Choose the component and its validation path before editing.

**Time: 5 minutes to choose a starting point; implementation time depends on the change.**

1. Read [Architecture](docs/architecture.md) to find the owning component.
2. Use the [desktop development guide](apps/desktop/README.md) for frontend changes or the [Windows gate](docs/windows-validation.md) for platform validation.
3. Include the commands you ran and their results with your change.

**You know it worked when:** your change names its owning component and includes relevant validation evidence.

If Repomon saves you context switches, star the repository to help other people find it.

## License

Read [LICENSE](LICENSE) for the Apache-2.0 terms.

Apache-2.0 © Ali Hamza Azam

## Find installation and reference details

Jump to the task or reference you need.

| Section | Go to |
|---|---|
| Install in 3 steps | [Open section](#install-in-3-steps) |
| First five minutes | [Open section](#first-five-minutes) |
| Choose your next task | [Open section](#choose-your-next-task) |
| Command line (TUI + headless `repomon`) | [Open section](#command-line-tui--headless-repomon) |
| Run the daemon as a service (optional) | [Open section](#run-the-daemon-as-a-service-optional) |
| Linux platform notes | [Open section](#linux-platform-notes) |
| Windows platform notes | [Open section](#windows-platform-notes) |
| Usage | [Open section](#usage) |
| repomind (fleet orchestrator, work in progress) | [Open section](#repomind-fleet-orchestrator-work-in-progress) |
| Remote access (open bridge over Tailscale) | [Open section](#remote-access-open-bridge-over-tailscale) |
| Prefer the terminal? The original TUI | [Open section](#prefer-the-terminal-the-original-tui) |
| How it compares | [Open section](#how-it-compares) |
| Architecture | [Open section](#architecture) |
| Mission Control | [Open section](#mission-control) |
| Documentation | [Open section](#documentation) |
| Status | [Open section](#status) |
| Contributing | [Open section](#contributing) |
| License | [Open section](#license) |

## Command line (TUI + headless `repomon`)

Install from the app or choose one alternative below. **Time: 1 minute from the app, 2-5 minutes for a prebuilt CLI, or 10-20 minutes for a first source build.**

1. From the app, open **Settings > System > Command-line tools > Install**. Without the app, expand the installation details and choose one method.
2. Complete that method, then open a new terminal.
3. Check the installed version.

   ```sh
   repomon --version
   ```

**You know it worked when:** `repomon --version` prints the installed version in a newly opened terminal.

<details>
<summary>Details: app, GitHub, Homebrew and source installation</summary>

**From the app** (nothing to download): **Settings > System > Command-line tools > Install**, or
the last step of the first-run setup wizard.

On macOS and Linux this links `repomon` and `repomond` into `~/.local/bin` (Linux AppImage
installs copy them out of the temporary mount instead, so reinstall the tools after app
updates); on Windows it copies `repomon.exe`, `repomond.exe`, and `repomon-agent-host.exe` into
`%LOCALAPPDATA%\repomon\bin` and puts that directory on your user PATH (never the machine PATH).

The card reports the installed version, whether your shell can find it, and the exact line to
add to your shell rc when it cannot.

A shell PATH probe that takes more than two seconds is reported as unknown.

Existing regular binaries in `~/.local/bin` are backed up as `<name>.bak` and restored when you
remove the app-installed tools.

**From GitHub**, macOS / Linux:

```sh
curl -fsSL https://github.com/AliHamzaAzam/repomon/releases/latest/download/install.sh | sh
```

Homebrew (macOS):

```sh
brew install AliHamzaAzam/tap/repomon      # or: brew tap AliHamzaAzam/tap && brew install repomon
brew services start repomon                # optional: run the daemon at login
```

Needs `tmux` and `git` on the system (the desktop bundle ships its own; the CLI does not).

No tmux?

`brew install tmux` (macOS), `sudo apt install tmux` (Debian/Ubuntu/WSL2), `sudo dnf install
tmux` (Fedora), `sudo pacman -S tmux` (Arch).

No prebuilt binary for your platform: `cargo install --git
https://github.com/AliHamzaAzam/repomon repomon-tui repomon-daemon`.

Enable cd-on-exit (optional): add to `~/.zshrc` or `~/.bashrc`:

```sh
eval "$(repomon shell-init zsh)"   # bash: repomon shell-init bash · fish: repomon shell-init fish
```

**From GitHub, Windows CLI**, PowerShell:

```powershell
irm https://github.com/AliHamzaAzam/repomon/releases/latest/download/install.ps1 | iex
```

Puts `repomon.exe`, `repomond.exe`, and `repomon-agent-host.exe` in
`%LOCALAPPDATA%\Programs\repomon` on your user PATH (env overrides: `REPOMON_INSTALL_DIR`,
`REPOMON_VERSION` to pin a tag instead of latest).

Then enable cd-on-exit by adding to your PowerShell profile (`$PROFILE`):

```powershell
repomon shell-init powershell | Out-String | Invoke-Expression
```

</details>

## Run the daemon as a service (optional)

Install a login service if you want notifications when both UIs are closed.

**Time: 1 minute.**

1. Install the login service with the command below.
2. Run `repomon daemon status` to check it.

**You know it worked when:** status reports the installed service. The app and CLI also auto-start the daemon without a service.

```sh
repomon daemon install
repomon daemon status
```

<details>
<summary>Details</summary>

Both the CLI and the desktop app auto-start `repomond` on demand, so a service is never
required.

To keep the daemon (and its notifications) alive across logins even with neither UI open:

```sh
repomon daemon install     # macOS: launchd LaunchAgent · Linux: systemd user unit
```

On Linux this writes `~/.config/systemd/user/repomon.service`; run `loginctl enable-linger` if
you want `repomond` to survive logout.

</details>

## Linux platform notes

Install the optional clipboard or notification helper for the behavior you need.

<details>
<summary>Details</summary>

- Desktop notifications use `notify-send` (libnotify); the chime plays through
  `canberra-gtk-play` or `paplay` when present.
- Clipboard copy uses `wl-copy` (Wayland) or `xclip` (X11); inside tmux, drag-select falls
  back to OSC52 when neither is installed. Image paste needs `wl-paste` or `xclip`.
- Click-to-focus notifications are macOS-only (`terminal-notifier`).

</details>

## Windows platform notes

Use Windows 10 version 1809 or newer and keep the three CLI executables together.

<details>
<summary>Details</summary>

- **No tmux, no WSL.** On Windows repomon runs natively. Each agent runs in its own detached
  host process (`repomon-agent-host.exe`, a ConPTY + server-side terminal emulator) that plays
  exactly the durability role tmux plays on Unix: agents survive daemon restarts and re-adopt
  with full scrollback. The daemon talks to its clients and to the hosts over named pipes
  instead of Unix sockets.
- **Windows Terminal recommended** for the CLI. The desktop app renders its own terminals and
  doesn't depend on the host console.
- **ConPTY floor: Windows 10 1809.** The host relies on ConPTY, which requires Windows 10
  version 1809 or newer (Windows 11 fully supported). Claude Code on native Windows needs Git
  for Windows.
- **Keep the three CLI exes together.** `repomon.exe`, `repomond.exe`, and
  `repomon-agent-host.exe` must live in the **same directory**; the daemon spawns the host by
  looking next to itself. `install.ps1`, the release zip, and **Settings > System >
  Command-line tools** already place all three together. (The desktop bundle carries its own
  copies and doesn't need this.)
- **No Visual C++ redistributable required.** The Windows binaries link the C runtime statically.
  Older builds did not, and on a machine without the redistributable `repomond.exe` died in the
  loader with `0xC0000135` before writing anything to its log, leaving the app's connection pill
  stuck on "Retrying". If you are on such a build, **Settings > System > Bundled Daemon** names
  the cause and links the fix.

</details>

## Usage

Run `repomon` to open the TUI, or choose a headless command below.

```sh
repomon                                # just run it: starts the daemon if needed, then the TUI
repomon add ~/code/pos-saas            # register a repo
repomon discover ~/code --add          # or find and register many at once

# headless / scripting (also auto-start the daemon)
repomon lane list
repomon lane new --repo pos-saas --branch feat/inventory --source main
repomon lane delete feat/inventory --delete-branch
```

<details>
<summary>Details</summary>

Correct model prices in **Settings > Usage** or with `repomon usage rates set/reset`; overrides
re-price usage immediately without a restart.

**`repomon` is the single command.** With no daemon running it launches a detached `repomond`
(which then survives across UI sessions), connects, and opens the TUI.

If the `repomond` binary can't be found it falls back to an in-process daemon.

Use `--embedded` to force in-process always, or manage the daemon with `repomon daemon start |
stop | restart | status | logs | install | uninstall`.

> **Building from source?** After a rebuild, run `repomon daemon restart` so the new code is
> served (the daemon outlives the UI).

The dev build runs from `./target/debug/repomon`.

</details>

## repomind (fleet orchestrator, work in progress)

Open the Repomind panel with `mod+9`, or start it from the CLI. Read the autonomy limits before delegating.

```sh
repomon orchestrate --autonomy supervised
```

**Time: 1-2 minutes, plus agent startup.** **You know it worked when:** the controller has a pane in the Repomind home lane.

<details>
<summary>Details: memory, CLI, TUI and guardrails</summary>

**Status: functional, not polished.** repomind works today, but expect rough edges: guardrail
behavior under real-world load isn't fully proven, and the UX (panel, notifications, dashboard)
is still catching up to the daemon-side feature set.

Treat it as an early feature, not a finished one.

repomind is an orchestrator agent for the fleet: a coding-agent session (Claude Code, Codex,
Antigravity, or OpenCode) wired to repomon's own MCP server, so it can read every lane's status
and act on your behalf, spawning workers, answering their permission prompts, and merging
finished work, while you supervise or check in only when it needs you.

**A home, not a window.** repomind's memory lives in its own git repo, `~/repomind` by default
(`[repomind] home`).

The daemon creates it on first use, never deletes or merges it itself, and registers it like any
other repo, with its main worktree marked the *controller lane*: the one lane every controller
agent runs in, holding the full fleet catalog instead of a worker's restricted one.

A worker agent cannot spawn into it, and up to `[repomind] max_controllers` controllers (2 by
default) may run there at once.

```
~/repomind/
  AGENTS.md              protocol every agent that runs here follows
  REPOMIND.md            the operator's persona overlay: voice, defaults, house rules
  profile/               standing facts about the fleet: repos, lanes, agents, quotas
  plans/{active,standing,done}/   goals in flight, standing orchestrations, closed goals
  playbooks/              approved procedures, drafts under playbooks/drafts/
  fleet/<repo>/notes.md   per-repo notes
  journal/YYYY-MM-DD.md   a daily digest exported from the orchestration journal
  knowledge/              cross-cutting facts
  .repomind/              daemon-owned: the assembled boot context, export state, locks
```

**How memory flows.** SQLite stays canonical for what the daemon writes itself (the journal,
schedules, approval rules): a one-way, debounced export renders those rows into the files above
and commits the batch to the home repo as `Repomind <repomind@local>`.

Repo notes and playbooks are file-first instead - the `repo.notes.*` and `playbook.*` RPCs read
and write `fleet/<repo>/notes.md` and `playbooks/<name>.md` directly, and approving a playbook
moves its file out of `drafts/`.

Claude controllers also get the home over basic-memory, registered as a second project beside
your own vault, so `search_notes`/`read_note`/`write_note` work the same way there; other
backends (Codex, Antigravity, OpenCode) read and write the files directly per `AGENTS.md`.

**Boot context.** A controller starts with no memory of the fleet.

Before every spawn into the controller lane, and on demand via `repomon repomind boot`, the
daemon assembles `~/repomind/.repomind/boot.md`: the `REPOMIND.md` overlay, the `profile/*`
notes, one status line per active plan, yesterday's and today's journal, and a one-line-per-lane
fleet snapshot, bounded to a token budget with a trailing line naming anything it had to drop to
fit.

It is daemon-owned and gitignored; never hand-edit it.

**Sidebar and panel.** A pinned Repomind row sits above the fleet sidebar's repo groups (brain
icon, state, controller count, active-goal count); the repomind home itself is excluded from the
ordinary repo groups and their counts.

The Repomind panel (`⌘9`) is the detail view: agents, active plans, a journal tail, and mail.

**CLI.**

```sh
repomon repomind status            # home, lane, window, controller cap, export state, counts, boot
repomon repomind boot              # regenerate the boot context; prints its path, size, and what trimmed
repomon repomind export            # run the one-way export now instead of waiting out its debounce
repomon repomind open              # print the home path
repomon repomind open --editor     # open it in $EDITOR
```

It's built into Mission Control (the repomind panel, `⌘9`) and started from the CLI:

```sh
repomon orchestrate --autonomy supervised --max-agents 4 "Review the fleet"
```

This makes sure the daemon is up, ensures the home and its controller lane exist, spawns (or
reuses) the primary controller there, and attaches you to it.

The positional prompt is an optional initial goal.

Choose `--autonomy read-only`, `--autonomy supervised` or `--autonomy autonomous`; `--max-agents N` caps workers and `--model m` selects a model.

**TUI command-center** (`O` key, or `6`): a pinned fleet row plus a dashboard for repomind,
reachable like any other zoom level.

The row and header escalate the moment repomind needs you, a permission/decision dialog, or an
end-of-turn wait, and fire a "repomind needs you" desktop notification when the TUI isn't
already looking at it.

Press `i` to type straight to repomind without leaving the view (mediated `send-keys`); `↵`/`→`
attaches to its real tmux pane instead.

**Guardrails.** By product decision, `--autonomy` defaults to `autonomous`, repomind may create,
merge, and delete lanes and run a goal end-to-end without asking first, bounded by a few hard
caps enforced server-side (not just requested in the prompt): a per-session action cap (100
actions by default), a concurrent-agent cap (`--max-agents`, default 4), a 15s dedupe on sending
the same text to the same lane twice in a row, and a two-phase human-confirmation flow for lane
deletion (the first call only returns an impact summary and a token; the delete only happens
once that token comes back).

Pass `--autonomy supervised` to have it propose lane creation for you to confirm instead, or
`--autonomy read-only` to keep it to observing.

The controller lane is a hard exception regardless of autonomy: repomind may never delete or
merge it, and destructive dialogs there (deletion, a push to remote, credential or device
access, installs) always hold for you.

Before merging a lane's work, repomind is expected to verify it: `lane_diff` (commits ahead of
base with diffstat, plus uncommitted changes) before `merge_lane` lands them.

</details>

## Remote access (open bridge over Tailscale)

Install Tailscale on both devices before enabling the bridge. **Time: 5-10 minutes.**

The daemon serves the same JSON-RPC API over a token-gated WebSocket bridge, so you can drive it
from any client; the protocol is documented in [docs/protocol.md](docs/protocol.md).

A native **iOS companion app** (fleet view, live conversations, Approve button) is built and
ships once an Apple Developer account is in place; until then the bridge and `remote pair`
pairing work for any client you point at them.

Bind it to your **private tailnet** address, never a public interface; anyone holding the token
can read your panes and type into your agents.

1. **Install [Tailscale](https://tailscale.com)** on the machine (and any device you'll connect
   from), signed into the same tailnet, so it can reach it at its `100.x.y.z` address.
2. **Enable the bridge**, then restart the daemon to apply:

   ```sh
   repomon remote enable     # detects the Tailscale IPv4, binds ws://<ip>:7878, mints a token
   repomon daemon restart
   ```

   No Tailscale detected? Pass the address yourself: `repomon remote enable --bind <ip:port>`.
3. **Pair a client:** `repomon remote pair` prints a QR (and a `repomon://<host:port>#<token>`
   link) for a client to connect.

Manage it with `repomon remote status` (shows the bind and a masked token), `repomon remote
enable --rotate-token` (mint a new token, then re-pair), and `repomon remote disable` (stops
serving; keeps the token).

Each change needs a `repomon daemon restart` to take effect.

**You know it worked when:** the paired client can read the fleet through the private tailnet address.

## Prefer the terminal? The original TUI

Run `repomon`, select a lane, then press Enter to zoom in or Space for the grid.

<p align="center">
  <img src="docs/demo.gif" alt="repomon TUI: triaging a fleet of AI coding agents across repos" width="800">
</p>

<details>
<summary>Details: zoom levels, dashboards and shell integration</summary>

repomon started as a terminal UI, and it's still a first-class client of the same daemon, not a
legacy mode: same fleet, same lanes, same agents, whichever one you have open.

```
REPOMON                                              14:02 fri 29 may 2026
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
FLEET   8 agents · 4 repos · 3 need you                    ↑ sorted: needs-you
─────────────────────────────────────────────────────────────────────────

  pos-saas ────────────────────────────────────────────────────────────
  ! wt-checkout  hotfix/checkout-bug     claude  needs you   89↻   3m
  > main         feat/supabase-migration claude  running    142↻  18m
  ○ wt-ui        spike/new-pos-ui                idle              2h

  montage-ai ──────────────────────────────────────────────────────────
  ! wt-mcp       spike/mcp-batch         codex   needs you   44↻   8m
  > main         phase-2-studio-floor    claude  running    201↻   2m

  ↑↓ select   ↵/→ open   spc babysit   n new-lane   / filter   g needs-you   q
```

repomon is one tool with four **zoom levels**, one selection that follows you the whole way:

- **Fleet**: every agent on one screen; the ones waiting on you float to the top.
- **Split**: fleet sidebar + the selected agent's live output and an input line.
- **Babysit grid**: live tiles auto-sized to your window; watch and nudge several at once.
- **Focus**: one agent full-screen with full live terminal, input, and controls.

Arrow keys drive everything (`↵`/`→` zoom in, `esc`/`←` zoom out, `space` the grid).

`!` flags an agent that needs you; `g` jumps to the next one.

Beyond the live views, three dashboards (keys `2`/`3`/`4`): a per-repo **timeline** of commit
density with cross-repo correlations, detected **work sessions** (focused vs parallel,
exportable to Markdown), and global commit **search**.

**Shell integration (cd-on-exit).** Pressing `c` on a lane exits repomon and changes your shell
into that worktree. repomon writes the path to the file descriptor in `$REPOMON_CD_FD`; add the
wrapper to your `~/.zshrc` / `~/.bashrc` so the shell acts on it:

```sh
eval "$(repomon shell-init zsh)"   # bash: repomon shell-init bash · fish: repomon shell-init fish
```

On **Windows / PowerShell** the wrapper reads the path from a temp file (`$REPOMON_CD_FILE`)
instead of an inherited file descriptor; add it to your `$PROFILE`:

```powershell
repomon shell-init powershell | Out-String | Invoke-Expression
```

</details>

## How it compares

Compare scope first: Repomon targets developers running agents across 5-15 active projects.

<details>
<summary>Details: comparison and single-repo tradeoff</summary>

Other tools run parallel agents in one repo with many worktrees: Claude Squad, Conductor,
Crystal and ccmanager.

Repomon spans many repos, worktrees and agents in either a desktop app or a terminal.

|  | **repomon** | Claude Squad / ccmanager | GUI apps (Conductor, Crystal) | built-in `claude agents` |
|---|---|---|---|---|
| **Scope** | many repos × worktrees × agents | one repo, many worktrees | one repo, many worktrees | one tool, flat list |
| **Interface** | desktop app or TUI, same fleet | terminal only | GUI only | inside the CLI |
| **Runtime** | tmux or ConPTY: survives close, reattach | tmux | app process | inside the CLI |
| **Triage** | needs-you float to top, jump-to-next | flat list | varies | grouped by state |
| **Usage limits** | live usage corner + auto-continue | No | No | No |
| **Remote** | open WebSocket bridge + APNs over Tailscale (iOS app soon) | No | No | No |

Honest take: if you work in **one** repo, Claude Squad/ccmanager or a single-repo GUI may be
simpler. repomon earns its keep once you're running agents across **several** projects at once,
and it doesn't make you choose between a terminal and a GUI to get there.

</details>

## Architecture

Use the [architecture guide](docs/architecture.md) to locate the component you need.

<details>
<summary>Details</summary>

A background daemon (`repomond`) owns SQLite, file watchers, the git layer, and the agent
runtime, exposing a JSON-RPC API over a local transport (Unix socket on macOS/Linux, named pipe
on Windows).

The agent runtime sits behind a `SessionBackend` trait: tmux on macOS/Linux, and per-agent host
processes on Windows.

Every client, the desktop app, the TUI, and the iOS companion, is a thin client over that one
API, so any of them can watch and drive the same fleet at once.

Five crates, plus the desktop app:

| Reference | Details |
|---|---|
| 1 | `repomon-core`: data model, gix git layer, SQLite store, watchers, agent runtime (`SessionBackend`). |
| 2 | `repomon-daemon`: the `repomond` socket/pipe server and background services. |
| 3 | `apps/desktop`: Mission Control, the Tauri desktop app (`repomon-desktop`), bundling its own daemon and (on macOS/Linux) a portable `tmux`. |
| 4 | `repomon-tui`: the `repomon` terminal UI. |
| 5 | `repomon-mcp`: repomind's MCP server (`repomond mcp`), exposing the fleet to an orchestrator agent over stdio. |
| 6 | `repomon-host`: `repomon-agent-host.exe`, the per-agent ConPTY host that gives Windows tmux-style durability (Windows only). |

</details>

## Mission Control

Open [the desktop guide](docs/desktop.md) for every setting and keyboard shortcut.

<details>
<summary>Details: desktop feature reference</summary>

Repomon 0.9.0 bundles the daemon and, on macOS and Linux, a portable `tmux`.

Agents run durably (survive closing the window, reattach with full scrollback) with no separate
install.

First launch walks you through a short onboarding flow, and **Settings > System** shows a live
health check for tmux, git, and every agent CLI, with one-click-copy install commands for
anything missing.

**One fleet, every repo.**

The sidebar groups lanes (repo + worktree) by project, sorts by recent activity, and floats the
ones waiting on you.

A lane with more than one agent running shows a live roster on hover, so you can see who's doing
what without opening it.

**Git explorer and editor, built in, side by side.**

The right rail is a resizable (drag its edge), multi-panel host.

Git (`⌘1`): branch status against its base, working-tree changes with per-file stats, commit
history, a unified diff viewer, and clickable commit details (message, author, full patch).

Editor (`⌘2`): a file tree over the lane's worktree, multi-file tabs with dirty tracking, and a
full CodeMirror editor themed to match.

If an agent changes a file you have open, you get a conflict banner (reload or keep mine)
instead of a silent overwrite.

**Usage and model rates.**

The Usage view (`mod+3`) tracks tokens and equivalent API cost.

Settings > Usage edits model rates and controls the sidebar Today cost row.

Reader updates recount old transcripts in bounded batches, with progress shown until totals
settle.

**Native Windows chrome.**

One custom title bar holds window controls and the app toolbar.

Settings > System installs the bundled CLI and reports its version and PATH status.

**Self-service recovery.**

Settings can stop, start, or reset the daemon and bulk-restore orphaned agent sessions, without
a terminal.

**Fleet mail between agents.**

Address one agent, a whole lane (`lane-12/*`), or the whole fleet (`*`), with per-recipient
delivery results.

**repomind (work in progress).**

An orchestrator agent that can run as Claude Code, Codex, Antigravity, or OpenCode and manage
agents of every kind underneath it.

Functional, still rough at the edges.

See the [repomind section](#repomind-fleet-orchestrator-work-in-progress) below.
See [docs/desktop.md](docs/desktop.md) for the full keyboard reference and every setting.

</details>

## Documentation

Choose the reference that matches your task.

| Guide | Use it for |
|---|---|
| [Architecture](docs/architecture.md) | Daemon, desktop, TUI and core responsibilities |
| [Desktop](docs/desktop.md) | Keyboard shortcuts, settings and daily procedures |
| [Daemon protocol](docs/protocol.md) | JSON-RPC client integration |
| [Agents](docs/agents.md) | Agent runtime and status detection |
| [Supervision](docs/agent-supervision.md) | Permission handling, mail delivery and supervised stall nudges |
| [Windows validation](docs/windows-validation.md) | Manual Windows 11 release gate |
| [Host protocol](crates/repomon-host/PROTOCOL.md) | Frozen Windows agent-host control contract |

## Status

Read the release and validation limits before depending on an unfinished feature.

<details>
<summary>Details</summary>

**Done:** Mission Control (fleet, git explorer, in-app editor, onboarding, System Health,
self-service daemon recovery, usage and model rates, native Windows title bar), the TUI
(fleet/today, the agent multiplexer, the history dashboard), the remote access layer (WebSocket
bridge + APNs + pairing).

All on macOS, Linux, and Windows, each with native service/notification/clipboard/liveness
paths.

**Work in progress:** repomind (cross-kind orchestration, fleet mail, playbooks, approval
memory, standing orchestrations) - functional, see the [repomind
section](#repomind-fleet-orchestrator-work-in-progress).

**Windows: released, validation catching up.** CLI and desktop app both ship in every release
(`repomon-<version>-{aarch64,x86_64}-pc-windows-msvc.zip`, `Repomon_<version>_x64-setup.exe`): a
host-process backend (`repomon-agent-host.exe`) in place of tmux, named-pipe IPC, durability
parity with Unix.

What's still pending: a physical Windows 11 end-to-end validation pass and binary signing (see
[docs/windows-validation.md](docs/windows-validation.md)) - until that lands, treat Windows as
newer and less battle-tested than macOS/Linux.

**Not started / deferred:** the iOS companion app (built, ships once an Apple Developer account
is in place), a web dashboard.

</details>
