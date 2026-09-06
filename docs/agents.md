# Agents

Choose an installed agent in **New Lane**, then spawn it. This guide explains terminal control, adoption, mail and status signals.

**You are here:** agent setup and troubleshooting; desktop shortcuts are in [the desktop guide](desktop.md#keyboard-control).

## Find your next task

| Task | Go to |
|---|---|
| How agents run | [Open section](#how-agents-run) |
| Choosing an agent | [Open section](#choosing-an-agent) |
| Managing agents in-app | [Open section](#managing-agents-in-app) |
| Interacting | [Open section](#interacting) |
| Auto-continue on usage limits | [Open section](#auto-continue-on-usage-limits) |
| Usage and cost tracking | [Open section](#usage-and-cost-tracking) |
| Usage corner (usage probe) | [Open section](#usage-corner-usage-probe) |
| Expanded agent rows + rename | [Open section](#expanded-agent-rows--rename) |
| Fleet mail for managed agents | [Open section](#fleet-mail-for-managed-agents) |
| Fleet mail and MCP wiring by agent kind | [Open section](#fleet-mail-and-mcp-wiring-by-agent-kind) |
| External sessions (running in another terminal) | [Open section](#external-sessions-running-in-another-terminal) |
| How status is detected | [Open section](#how-status-is-detected) |
| Adding a new agent | [Open section](#adding-a-new-agent) |

<details>
<summary>Why this guide exists</summary>

repomon runs every agent the same way (each in its own durable session window), but it learns
each agent's *status* differently, because each CLI stores its session state differently.

</details>

## How agents run

Spawn from **New Lane**, the TUI `e` key, or `agent.spawn`.

The Unix backend constructs a command like this; `<worktree>` and `<agent-binary>` are placeholders.

```
tmux new-window -t repomon -n lane-7 -c <worktree> '<agent-binary> [task]'
```

<details>
<summary>Details</summary>

When you spawn an agent (New Lane, the `e` key, or `agent.spawn`), the daemon launches its CLI
in a window named `lane-<id>` inside the configured session (default `repomon`).

On macOS/Linux, the example above runs in a tmux window.

On **Windows** there is no tmux: the daemon spawns a detached host process,
`repomon-agent-host.exe`, per window (`\\.\pipe\repomon-<session>-<window>`).

The host owns a ConPTY child and a server-side terminal emulator with scrollback, and it plays
the exact durability role tmux plays on Unix.

Either way the daemon reads output with `capture_named` (`capture-pane` / host `capture`), sends
input with `send_*` (`send-keys` / host `send_text`), and attach hands you the raw session.

Because the session backend owns the process (a tmux server or a detached host), the agent
survives the daemon and the TUI.

The spawned kind is recorded on the lane so repomon can identify it later.

Several agents can run in the **same** worktree at once: a second spawn (or adopting an external
session into an occupied lane) takes the next slot - `lane-<id>-2`, `lane-<id>-3`, … - and they
run side by side.

Fleet and the sidebar mark such a lane with an `×N` badge, and `Tab`/`⇧Tab` cycle the cursor
between a lane's agents in Split/Focus; input and attach route to the cursored one.

</details>

## Choosing an agent

Open **New Lane** and select an available built-in or a custom command.

<details>
<summary>Details</summary>

New Lane lists the **auto-detected** built-ins (claude-code / codex / hermes / opencode /
antigravity / aider / cursor, marked available if on PATH) plus any **custom agents** you define
- cycle them with Tab (Shift+Tab to go back).

The **default** agent (marked default) is preselected.

</details>

### Multiple Claude accounts

Choose the detected account entry whose config directory holds the session you want.

<details>
<summary>Details</summary>

Claude keeps each account's data in a config dir (`~/.claude` by default; a second account is
typically run with `CLAUDE_CONFIG_DIR=~/.claude-work`). repomon scans for these - the default
`~/.claude` plus any `~/.claude-*` holding a `projects/` dir, and `$CLAUDE_CONFIG_DIR` - and
offers **one agent per account**: `claude-code` (default) and e.g.

`claude-work` (→ `CLAUDE_CONFIG_DIR=~/.claude-work claude`).

No custom config needed.

Detection and adopt are account-aware: a work-account session is read from
`~/.claude-work/projects` and adopting it resumes against that account. (A shell *alias* like
`claude-work` isn't a real binary, so a custom agent pointing at `claude-work` won't launch -
use the autodetected entry instead.)

</details>

## Managing agents in-app

In the TUI, press `A` from Fleet or `Ctrl+A` from New Lane.

**Time: 2 minutes.**

1. Open the TUI agent manager with `A`.
2. Press `n`, enter a name and executable command, and press Enter to save.
3. Press `*` to make the selected entry the default, if wanted.

**You know it worked when:** the custom entry appears in New Lane and the default is preselected.

```toml
# ~/.config/repomon/config.toml
default_agent = "claude-yolo"

[agents]
claude-yolo = "claude --dangerously-skip-permissions"
claude-resume = "claude --continue"
```

<details>
<summary>Details</summary>

Press **`A`** from Fleet (or **`Ctrl+A`** from New Lane) to open the agent manager:

- **`n`** - add a custom agent: a *name* (what you pick in New Lane) and a *command* (the
  launch command line, run in the lane's worktree). Tab switches fields, `↵` saves.
- **`e`** - edit the selected custom agent (built-ins are read-only). Renaming is handled
  transparently.
- **`d`** - delete the selected custom agent.
- **`*`** - set (or clear) the selected agent as the default; built-ins can be the default too.

Changes are written straight to `~/.config/repomon/config.toml`.

You can still hand-edit it:

> Note: editing agents in-app rewrites `config.toml` via the serializer, so hand-added >
comments in that file are not preserved.

`agent.detect` returns the combined list (with a `default` flag); `agent.add` / `agent.remove` /
`agent.set_default` mutate the config and persist it; `agent.spawn` resolves a chosen name to
its custom command (if any) or the built-in binary, appends an optional task, and runs it.

</details>

## Interacting

Attach for full terminal behavior; use `i` for a short instruction inside the TUI.

<details>
<summary>Details</summary>

There are two ways to drive an agent, and they trade off fidelity vs. staying in repomon's
chrome.

</details>

### Open it as a real terminal (the native way) - `↵`/`→`/`a`

Press `Enter` in Split/Grid, or `Enter`, Right arrow or `a` in Focus.

<details>
<summary>Details</summary>

Pressing **`↵`** (Split/Grid), or **`↵` / `→` / `a`** (Focus), **attaches** to the agent's own
tmux pane.

This is a *genuine terminal* - there is **no difference** from running the agent in a plain
terminal window: native wheel scrolling and scrollback, character-precise mouse selection, **⌘V
image paste** straight into Claude, full color, every key.

**To come back to repomon, press `F12`** (single key) - or `Ctrl-b d`, or `Ctrl-b q`.

A thin status bar along the bottom of the attached pane always shows this.

Detaching leaves the agent **running in the background**; don't type `exit` or `Ctrl-C` unless
you actually want to end it.

repomon configures its tmux server to feel native: `mouse on` (wheel scroll + drag-select),
`set-clipboard on` (OSC-52 passthrough), a 50k-line scrollback, drag-select copies straight to
the system clipboard via `pbcopy`, and a status bar showing the detach key.

Because you're in the real process, anything the agent supports in a terminal - including image
paste - works exactly as it would standalone.

> Why attach rather than emulate?

The in-app view is a `capture-pane` *picture* plus > `send-keys`; it can't carry a nested
terminal's scroll wheel or real clipboard-image paste. > Attaching hands you the actual PTY, so
the focused agent is indistinguishable from native.

</details>

#### On Windows

Use `Enter`, Right arrow or `a` to open a Windows Terminal tab; press `F12` to detach.

<details>
<summary>Details</summary>

There is no tmux to hand off, so attach works in two layers.

The **embedded focus view** renders the agent from the host's server-side terminal (`capture` +
`cursor` + `alternate_on`) and forwards input the same way it does on Unix.

For a genuine terminal, `↵`/`→`/`a` **pops out** the agent into a new Windows Terminal tab
running `repomon attach-host <window>`, a raw byte proxy that subscribes to the host's byte
stream (`subscribe_bytes`), pipes your keystrokes back, and tracks console resizes.

Because a subscribed connection ignores client frames, the attach client opens a second control
connection for input and resize while the first streams the pane.

**Press `F12` to detach** (tmux-parity); detaching leaves the agent running in its host process.

If Windows Terminal (`wt.exe`) is not available the client falls back to a new console window.

</details>

### Quick mediated type - `i`

Press `i` to type into the agent without leaving the TUI; press `Ctrl+O` to leave insert mode.

| Kind          | Binary         |
|---------------|----------------|
| `claude-code` | `claude`       |
| `codex`       | `codex`        |
| `hermes`      | `hermes chat --tui` |
| `opencode`    | `opencode`     |
| `antigravity` | `agy`          |
| `aider`       | `aider`        |
| `cursor`      | `cursor-agent` |
| other         | the kind string itself |

<details>
<summary>Details</summary>

For a fast one-liner without the attach context-switch, **`i`** enters **insert** mode and
forwards each keystroke via `send-keys` - printable chars, Enter, Backspace, arrows,
**Shift+Tab** (Claude's mode cycling), `Ctrl-<key>` (e.g.

`Ctrl-C`), and **`Esc`** (the agent needs it to interrupt/clear).

Because `Esc` is forwarded, leave insert with **`Ctrl-O`**.

**Option/Alt + Arrow** (word jump) and **Alt + Backspace** (word delete) forward too - set
Terminal.app to Profiles to Keyboard to "Use Option as Meta key".

This view is a snapshot, so:

- **Scroll back** with **`PgUp`/`PgDn`** (work in both modes; always reach repomon). Typing or
  `↵`/`esc` returns to the live tail.
- **Select & copy**: drag over lines - copied to the clipboard on release (line-granular).
- **Paste an image**: press **`v`** - repomon saves the clipboard image to a temp PNG and inserts
  its path (Claude reads images referenced by path).

For anything the snapshot can't do (precise selection, wheel scroll, ⌘V image paste), just open
the real terminal with `↵`.

`AgentKind::command()` maps kinds to binaries:

</details>

## Auto-continue on usage limits

Leave auto-continue enabled to resume a managed agent after its quota resets.

<details>
<summary>Details</summary>

When a Claude agent hits its usage limit it prints "limit reached · resets at <time>" and stops
mid-work. repomon **auto-continues** it: a background watcher in the daemon scans each managed
agent's pane (~every 20 s), and when it sees the blocking message it schedules a resume - at the
parsed reset time (+60 s), or on a 5-minute periodic retry if the time can't be read - then
types the continue message (`continue` + Enter).

The lane shows **` rate-limited · resume 3:00 PM`** while it waits.

This runs even with the TUI closed, so durable agents you left running get resumed on their own.

- **On by default** for every repomon-managed agent. The transcript doesn't record limit info, so
  detection reads the tmux pane; the "approaching usage limit" warning never triggers it.
- **Per-lane off:** press **`C`** on a lane to disable auto-continue for it this session (it then
  shows the normal `! needs you` when paused). **Globally:** set `auto_continue = false` in
  `config.toml`. Change the typed message with `auto_continue_message` (default `"continue"`).
- **Give-up:** after 6 attempts that don't take, it stops and flags the lane **needs you** so you
  can step in.
- Only **managed** agents (with a tmux window) are touched - external sessions have no window to
  type into. The detection/parse and the state machine are pure and unit-tested
  (`agent/limit.rs`, `auto_continue.rs`).

</details>

## Usage and cost tracking

Open desktop Usage with `mod+3` to inspect local token and equivalent-cost history.

<details>
<summary>Details</summary>

The desktop Usage view (`mod+3`) reads local transcripts into a per-turn token ledger,
separately from the optional quota probe below.

Settings > Usage controls tracking and price refresh, edits model rates, and shows or hides
today's equivalent API cost in the sidebar.

A reader revision recounts older sources in bounded batches; the view shows progress until the
totals settle.

See [desktop.md](desktop.md#usage) for supported readers and pricing.

</details>

## Usage corner (usage probe)

Enable **Probe account usage** only if you want the optional quota probe.

<details>
<summary>Details</summary>

With `usage_probe = true` (a Settings toggle, **off by default**), the TUI shows agent usage in
the **bottom-right corner** - e.g.

`5h 38% · wk 12% · 3:00 PM` (limit windows + the soonest reset) - for the **account the focused
agent runs under**.

It's provider-aware and per-account: a Claude agent shows its account's `/usage` (`~/.claude` vs
`~/.claude-work`), a Codex agent shows its `/status`; switch focus and the corner follows.

Subscription usage has no CLI flag, file, or supported endpoint - the only source is an
interactive command (Claude `/usage`, Codex `/status`).

So a daemon watcher (`usage_watch.rs`), **only while a local desktop or TUI client is active**,
spawns a hidden throwaway session per account every ~5 minutes, sends the usage command,
captures and parses the pane (`agent/usage.rs`, fixture-tested), then dismisses (`Esc`) and
kills the window.

It never sends a model prompt.

Numbers are normalized to **% used** across agents (Codex reports "% left"); windows shown are
whatever the tool reports - Claude's 5-hour + weekly, Codex's 5-hour/weekly or (Free plan)
monthly.

Caveats, by design:

- It **spawns a background agent process** briefly per probe (hence opt-in). The probe window is
  named `usage-probe-…` (not `lane-…`) and runs in your home dir, so it never inflates a lane's
  `×N` agent count. The first run accepts the one-time folder-trust prompt for that dir; each probe
  leaves a tiny (promptless) transcript/session behind. Codex is probed only when it's installed
  (`~/.codex` exists).
- The `/usage` and `/status` layouts are undocumented and change between versions. The parsers
  anchor on labels (not positions) and return nothing rather than wrong numbers; when usage can't
  be read, the corner **falls back** to the focused lane's rate-limit countdown (` resume 3:00
  PM`), or shows nothing. If a tool restyles its screen, recapture the fixture
  (`crates/repomon-core/src/agent/fixtures/`) and adjust the parser.

</details>

## Expanded agent rows + rename

Enable **expand agent rows** in TUI Settings to select agents individually.

**Time: 1 minute.**

1. Enable **expand agent rows** in Settings (`,`).
2. Select an agent row and press `R`.
3. Enter the label and press Enter; Escape cancels and an empty label clears it.

**You know it worked when:** the label survives a refresh for that same transcript identity.

<details>
<summary>Details</summary>

By default a lane running several agents shows as one sidebar row with an `×N` badge.

Turn on **`expand agent rows`** in Settings (`,`) to instead show the lane as a small tree: the
lane header (keeping `×N`) with one indented row per agent - `↳ <summary>  <status>`.

The summary is auto-derived (the first 1–4 words of that agent's opening prompt), and each
agent's own status glyph is shown, so you can see and select individual agents directly in the
Fleet/Split sidebars.

Up/down navigate the rows; selecting an agent row makes it the active agent
(Enter/focus/attach/stop/keys target it).

Press **`R`** on a selected agent row to **rename** it inline (Enter saves, Esc cancels; an
empty name clears the custom label).

The label persists in the daemon keyed by the agent's transcript id, so it survives refreshes
and daemon restarts, and never bleeds onto a different agent that later reuses the slot.
(Sessions without a transcript id yet - a just-spawned placeholder - can't be renamed until
their transcript appears.) See `session.rename` in `docs/protocol.md`.

</details>

## Fleet mail for managed agents

Use `message_send` and `message_inbox` from a managed MCP-capable agent.

<details>
<summary>Details</summary>

Managed Claude, Codex, Hermes, OpenCode, Antigravity, and Cursor sessions receive a restricted
local `repomon` MCP server at spawn or adopt time.

It exposes `fleet_status`, `message_send`, `message_inbox`, and `message_mark_read`.

It does not expose repomind's mutating fleet tools.

The agent process inherits a one-time identity token; only its SHA-256 hash is stored, and the
generated MCP config contains no token.

OpenCode receives the registration through the runtime-only `OPENCODE_CONFIG_CONTENT` merge, so
managed settings with higher precedence remain authoritative.

Antigravity requires its global `~/.gemini/config/mcp_config.json` registration.

Cursor requires its global `~/.cursor/mcp.json` registration. repomon merges only
`mcpServers.repomon`, never writes `.agents/mcp_config.json` in a repository, and keeps identity
in the managed process environment.

Antigravity may still show its normal workspace trust and tool permission prompts; Cursor's
`--approve-mcps` flag is available for headless/non-interactive workflows.

Hermes is launched in its persistent modern TUI.

Because Hermes filters nonstandard variables before starting stdio MCP servers, repomon
registers `${REPOMON_MCP_*}` placeholders through Hermes's own atomic config writer; the
per-session values are resolved only in the managed process.

An opening task is submitted after Hermes reports that its composer is ready (`-q` is
deliberately not used because it exits after one turn).

Adopt resumes with `hermes chat --resume <id>` and keeps the lane worktree with
`--no-restore-cwd`.

**Aider**: has no native MCP client support as of its current release.

The identity token and socket are still passed via the process environment in case a future
version adds support, but fleet mail tool calls will not reach the MCP server.

**Custom/Other agents**: the daemon inspects the command's binary name (`kind_from_command`) to
detect whether a custom agent wraps a known binary.

Wrappers like `claude --dangerously-skip-permissions` receive ClaudeCode's `--mcp-config`
wiring; `agy --mode plan` receives Antigravity's `~/.gemini/config/mcp_config.json`
registration; `cursor-agent --approve-mcps` receives Cursor's `~/.cursor/mcp.json` registration.

Completely unknown binaries (e.g.

`my-exotic-agent`) receive no MCP registration, though the three environment variables are
always set.

Messages are durable in the daemon database.

Terminal injection is only attempted when the recipient has a live managed window and is
waiting, idle, or at an ended turn with no dialog, rate limit, or stall.

Agent-to-agent injection is off by default.

Operator and repomind injection is on by default.

Inbox access works even when injection is disabled or unsupported.

See `docs/messaging.md` for addressing, threading, limits, and UI behavior.

</details>

## Fleet mail and MCP wiring by agent kind

Check the matrix before relying on a backend’s mail or orchestrator support.

| Kind | Worker fleet mail | Orchestrator | Transcript |
|------|-------------------|--------------|------------|
| Claude Code (`claude`) | yes, `--mcp-config` | yes | full JSONL under `~/.claude/projects/` |
| Codex (`codex`) | yes, `-c mcp_servers.repomon...` | yes | pane view only |
| Antigravity (`agy`) | yes, `~/.gemini/config/mcp_config.json` | yes | pane view only |
| OpenCode (`opencode`) | yes, `OPENCODE_CONFIG_CONTENT` | yes | pane view only |
| Cursor (`cursor-agent`) | yes, `~/.cursor/mcp.json` | no | pane view only |
| Aider (`aider`) | no, Aider has no MCP client | no | mtime only |
| Custom (`[agents]` entries) | routed by the binary's dialect, see below | no | none |

<details>
<summary>Details</summary>

Fleet mail and the orchestrator role both ride on the restricted MCP surface the daemon exposes
(`message_send`, `message_inbox`, `message_mark_read`, `fleet_status`).

Mail is addressed by `lane-X/slot`, never by kind, so any wired agent can mail any other; see
`docs/messaging.md` for the address grammar.

What differs per kind is how the MCP server gets registered and whether the kind can run as an
orchestrator.

Every wired kind satisfies four pillars on both the spawn and the adopt path: a registration
mechanism for the MCP server, `REPOMON_MCP_MODE=agent`, `REPOMON_MCP_SOCKET`, and
`REPOMON_MCP_IDENTITY_TOKEN` in the process environment.

The three environment variables are set for every kind, Aider and unknown custom agents
included, so a future MCP-capable version or an operator wrapper picks them up without changes
here.

No token or socket path is ever written to a config file on disk; the registered command only
names the `repomond mcp` executable.

All kinds can be adopted.

Claude Code, OpenCode and Antigravity resume their session (`--resume`, `--session`,
`--conversation`); Codex, Cursor, Aider and custom agents re-launch fresh in the worktree
because Repomon’s adoption path relaunches them instead of resuming an exact session.

</details>

### Custom agents

Point custom entries at an executable, not a shell alias.

```toml
[agents]
claude-yolo = "claude --dangerously-skip-permissions"   # Claude Code wiring
my-agy      = "agy --mode plan"                         # Antigravity wiring
my-cursor   = "cursor-agent --approve-mcps"             # Cursor wiring
exotic-tool = "my-exotic-agent"                         # unknown binary: no MCP wiring
```

<details>
<summary>Details</summary>

An `[agents]` entry is classified by its binary name before flags are applied, so a custom
command inherits the wiring of the agent it wraps:

</details>

### Cursor notes

Use `cursor-agent`; inspect its registration with `cursor-agent mcp list`.

<details>
<summary>Details</summary>

The CLI is `cursor-agent` (`-p <prompt>` for headless runs, `--approve-mcps` to auto-approve MCP
tools, `cursor-agent mcp list` to inspect registrations).

Registration goes into the global `~/.cursor/mcp.json` (or the path in
`REPOMON_CURSOR_MCP_CONFIG`) with the standard `mcpServers` shape; a project-level
`.cursor/mcp.json` also works.

Cursor cannot act as an orchestrator.

</details>

### Aider notes

Read Aider mail through the fleet inbox unless your own wrapper provides MCP.

<details>
<summary>Details</summary>

The core Aider CLI has no MCP client, so Aider workers get no fleet mail.

Community wrappers (`mcpm-aider`, `AiderDesk`) exist but need a wrapper repomon cannot know
about; the environment variables are still set so such a wrapper can use them.

</details>

## External sessions (running in another terminal)

Select an external session and press `o` to adopt it into a managed lane.

**Time: 1 minute, plus CLI startup.**

1. Select the external session with Tab or Shift+Tab.
2. Press `o` to adopt it.
3. Check the managed session before closing the original terminal.

**You know it worked when:** a managed agent appears in the lane; supported backends resume the selected session.

<details>
<summary>Details</summary>

Because status comes from the transcript, a `claude` you start in any other terminal inside a
registered repo's worktree is **detected automatically** - its status and "needs you" show up on
that lane, tagged `·ext` (external: repomon didn't spawn it, so it has no tmux window).

If you run **several** Claude sessions in one worktree, each (a distinct `<session-id>.jsonl`,
active within the last few hours) shows as its own entry in the lane detail - `Tab`/`⇧Tab` move
the cursor (`‣`) between them.

repomon can't type into a plain terminal process, so to drive an external session press **`o` to
adopt** the highlighted one (Fleet/Split/Focus): repomon relaunches it in a managed tmux lane.

Every agent kind is adoptable.

Claude, OpenCode, and Antigravity resume *that exact* session with the backend's exact resume
flag: Claude uses `--resume <id>`, OpenCode uses `--session <id>`, and Antigravity uses
`--conversation <id>` (without an ID, each backend's continue flag is used instead).

Codex, Cursor, Aider, and custom agents have no stable session-resume flag, so adopting one of
those relaunches the same command fresh in the worktree rather than resuming the prior
conversation.

The original terminal window is left as-is, so close it once you've adopted. repomon can manage
several agents in the same worktree, each in its own tmux window (`lane-<id>`, `lane-<id>-2`,
…), so adopting an external session adds a managed agent alongside any already running - and you
can observe every external session in the lane detail and choose which to adopt.

</details>

## How status is detected

Compare the state with the backend-specific signal below before reporting a wrong status.

<details>
<summary>Details</summary>

Each agent kind has an `AgentMonitor` (`crates/repomon-core/src/agent/`).

Monitors are tried in priority order; the first to return a summary wins.

If no monitor returns a summary, a live managed window starts as **Idle**.

The pane overlay promotes it to **Running** only when working-footer or subagent evidence is visible; a detected permission dialog becomes **Waiting**.

</details>

### Claude Code - rich status

Check the transcript summary and detected dialog when Claude’s status looks wrong.

<details>
<summary>Details</summary>

Transcripts live at `~/.claude/projects/<encoded-cwd>/<session>.jsonl`, where the directory name
is the working directory with `/` and `.` replaced by `-`. repomon derives:

- **tool-call count** - `tool_use` blocks across assistant messages,
- **status** - *Waiting* (the last entry is an assistant turn with no tool call to **needs you**),
  *Running* (mid tool-loop), or *Idle* (no activity for 10 min),
- **title** - first user message or a `summary` entry.

The encoding scheme has changed before, so it's isolated in `claude::encode_project_dir` and
fixture-tested; matching also falls back to the `cwd` recorded inside each transcript.

</details>

### Aider - coarse status

Check when Aider’s history file last changed.

<details>
<summary>Details</summary>

Aider writes `.aider.chat.history.md` into the working directory. repomon uses that file's
modification time: **Running** if it changed in the last two minutes, else **Idle**. (There's no
reliable "needs you" signal yet.)

</details>

### Codex - tmux-only for now

Inspect the managed Codex pane for working or permission signals.

<details>
<summary>Details</summary>

`CodexMonitor::summary_for` returns no transcript summary for live status.

Managed Codex status comes from the window placeholder and pane overlay described above.

Usage accounting is separate: the ledger does parse supported Codex JSONL usage records.

A future transcript-based live-status implementation belongs in `CodexMonitor::summary_for`.

</details>

### OpenCode

Inspect OpenCode’s session store and managed pane.

<details>
<summary>Details</summary>

OpenCode 1.15.5 stores sessions, messages, and parts in its SQLite data store. repomon opens
that store read-only, validates every required table and column before querying, and returns no
summary when the schema is incompatible.

The latest assistant finish state and any running tool part produce Running, Waiting, or error
attention.

Exact adoption uses `opencode --session <id>`.

OpenCode local token and cost statistics do not satisfy repomon's quota UI contract.

Usage is therefore degraded and no percentage or reset window is displayed.

</details>

### Antigravity

Inspect Antigravity’s pane footer and dialog.

<details>
<summary>Details</summary>

Antigravity 1.1.12 documents and maintains
`~/.gemini/antigravity-cli/cache/last_conversations.json`, a cwd-to-conversation map. repomon
uses that stable cache for external identity and exact `agy --conversation <id>` adoption.

Transcript databases contain protobuf payloads without a stable status contract, so managed
windows and pane dialog detection supply live state.

Antigravity's `>` selection cursor is recognized for trust and permission attention.

Because the pane is the only live signal, its layout is read on Antigravity's own terms.

It streams the eight-dot braille spinner cycle (U+28FE, U+28FD, U+28FB, U+28BF, U+287F, U+28DF,
U+28EF, U+28F7) rather than the four-dot one, and prints `esc to cancel` at the head of its
status line for exactly as long as a turn is in flight, swapping it for `? for shortcuts` when
the turn ends; both read as "working", and the footer covers a capture that lands between
spinner redraws.

Its menus are boxless, sit above key hints (`Navigate`, `Amend`) that the box-drawing scan reads
as content below the menu, and wrap at the pane width, which splits the contiguous option run at
the 80 columns the tmux server usually runs; `detect_dialog` therefore falls back to a dedicated
Antigravity recognizer anchored on the question and one of those live footers.

A background shell task ticking in the footer is not the agent working, and a quota wall
(`Individual quota reached`) reports itself as `quota exhausted` so an idle row says what
stopped it.

The live `/usage` panel was not stable enough to fixture-test percentage and reset fields, so
usage is degraded.

Antigravity and OpenCode are both valid repomind orchestrator backends alongside Claude and
Codex (`orchestrator.start` with `agent: "antigravity"`/`"agy"` or `"opencode"`/`"open-code"`);
neither has a parseable on-disk transcript, so both are monitored pane-only, the same tradeoff
Codex makes (no `orchestrator.transcript` chat view, no `end_of_turn` attention, no
`session_id`).

`aider` and `cursor` have no MCP client repomon can drive as an orchestrator, so
`orchestrator.start` rejects them with `invalid_params` rather than spawning a broken window.

</details>

## Adding a new agent

Add the kind, monitor and picker entry together.

**Time: allow 30-60 minutes for initial wiring; monitoring and fixture work depend on the backend.**

1. Add a variant (or use `Other`) and a binary in `AgentKind` (`model.rs`).
2. Implement `AgentMonitor` for it in `crates/repomon-core/src/agent/` and add it to
   `default_monitors()`.
3. Add it to `AGENT_KINDS` in the TUI so New Lane can spawn it (Tab to cycle).

**You know it worked when:** New Lane offers the kind, spawning runs its binary, and its monitor returns the expected fixture status.
