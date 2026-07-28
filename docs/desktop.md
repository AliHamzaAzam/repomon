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

## The app icon

The icon is authored as an Icon Composer bundle at
`design/repomon-logo/macos-liquid-glass/Repomon.icon`: layered SVGs plus a manifest describing the
glass, refraction, and lighting. On macOS 26 and later the system renders those layers live, so the
icon picks up appearance tinting and specular response instead of being a flat picture of them.

Regenerate the shipped assets from it with `actool`:

```bash
xcrun actool --compile <out> --app-icon Repomon \
  --output-partial-info-plist <out>/partial.plist \
  --platform macosx --minimum-deployment-target 11.0 --include-all-app-icons \
  design/repomon-logo/macos-liquid-glass/Repomon.icon <empty>.xcassets
```

That emits `Assets.car` (copied to `src-tauri/macos/`, placed in the bundle by
`bundle.macOS.files`, and selected by the `CFBundleIconName` in `src-tauri/Info.plist`) and a
static `Repomon.icns` used as `icons/icon.icns`. The remaining PNG sizes and `icon.ico` are scaled
from actool's 256px render, which is the largest it emits and is also the ceiling for every entry
in the bundle's icon list. Older macOS, Windows, and Linux ignore the catalog and get that static
render.

The in-app mark (`src/components/BrandMark.tsx`) is the same glyph drawn from theme tokens rather
than the icon's fixed gradient, so it follows the light/dark setting and your chosen accent. Only
the OS-level icon is the glass artwork.

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
| `mod+shift+5` | Repomind full screen |
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
| `mod+shift+b` | Edit the selected lane's project notes |

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

Mission Control asks for notification permission on first launch. That request is what registers
it with Notification Center, and it is why its alerts carry the repomon icon; decline it and the
app posts nothing, leaving only the daemon's fallback below.

It also holds **System popup when no window is open**. The daemon posts its own OS notification
when no UI is covering one, which on macOS goes out through `osascript` and so arrives from Script
Editor, wearing Script Editor's icon. Turn it off and that popup stops: Mission Control still
notifies under its own identity while it is running, and the TUI still pops its own while it is on
screen. The trade is that a machine running neither UI stops notifying at the OS level, which is
why it ships on.

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

## Repo notes

Every registered project has a notes file: conventions, build and test commands, merge
preferences, gotchas, anything worth telling a worker every time. repomind reads them when it
plans and folds them into the prompts of agents it spawns there, so this is where you write
something the orchestrator will still know next week.

Open them from `mod+shift+b`, or right-click a project header in the sidebar and choose **Repo
notes**. They are plain markdown on disk under the daemon's data directory and stay editable
outside the app, so the editor loads fresh each time rather than caching. The 8 KB cap is enforced
by the daemon; the editor counts bytes (not characters) against it so a doomed save is refused
before it is sent.

## The orchestration journal

repomind writes every action it takes to a journal the daemon owns: what it did, which lane and
repo it touched, and whether it worked. **Control center > Journal** (`mod+k`) shows it newest
first, with a search box over the history.

An entry that names a lane is clickable and jumps you to that lane. Opening the tab shows the
recent tail rather than a search, so it doubles as "what happened while I was away".

## Playbooks

When repomind finishes a multi-lane goal it drafts a playbook: the pattern, the per-repo steps,
the worker prompts that worked, the failure modes it hit. **Control center > Playbooks** lists
them.

A draft is inert. repomind is only offered a playbook back once you approve it, which is
deliberate: instructions the orchestrator wrote feeding into its own future prompts unreviewed is a
self-poisoning path. Approve is only reachable once you have opened a playbook and its text is on
screen, so nothing can be waved through from the list.

A playbook that was approved and then re-drafted reads **approved · revision pending**: the old
approved text is still what repomind follows, and the revision waits for you. Deleting asks first,
since the procedure took real work to earn; approving does not, because reading it and clicking
Approve is the review.

## Standing orchestrations

**Control center > Schedules** runs repomind on a timer without you starting it. Add one with a
spec, a goal, and optionally an action cap; results arrive as notifications and land in the
journal.

The spec grammar is `daily HH:MM`, `weekdays HH:MM`, `weekends HH:MM`, `every Nm`, or `every Nh`.
The app deliberately does not re-implement that grammar to pre-validate your input, because a
second copy would drift from the daemon's; a bad spec comes back with an error that names the
accepted forms.

Unattended runs are bounded harder than attended ones: a lower action cap, and repomind refuses to
merge or delete a lane when nobody is watching. It reports and recommends instead. Leaving the cap
blank uses the daemon's conservative default rather than sending zero, which would produce a
schedule that fires and does nothing.

## Approval policy

**Control center > Approvals** lists the command patterns repomind may approve on your behalf,
grouped by project. These are learned: after you approve the same pattern in the same repo enough
times, repomind proposes a rule and you confirm it. Revoke any of them here.

Two limits are structural, not settings. Destructive commands always reach you no matter what is
listed here, and a denial is never generalised into an auto-deny, it just keeps escalating. Rules
are per-repo, so `cargo test` approved in two projects is two rules and revoking one leaves the
other standing.

## Repomind

The repomind panel lives in the right sidebar and opens with `mod+5`. `mod+shift+5` blows it up to
full screen, and Escape or **Exit** brings it back; going full screen opens the panel if it was
closed, so it has somewhere to shrink back to.

**Answering prompts.** Repomind's agent sometimes stops on something only you can answer, like
Claude Code's "Do you trust this folder?" trust prompt. The message box types text and presses
Enter, which cannot express "just press Enter" or "press Escape", so a prompt like that used to be
unanswerable from the app: the question was visible and there was no way through it. The key row
above the pane sends those directly. `1` `2` `3` pick a numbered option, **Enter** confirms the
highlighted one, and **Esc** cancels. The row's label turns amber and reads **Answer** while the
daemon reports the pane is waiting on a permission or a decision.

Note that the panel's **Esc** button sends Escape to repomind, while pressing Escape on the
keyboard leaves full screen. They are deliberately different: one is aimed at the agent, the other
at the window.

The live pane is a raw terminal capture, so it is stripped of escape sequences before display.
Colour is lost, but the text is readable; the alternative was the literal bytes.

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
