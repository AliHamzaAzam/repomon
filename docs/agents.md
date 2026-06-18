# Agents

repomon runs every agent the same way — each in its own durable tmux window — but it learns
each agent's *status* differently, because each CLI stores its session state differently.

## How agents run

When you spawn an agent (New Lane, the `e` key, or `agent.spawn`), the daemon launches its
CLI in a tmux window named `lane-<id>` inside the configured session (default `repomon`):

```
tmux new-window -t repomon -n lane-7 -c <worktree> '<agent-binary> [task]'
```

The daemon reads output with `capture-pane`, sends input with `send-keys`, and `attach`
gives you the raw session. Because tmux owns the process, the agent survives the daemon and
the TUI. The spawned kind is recorded on the lane so repomon can identify it later.

Several agents can run in the **same** worktree at once: a second spawn (or adopting an
external session into an occupied lane) takes the next slot — `lane-<id>-2`, `lane-<id>-3`,
… — and they run side by side. Fleet and the sidebar mark such a lane with an `×N` badge,
and `Tab`/`⇧Tab` cycle the cursor between a lane's agents in Split/Focus; input and attach
route to the cursored one.

## Choosing an agent

New Lane lists the **auto-detected** built-ins (claude-code / codex / aider, marked ✓ if on
PATH) plus any **custom agents** you define — cycle them with Tab (Shift+Tab to go back). The
**default** agent (marked ★) is preselected.

### Multiple Claude accounts

