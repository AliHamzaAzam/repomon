# Mission Control (desktop app)

Mission Control is repomon's desktop client: the same fleet the TUI drives, in a window, with
embedded terminals for every agent. It talks to the same daemon over the same socket, so the TUI,
the desktop app, and the iOS client can all watch one fleet at once.

## Install

Preview builds are published to the moving
[`desktop-preview`](https://github.com/AliHamzaAzam/repomon/releases/tag/desktop-preview) release
for macOS, Windows, and Linux:

| Platform | File |
|---|---|
| macOS (Apple silicon and Intel) | `Repomon_<version>_universal.dmg` |
| Windows | `Repomon_<version>_x64-setup.exe` |
| Linux | `Repomon_<version>_amd64.AppImage`, `.deb`, or `.rpm` |

The app updates itself: it checks the same release on launch and from **Settings > General >
Check for updates**, so you only download by hand once.

The daemon ships inside the bundle, so the desktop app needs no separate `repomond`. It does still
need `tmux` on macOS and Linux (`brew install tmux`, `sudo apt install tmux`). If you launch the
app from the Dock or Finder rather than a terminal, it resolves your login shell's `PATH` at
startup, so tools installed in `~/.local/bin` or `/opt/homebrew/bin` are found.

## Keyboard control

Everything the app does can be driven from the keyboard. Press `⌘?` (Ctrl+? elsewhere) to open the
reference inside the app: it is generated from the same table that dispatches the shortcuts, so it
cannot drift from what actually works.

Shortcuts use a modifier on purpose. A focused terminal forwards every bare keystroke to the agent
running in it, so an unmodified shortcut would steal the agent's input. `mod` below is **Cmd on
macOS** and **Ctrl elsewhere**.

### Panels

| Chord | Action |
|---|---|
| `mod+,` | Open settings |
| `mod+4` | Toggle extensions |
| `mod+5` | Toggle repomind |
| `mod+6` | Cycle theme (system, dark, light) |
| `mod+k` | Open the control center |

### Layout

| Chord | Action |
|---|---|
| `mod+shift+1` | Focused layout, one pane |
| `mod+shift+2` | Split layout, active pane plus its peer |
| `mod+shift+0` | Grid layout, up to six panes |

### Fleet

| Chord | Action |
|---|---|
| `mod+/` | Filter the fleet |
| `mod+u` | Show only lanes needing attention |
| `mod+r` | Refresh |
| `mod+n` | New lane |
| `mod+shift+n` | Add repository |
| `mod+g` | Jump to a lane needing attention |
| `mod+shift+h` | Hide the selected lane's project |

### Lane

These need a selected lane. With nothing selected they do nothing.

| Chord | Action |
|---|---|
| `mod+e` | Spawn agent |
| `mod+t` | Open terminal |
| `mod+p` | Pin or unpin lane |
| `mod+d` | Delete lane (asks first) |
| `mod+shift+m` | Merge lane (asks first) |
| `mod+.` | Stop the agent in the visible pane (asks first) |

### Agents

| Chord | Action |
|---|---|
| `mod+[` | Previous agent tab |
| `mod+]` | Next agent tab |

### Terminals

| Chord | Action |
|---|---|
| `shift+escape` | Leave the terminal, back to the fleet list |
| `mod+shift+f` | Find in the terminal |

`shift+escape` rather than plain Escape is deliberate: Claude Code uses Escape to interrupt its own
work, so the terminal keeps it. Once focus is on the fleet list, `j`/`k` and the arrow keys move the
selection and `/` jumps to the filter.

## Settings

**Settings > General** holds the default agent, the worktree path template, the auto-continue
message, and the behavior toggles (auto-continue rate-limited agents, prompt on spawn, probe
account usage, expand multi-agent lanes, embedded terminal renderer). The updater lives at the
bottom.

**Notifications** has a master switch plus one toggle per event: needs-you, rate-limited, resumed,
idle, sound, show-why, coalescing, click-to-focus, and whether subagents count.

**Appearance** sets the accent from a swatch or a custom hex value, picks the repomind agent and
model, and holds **Sort projects by activity**: with it on, sidebar project groups order by their
most recent lane activity so whatever you are working in floats to the top. Only the groups move.
Lane order inside a group is deliberately left alone, because sorting lanes by activity makes them
bubble around on every line an agent prints.

**Keyboard** is the shortcut reference, with search.

Settings are stored by the daemon and shared with the TUI, so a change here shows up there too.
Nothing saves until you press **Save**.

## Hiding projects

A project you are not working in can be hidden from the sidebar with the `⊘` button on its header
or `mod+shift+h`. Hiding is not removing: the repo stays registered, stays watched, and keeps every
lane and worktree it owns. Its lanes leave the sidebar and stop counting toward the needs-you and
running totals, and a **Hidden (N)** list at the bottom of the sidebar brings any of them back.

The flag lives in the daemon, so the TUI honors it too and it survives a restart. The TUI has no
unhide view of its own, so a project hidden there stays hidden until you restore it here.

## Extensions

The Extensions view manages Claude Code marketplaces, plugins, and skills, either globally or
scoped to one repository.

It is account-aware. If you run more than one Claude account (a default `~/.claude` plus a variant
such as `~/.claude-work`), an account picker appears and every listing and action targets the
account you choose. Codex is listed too, but it uses a different extension model, so it shows an
empty state rather than pretending to have Claude-style plugins.

## Known gaps

- On Windows and Linux, `mod` is Ctrl, which is also the terminal's own control modifier. A bound
  Ctrl chord pressed while a terminal is focused currently fires the GUI action **and** reaches the
  agent. macOS is unaffected, since Cmd is not a terminal control key.
- Hiding a project can only be undone from Mission Control. The TUI honors the flag but has no
  reveal list, so it cannot unhide.
- The iOS companion app is built but unreleased.