Claude keeps each account's data in a config dir (`~/.claude` by default; a second account is
typically run with `CLAUDE_CONFIG_DIR=~/.claude-work`). repomon scans for these — the default
`~/.claude` plus any `~/.claude-*` holding a `projects/` dir, and `$CLAUDE_CONFIG_DIR` — and
offers **one agent per account**: `claude-code` (default) and e.g. `claude-work`
(→ `CLAUDE_CONFIG_DIR=~/.claude-work claude`). No custom config needed. Detection and adopt
are account-aware: a work-account session is read from `~/.claude-work/projects` and adopting
it resumes against that account. (A shell *alias* like `claude-work` isn't a real binary, so a
custom agent pointing at `claude-work` won't launch — use the autodetected entry instead.)

## Managing agents in-app

Press **`A`** from Fleet (or **`Ctrl+A`** from New Lane) to open the agent manager:

- **`n`** — add a custom agent: a *name* (what you pick in New Lane) and a *command* (the
  launch command line, run in the lane's worktree). Tab switches fields, `↵` saves.
- **`e`** — edit the selected custom agent (built-ins are read-only). Renaming is handled
  transparently.
- **`d`** — delete the selected custom agent.
- **`*`** — set (or clear) the selected agent as the default; built-ins can be the default too.

Changes are written straight to `~/.config/repomon/config.toml`. You can still hand-edit it:

```toml
# ~/.config/repomon/config.toml
default_agent = "claude-yolo"

[agents]
claude-yolo = "claude --dangerously-skip-permissions"
claude-resume = "claude --continue"
```

> Note: editing agents in-app rewrites `config.toml` via the serializer, so hand-added
> comments in that file are not preserved.

`agent.detect` returns the combined list (with a `default` flag); `agent.add` / `agent.remove`
/ `agent.set_default` mutate the config and persist it; `agent.spawn` resolves a chosen name
to its custom command (if any) or the built-in binary, appends an optional task, and runs it.

## Interacting

There are two ways to drive an agent, and they trade off fidelity vs. staying in repomon's chrome.

### Open it as a real terminal (the native way) — `↵`/`→`/`a`

Pressing **`↵`** (Split/Grid), or **`↵` / `→` / `a`** (Focus), **attaches** to the agent's own
tmux pane. This is a *genuine terminal* — there is **no difference** from running the agent in a
plain terminal window: native wheel scrolling and scrollback, character-precise mouse
selection, **⌘V image paste** straight into Claude, full color, every key.

**To come back to repomon, press `F12`** (single key) — or `Ctrl-b d`, or `Ctrl-b q`. A thin
status bar along the bottom of the attached pane always shows this. Detaching leaves the agent
**running in the background**; don't type `exit` or `Ctrl-C` unless you actually want to end it.

repomon configures its tmux server to feel native: `mouse on` (wheel scroll + drag-select),
`set-clipboard on` (OSC-52 passthrough), a 50k-line scrollback, drag-select copies straight to
the system clipboard via `pbcopy`, and a status bar showing the detach key. Because you're in
the real process, anything the agent supports in a terminal — including image paste — works
exactly as it would standalone.

> Why attach rather than emulate? The in-app view is a `capture-pane` *picture* plus
> `send-keys`; it can't carry a nested terminal's scroll wheel or real clipboard-image paste.
> Attaching hands you the actual PTY, so the focused agent is indistinguishable from native.

### Quick mediated type — `i`

For a fast one-liner without the attach context-switch, **`i`** enters **insert** mode and
forwards each keystroke via `send-keys` — printable chars, Enter, Backspace, arrows,
**Shift+Tab** (Claude's mode cycling), `Ctrl-<key>` (e.g. `Ctrl-C`), and **`Esc`** (the agent
needs it to interrupt/clear). Because `Esc` is forwarded, leave insert with **`Ctrl-O`**.
**Option/Alt + Arrow** (word jump) and **Alt + Backspace** (word delete) forward too — set
Terminal.app → Profiles → Keyboard → "Use Option as Meta key". This view is a snapshot, so:

- **Scroll back** with **`PgUp`/`PgDn`** (work in both modes; always reach repomon). Typing or
  `↵`/`esc` returns to the live tail.
- **Select & copy**: drag over lines — copied to the clipboard on release (line-granular).
- **Paste an image**: press **`v`** — repomon saves the clipboard image to a temp PNG and inserts
  its path (Claude reads images referenced by path).

For anything the snapshot can't do (precise selection, wheel scroll, ⌘V image paste), just open
the real terminal with `↵`.

`AgentKind::command()` maps kinds to binaries:

| Kind          | Binary         |
|---------------|----------------|
| `claude-code` | `claude`       |
| `codex`       | `codex`        |
| `aider`       | `aider`        |
| `cursor`      | `cursor-agent` |
| other         | the kind string itself |

## Auto-continue on usage limits

When a Claude agent hits its usage limit it prints "limit reached · resets at <time>" and stops
mid-work. repomon **auto-continues** it: a background watcher in the daemon scans each managed
agent's pane (~every 20 s), and when it sees the blocking message it schedules a resume — at the
parsed reset time (+60 s), or on a 5-minute periodic retry if the time can't be read — then types
the continue message (`continue` + Enter). The lane shows **`⏳ rate-limited · resume 3:00 PM`**
while it waits. This runs even with the TUI closed, so durable agents you left running get
resumed on their own.

- **On by default** for every repomon-managed agent. The transcript doesn't record limit info, so
  detection reads the tmux pane; the "approaching usage limit" warning never triggers it.
- **Per-lane off:** press **`C`** on a lane to disable auto-continue for it this session (it then
  shows the normal `⏸ needs you` when paused). **Globally:** set `auto_continue = false` in
  `config.toml`. Change the typed message with `auto_continue_message` (default `"continue"`).
- **Give-up:** after 6 attempts that don't take, it stops and flags the lane **needs you** so you
  can step in.
- Only **managed** agents (with a tmux window) are touched — external sessions have no window to
  type into. The detection/parse and the state machine are pure and unit-tested
  (`agent/limit.rs`, `auto_continue.rs`).

## Usage corner (`/usage` probe)

With `usage_probe = true` (a Settings toggle, **off by default**), the TUI shows Claude's account
usage in the **bottom-right corner** — `5h 38% · wk 12% · 3:00 PM` (5-hour window %, weekly %, and
the 5-hour reset time) — for the **account the focused agent runs under** (e.g. `~/.claude` vs
`~/.claude-work`); switch focus to an agent on another account and the corner follows.

Subscription usage has no CLI flag, file, or supported endpoint — the only source is the
interactive `/usage` command. So a daemon watcher (`usage_watch.rs`), **only while a TUI is
attached**, spawns a hidden throwaway `claude` window per account every ~5 minutes, sends `/usage`,
captures and parses the pane (`agent/usage.rs`, fixture-tested), then dismisses (`Esc`) and kills
the window. It never sends a model prompt. Caveats, by design:

- It **spawns a background `claude` process** briefly per probe (hence opt-in). The probe window is
  named `usage-probe-…` (not `lane-…`) and runs in your home dir, so it never inflates a lane's
  `×N` agent count. The first run accepts the one-time folder-trust prompt for that dir; each probe
  leaves a tiny (promptless) transcript behind.
- The `/usage` layout is undocumented and changes between Claude versions. The parser anchors on
  labels (not positions) and returns nothing rather than wrong numbers; when it can't read usage,
  the corner **falls back** to the focused lane's rate-limit countdown (`⏳ resume 3:00 PM`), or
  shows nothing. If Claude restyles `/usage`, recapture the fixture and adjust the parser.

## External sessions (running in another terminal)

Because status comes from the transcript, a `claude` you start in any other terminal inside a
registered repo's worktree is **detected automatically** — its status and "needs you" show up
on that lane, tagged `·ext` (external: repomon didn't spawn it, so it has no tmux window).

If you run **several** Claude sessions in one worktree, each (a distinct `<session-id>.jsonl`,
active within the last few hours) shows as its own entry in the lane detail — `Tab`/`⇧Tab`
move the cursor (`‣`) between them.

repomon can't type into a plain terminal process, so to drive an external session press
**`o` to adopt** the highlighted one (Fleet/Split/Focus): repomon resumes *that exact* session
with `claude --resume <id>` (or `--continue` for the most recent) in a managed tmux lane,
after which it's fully interactive here. The original terminal window is left as-is — close it
once you've adopted. repomon can manage several agents in the same worktree, each in its own
tmux window (`lane-<id>`, `lane-<id>-2`, …), so adopting an external session adds a managed
agent alongside any already running — and you can observe every external session in the lane
detail and choose which to adopt.

## How status is detected

Each agent kind has an `AgentMonitor` (`crates/repomon-core/src/agent/`). Monitors are tried
in priority order; the first to return a summary wins. If none does, the daemon falls back to
"is the repomon-spawned tmux window alive?" and shows the recorded kind as **Running**.

### Claude Code — rich status

Transcripts live at `~/.claude/projects/<encoded-cwd>/<session>.jsonl`, where the directory
name is the working directory with `/` and `.` replaced by `-`. repomon derives:

- **tool-call count** — `tool_use` blocks across assistant messages,
- **status** — *Waiting* (the last entry is an assistant turn with no tool call → **needs you**),
  *Running* (mid tool-loop), or *Idle* (no activity for 10 min),
- **title** — first user message or a `summary` entry.

The encoding scheme has changed before, so it's isolated in `claude::encode_project_dir` and
fixture-tested; matching also falls back to the `cwd` recorded inside each transcript.

### Aider — coarse status

Aider writes `.aider.chat.history.md` into the working directory. repomon uses that file's
modification time: **Running** if it changed in the last two minutes, else **Idle**. (There's
no reliable "needs you" signal yet.)

### Codex — tmux-only for now

Codex's on-disk session format isn't stable enough to parse reliably, so `CodexMonitor`
returns nothing and repomon relies on the tmux-alive fallback for Codex agents it spawned.
When the format stabilizes, implement `CodexMonitor::summary_for` like the others.

## Adding a new agent

1. Add a variant (or use `Other`) and a binary in `AgentKind` (`model.rs`).
2. Implement `AgentMonitor` for it in `crates/repomon-core/src/agent/` and add it to
   `default_monitors()`.
3. Add it to `AGENT_KINDS` in the TUI so New Lane can spawn it (Tab to cycle).
