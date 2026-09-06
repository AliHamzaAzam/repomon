# Mission Control (desktop app)

Download the app, then add one repository. This guide covers the desktop fleet, editor, Usage and Repomind.

**You are here:** desktop setup and daily controls. `mod` means Cmd on macOS and Ctrl on Windows/Linux.

## Find your next task

| Task | Go to |
|---|---|
| Install | [Open section](#install) |
| Permissions for agents | [Open section](#permissions-for-agents) |
| The app icon | [Open section](#the-app-icon) |
| Keyboard control | [Open section](#keyboard-control) |
| Settings | [Open section](#settings) |
| When the daemon will not start | [Open section](#when-the-daemon-will-not-start) |
| Hiding projects | [Open section](#hiding-projects) |
| Repo notes | [Open section](#repo-notes) |
| The orchestration journal | [Open section](#the-orchestration-journal) |
| Playbooks | [Open section](#playbooks) |
| Standing orchestrations | [Open section](#standing-orchestrations) |
| Approval policy | [Open section](#approval-policy) |
| Repomind | [Open section](#repomind) |
| Git explorer | [Open section](#git-explorer) |
| In-app editor | [Open section](#in-app-editor) |
| Extensions | [Open section](#extensions) |
| Usage | [Open section](#usage) |
| Known gaps | [Open section](#known-gaps) |

<details>
<summary>Why this guide exists</summary>

Mission Control is repomon's desktop client: the same fleet the TUI drives, in a window, with
embedded terminals for every agent.

It talks to the same daemon over the same socket, so the TUI, the desktop app, and the iOS
client can all watch one fleet at once.

The [GUI demo recorder](../scripts/record-gui-demo.sh) stages an isolated, entirely synthetic
fleet for a 90-second tour of multitasking, Git, the editor, Usage, Repomail, Supervision and
Repomind.

Run `--dry-run` to verify the sandbox or `--dry-run --tour` to rehearse without recording.

The operator runs the capture from a macOS terminal with Screen Recording and Accessibility
permission; `--still` captures the same opening hero frame at 1440x900.

See [the recorder guide](../scripts/gui-demo/README.md) for the exact commands and isolation
details.

</details>

## Install

Choose your operating system in the table, then complete the three steps.

**Time: 3-5 minutes; agent installation can take another 5-10 minutes.**

1. Download the release file for your OS from the link below.
2. Install it and open Repomon.
3. Use onboarding to add a repo and choose an installed agent CLI.

**You know it worked when:** your repo and its main lane appear in the fleet.

| Platform | File |
|---|---|
| macOS (Apple silicon and Intel) | `Repomon_<version>_universal.dmg` |
| Windows | `Repomon_<version>_x64-setup.exe` |
| Linux | `Repomon_<version>_amd64.AppImage`, `.deb`, or `.rpm` |

<details>
<summary>Details</summary>

Download published builds from the [latest
release](https://github.com/AliHamzaAzam/repomon/releases/latest).

Previews are also published to the moving
[`desktop-preview`](https://github.com/AliHamzaAzam/repomon/releases/tag/desktop-preview)
release for macOS, Windows, and Linux:

The app updates itself: it checks the same release on launch and from **Settings > General >
Check for updates**, so you only download by hand once.

The daemon ships inside the bundle, so the desktop app needs no separate `repomond`.

On macOS and Linux it also ships its own portable `tmux` and falls back to it automatically when
none is installed, so nothing extra needs installing there either; Windows never needed tmux,
since it uses the built-in ConPTY host instead.

If you launch the app from the Dock or Finder rather than a terminal, it resolves your login
shell's `PATH` at startup, so tools installed in `~/.local/bin` or `/opt/homebrew/bin` are
found.

A **first-run setup wizard** takes a new install through seven steps: a welcome screen that says
what Repomon does and what a lane and a worktree are, a system check (the same
tmux/git/agent-CLI probes as Settings > System, below, with install commands and a re-check
button), adding your first repositories, choosing a default agent from the CLIs that were
actually found, turning on desktop notifications and the needs-you alert, an introduction to the
Repomind home at `~/repomind` with an option to start it when setup finishes, and a summary of
what was set up.

A rail across the top shows where you are and takes you back to any step you have already seen;
Back and Continue sit in the footer, Enter continues, and Skip setup (or Esc) leaves for the app
at any point.

Each move is written to local storage, so quitting mid-way to install a CLI or clone a repo
resumes on the same step rather than at the start.

Settings > General > Replay Onboarding reopens it from the top.

On Windows, one custom title bar contains the app toolbar and native-style minimize, maximize,
and close controls.

There is no second system title bar above it.

</details>

## The app icon

Use the maintainer-local brand sources only when you need to regenerate packaged icons. These scripts live in the maintainer checkout, not a fresh clone.

**Time: 5-10 minutes with Xcode and the saved Icon Composer document ready.**

1. Save material, transparency, color and lighting edits in Icon Composer.
2. From the maintainer repository root, build the saved macOS document and install the rendered outputs into the repository.

```bash
python3 docs/brand/final/tools/build-macos.py
python3 docs/brand/final/tools/install-macos.py
python3 docs/brand/final/tools/install-windows.py
```

3. Inspect the generated appearance previews and verify the tracked catalog and fallback icons.

**You know it worked when:** the build reports unchanged Composer source files and the installers finish with the rendered outputs in place.

To update an existing app’s icon, run this optional command. It replaces only icon assets, backs them up and verifies the executable is unchanged; no bundle rebuild or executable restart is needed.

```bash
python3 docs/brand/final/tools/install-macos.py --app /Applications/Repomon.app
```

<details>
<summary>Details</summary>

The brand sources are kept outside version control (`docs/brand/` is gitignored and lives on the
maintainer's machine); the repository ships only the rendered icons under
`apps/desktop/src-tauri/icons` and `apps/desktop/src-tauri/macos/Assets.car`.

The approved geometry is `docs/brand/repo-logo-final.svg`, including the user's manual path
adjustments.

Preserve that file's paths, stroke widths, openings, and central square.

The macOS Icon Composer bundle at `docs/brand/final/macos/Repomon.icon` expands the source
strokes into closed filled outlines and separates the mesh and square into two SVG layers.

This avoids the macOS 26 renderer filling across open stroke interiors when applying layer
colors.

The source SVG remains unchanged.

Icon Composer 2.0 and the Xcode 27 toolchain supply the Liquid Glass lighting, refraction,
masking, and appearance variants.

The SVG layers contain no baked effects.

Save material, transparency, color, and lighting edits in Icon Composer before running the macOS
build.

The build compiles the saved `.icon` document as-is and checks that its files remain
byte-identical; it never resets Composer settings.

If the canonical SVG geometry changes, run `python3
docs/brand/final/tools/export-macos-layers.py` explicitly first to refresh only the two vector
layers while preserving `icon.json`.

This backs up and replaces only `Assets.car` and `icon.icns`, then verifies the executable is
unchanged.

Saving in Icon Composer does not automatically update compiled or installed app icons.

Dock or Finder can retain the previous cached image until the app is reopened or the icon
refreshes.

The macOS build needs Xcode's Icon Composer and native rendering services.

It emits six 1024px appearance previews, `Assets.car`, and the native static `Repomon.icns`
fallback, plus a 256px preview extracted from the fallback for compatibility inspection.

The installer copies the catalog to `src-tauri/macos/` and the ICNS to
`src-tauri/icons/icon.icns`.

`bundle.macOS.files` packages the catalog, and `CFBundleIconName=Repomon` selects it on modern
macOS.

Older macOS uses the native ICNS fallback.

Windows and Linux use a separate flat warm-white tile with the exact approved graphite and
orange mark.

The Windows build requires `rsvg-convert`, renders each size directly from the SVG, and packs a
multiresolution RGBA ICO.

It also regenerates the Store logo sizes.

These PNGs are not downsized from the macOS glass artwork.

Preview and provenance files live in `docs/brand/final/`.

The in-app mark (`src/components/BrandMark.tsx`) is the same glyph drawn from theme tokens
rather than the icon's fixed palette, so it follows the light/dark setting and your chosen
accent.

Only the OS-level icon is the glass artwork.

</details>

## Keyboard control

Press `mod+?` to find a shortcut without leaving the app.

<details>
<summary>Details</summary>

Everything the app does can be driven from the keyboard.

Press `⌘?` (Ctrl+? elsewhere) or a bare `?` outside a text field to open a searchable cheat
sheet overlay; **Settings > Keyboard** holds the same reference plus a conflict check and a
printable page.

Both are generated from the same table that dispatches the shortcuts, so neither can drift from
what actually works.

Shortcuts use a modifier on purpose.

A focused terminal forwards every bare keystroke to the agent running in it, so an unmodified
shortcut would steal the agent's input.

`mod` below is **Cmd on macOS** and **Ctrl elsewhere**.

</details>

### Panels

Press the chord for the panel you need.

| Chord | Action |
|---|---|
| `mod+1` | Toggle the git explorer panel |
| `mod+2` | Toggle the compact in-app editor in the right rail |
| `mod+3` | Toggle the usage view |
| `mod+4` | Open the control center |
| `mod+5` | Toggle multitasking |
| `mod+6` | Toggle extensions |
| `mod+7` | Toggle the supervision panel |
| `mod+8` | Toggle the repomail panel |
| `mod+9` | Toggle repomind |
| `mod+,` | Open settings |
| `mod+k` | Open the control center (same as mod+4) |
| `mod+shift+9` | Repomind full screen |
| `mod+shift+t` | Cycle theme (system, dark, light) |
| `mod+p` | Find file in workspace |
| `mod+shift+f` | Search project files in workspace (inside a focused terminal, this searches the terminal) |
| `mod+shift+v` | Toggle preview (Markdown or SVG tab, whichever is active) |

#### File finder

Press `mod+p`, type a filename, then use the keys below to choose a result.

| Chord | Action |
|---|---|
| `ctrl+n` | Next result (also Down arrow) |
| `ctrl+p` | Previous result (also Up arrow) |
| `escape` | Close the finder |

<details>
<summary>Details</summary>

Git, repomind, and the compact editor share one right-rail panel host: a resizable pane (drag
its left edge) with one header button per tab.

Pressing a tab's chord (or clicking its header button) opens the rail on that tab if it is
closed, switches to that tab if the rail is open on another one, and closes the rail if it is
already open on that tab.

Each header button's active state is scoped to its own tab, so opening on Git does not light up
the Repomind button.

The **Repomind** button also carries a small state dot, the counterpart to Repomail's unread
badge: teal while a controller is running, amber (or red) when one wants you, and nothing at all
when the home is off.

</details>

### Editor workspace

Click **Editor** in the toolbar to open the full workspace.

**Time: 2 minutes.**

1. Click **Editor**.
2. Use `mod+p` to find and open a file.
3. Edit the file, then press `mod+s`.

**You know it worked when:** the file tab no longer has a dirty marker.

<details>
<summary>Details</summary>

The toolbar **Editor** button is a split button: - Clicking **Editor** opens the full
center-pane Editor workspace, mutually exclusive with Multitasking. - Clicking the split chevron
(or pressing `mod+2`) toggles the compact side-rail editor.

Both the center workspace and the rail editor share the same in-memory editor store.

Open tabs, unsaved buffer edits, cursor positions, and scroll offsets remain intact when moving
between the center workspace and the rail editor, or when switching between lanes.

The center Editor workspace provides: - A resizable file tree on the left (persisted width, min
180px, default 240px) with real-time filter search, SVG file-kind icons, keyboard navigation
(arrow keys to navigate, Enter to open, Left and Right to collapse or expand), and a locate
button to reveal the active file. - A multi-tab editor in the center supporting syntax
highlighting across every language, bracket auto-closing, code folding, word completion,
multi-cursor editing, and image/binary/PDF viewers. - A status line at the bottom showing the
syntax language (clickable to override), cursor position (line and column), detected indentation
unit, and one-click toggles for line wrapping and rendering whitespace. - For Markdown buffers
(`.md` and `.markdown`), a live preview toggle in the status line (or `mod+shift+v`) opens a
resizable side-by-side split.

The editor and preview maintain scroll synchronization aligned to the nearest heading, with
support for task lists, GitHub tables, fenced code highlighting, local image resolution, and
external links. - For SVG buffers, the same preview toggle (and `mod+shift+v`) opens the
rendered SVG in the same resizable split, live from the unsaved buffer (sanitized before
render).

`.svg` files stay text tabs otherwise - full syntax editing, not a dedicated image viewer. - The
image viewer (PNG, JPEG, GIF, WebP, BMP, ICO) streams bytes through the asset protocol, fills
the tab, and supports fit/actual-size/free zoom (10-800%), `Mod` plus scroll-wheel zoom around
the cursor, drag-to-pan, and double-click to toggle fit and actual size.

</details>

### Editor

Focus the editor before using these CodeMirror shortcuts.

| Chord | Action |
|---|---|
| `mod+s` | Save file |
| `mod+/` | Toggle line comment |
| `mod+d` | Select next occurrence |
| `mod+g` | Go to line |
| `alt+up` | Move line up |
| `alt+down` | Move line down |
| `shift+alt+down` | Copy line down |

<details>
<summary>Details</summary>

CodeMirror's own bindings, live only while the editor has focus - these are not in the global
dispatcher above, so they cannot be remapped from Settings and do not appear if you search the
overlay without the editor focused.

</details>

### Layout

Choose one pane, a split, or a grid with the keys below.

| Chord | Action |
|---|---|
| `mod+shift+1` | Focused layout, one pane |
| `mod+shift+2` | Split layout, active pane plus its peer |
| `mod+shift+0` | Grid layout, up to six panes |

### Fleet

Select a lane or filter the fleet with the keys below.

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

#### The sidebar

Click a lane to focus it; use **Needs you** to find agents awaiting attention.

<details>
<summary>Details</summary>

Two lines per lane, with the same anatomy on every row so the columns can be read down rather
than across.

The first line is the health dot, the lane name (the part that gives way if the sidebar runs out
of room), and the most urgent status, one short word from a fixed vocabulary that never
truncates.

The second line is the branch (truncating from its start so the identifying tail stays visible,
unlike the name), the agent count or agent icon, and one change cell holding either uncommitted
files or divergence from upstream (the tooltip carries both, and the status pill's tooltip
carries the daemon's fuller explanation).

The sidebar scrolls once a project's lanes outgrow the visible height.

A project header names its own count ("12 lanes") and carries an amber pip with the number of
its agents that need you, so a collapsed project still reports.

Hidden projects sit behind one disclosure at the bottom; expanding gives one dense line each,
with an unhide button on hover.

Above the projects sits one pinned **Repomind** row, in the same two-line anatomy.

Its first line is the brain mark, the name, the needs-you pip, and the same one-word status pill
every lane carries, reading "off" when no controller is running.

Its second line is the home path and what the home holds: the number of controllers in the lane
and the number of goals in `plans/active`.

Clicking it focuses the controller lane's agents in the terminal bay exactly as clicking a lane
row does; right-clicking offers **Start Repomind** (or **Stop Repomind**), **Open panel**, and
**Open home in editor**.

The row is the sidebar's first stop, so `j` / `k` and the arrow keys walk from it into the
project groups.

The repomind home is not one of those groups.

It is registered like any other repo, but the sidebar files it under the pinned row instead, and
its controllers are counted there rather than in the project headers or the "Needs you" and
"Running" chips, so nothing is counted twice.

A worktree lane whose branch is already contained in the repository's default branch is marked
"merged": the work landed, and the worktree is bookkeeping.

Right-clicking a lane row offers "Pin lane to top" and, for a worktree lane, "Remove worktree",
which raises the usual confirm and keeps the branch.

The two filter chips ("Needs you", "Running") are toggles over the same per-agent state the rows
show, so a chip's number always matches the rows it selects.

Below a measured width they drop their labels for an icon plus the count, with the full label
moved to the tooltip, rather than truncating the text mid-word.

</details>

#### Agent status

Read the status pill, then hover it for the daemon’s reason.

| State | Means |
|---|---|
| running | the pane is working: a spinner or streaming marker is on screen, background subagents are going, or the transcript is still being written |
| needs you | the turn ended with nothing pending, and the agent is waiting for the next instruction |
| decision | a permission, plan or question dialog is open on the pane |
| stalled | a running agent whose pane has not changed for longer than the stall threshold |
| limited | paused on a usage limit; repomon auto-continues it at the reset time |
| idle | at its prompt with no dialog and nothing running |
| external | a session running outside repomon, adoptable but not managed |
| inferred | the worktree is changing but the agent behind it could not be identified |
| exited | the agent's process ended; nothing is running in the pane |

<details>
<summary>Details</summary>

Every pill, chip, count and filter in the sidebar is derived from one per-agent state, so a lane
row and the chip above it can never disagree.

The daemon decides the state; the frontend only projects it, and never re-reads pane text or
runs timers of its own.

The pane outranks the transcript on liveness, in both directions.

A transcript only records when the last message landed, so it decays to idle through a long tool
call and reads "needs you" through a turn whose background agents are still working; both of
those are panes the operator can see are busy.

The other way round, an agent repomon has no transcript for (Codex, Antigravity, aider) is
judged by how recently its files moved, which a background task the agent started keeps moving
after the turn is over: a pane with no dialog, no subagent and no spinner ends that, reading
"background file activity only, pane at rest" rather than "no output for 1s".

An agent whose transcript shows a turn genuinely mid flight is never demoted this way, because a
capture taken between redraws can miss a spinner; a frozen pane surfaces as stalled instead.

A dialog outranks everything: a pane asking a question is not working, whatever else is on
screen.

Classification runs on the daemon every two seconds for every managed session, whatever any
client has in view and whether or not notifications are on, and each status transition is pushed
as `event.agent.status` (`lane_id`, `session`, `window`, `status`, `reason`, `previous`).

The sidebar also polls, so a status change lands within one heartbeat even if nothing is pushed.

Counts are agent counts, never lane counts. "Needs you" counts agents in needs you, decision,
stalled or limited; "Running" counts agents in running.

A lane pill shows the most urgent state among its agents plus, for running, how many of them are
in it.

Hovering a pill shows the daemon's `status_reason` for the agents behind it ("spinner on screen:
Thinking (2m 14s)", "dialog pending: Bash", "no output for 41m"), so a status you do not believe
can be reported rather than merely doubted.

</details>

### Lane

Select a lane before running one of these actions.

| Chord | Action |
|---|---|
| `mod+e` | Spawn agent |
| `mod+t` | Open terminal |
| `mod+shift+p` | Pin or unpin lane |
| `mod+d` | Delete lane (asks first) |
| `mod+shift+m` | Merge lane (asks first) |
| `mod+.` | Stop the agent in the visible pane (asks first) |

<details>
<summary>Details</summary>

These need a selected lane.

With nothing selected they do nothing.

</details>

### Agents

Select a lane, then move between its agent tabs.

| Chord | Action |
|---|---|
| `mod+[` | Previous agent tab |
| `mod+]` | Next agent tab |

### Terminals

Press `shift+escape` to return keyboard focus to the fleet.

| Chord | Action |
|---|---|
| `mod+shift+f` | Find in the terminal |
| `shift+escape` | Leave the terminal, back to the fleet list |

<details>
<summary>Details</summary>

`shift+escape` rather than plain Escape is deliberate: Claude Code uses Escape to interrupt its
own work, so the terminal keeps it.

Once focus is on the fleet list, `j`/`k` and the arrow keys move the selection and `/` jumps to
the filter.

</details>

## Settings

Open **Settings** with `mod+,`, then choose the tab for your task.

<details>
<summary>Details</summary>

**Settings > General** holds the default agent, the worktree path template, the auto-continue
message, and the behavior toggles (auto-continue rate-limited agents, prompt on spawn, probe
account usage, expand multi-agent lanes, embedded terminal renderer).

The updater lives at the bottom.

**System** shows live checks for `tmux` (system install or the bundled sidecar), `git`, and
every configured agent CLI, each with a one-click-copy install command when it is missing.

This is the same check the first-run wizard runs and the footer connection pill opens when it
has something to flag.

It also holds the daemon's self-service controls: stop, start, or reset the daemon, and
bulk-restore agent sessions left orphaned by a crash or an update.

Two rows at the top of that tab do not go through the daemon, because they are what you need
when the daemon is the thing that is broken:

- **Bundled Daemon** runs `repomond --version` from the copy inside this bundle and reports the
  exact result. A daemon that cannot start at all (on Windows, a missing Visual C++ runtime kills
  it in the loader with `0xC0000135` before a single line reaches its log) shows up here as "Does
  not start", with the failure, the one line that fixes it, and a **Show log** button. Every other
  row on this tab goes blank in exactly that situation, which is why this one asks the binary
  directly.
- **Command-line tools** installs the `repomon` CLI out of the bundle. macOS and Linux get symlinks
  to `repomon` and `repomond` in `~/.local/bin`, so an app update carries through without
  reinstalling. Linux AppImage installs use copies instead because their mount disappears on
  exit; remove and install these tools again after app updates. Windows gets copies of `repomon.exe`, `repomond.exe`, and `repomon-agent-host.exe`
  in `%LOCALAPPDATA%\repomon\bin` plus that directory on the **user** PATH in
  `HKCU\Environment` (the machine PATH is never touched, so no elevation is needed). The card
  reports the installed version by running it, says whether a terminal can find the directory (it
  asks your login shell, not the app's own stripped `PATH`), and gives you the exact `export PATH`
  line to paste when it cannot. Shell probes run in the background with a two-second limit; a
  failed probe shows "could not read your shell PATH" and leaves PATH status unknown. Existing
  regular binaries in `~/.local/bin` move to `<name>.bak`, and an existing backup is preserved.
  **Remove** restores these backups after removing the app-installed links or copies, and removes
  the PATH entry on Windows. The last step of the setup wizard offers the same card.

**Agents** lets you add or remove custom agent CLIs (a name plus the launch command) and set the
default agent, without hand-editing `config.toml`.

See `docs/agents.md` for the underlying `agent.add`/`agent.remove`/`agent.set_default`
mechanism, which this tab is a UI over.

**Notifications** has a master switch plus one toggle per event: needs-you, rate-limited,
resumed, idle, sound, show-why, coalescing, click-to-focus, and whether subagents count.

Mission Control asks for notification permission on first launch.

That request is what registers it with Notification Center, and it is why its alerts carry the
repomon icon; decline it and the app posts nothing, leaving only the daemon's fallback below.

It also holds **System popup when no window is open**.

The daemon posts its own OS notification when no UI is covering one, which on macOS goes out
through `osascript` and so arrives from Script Editor, wearing Script Editor's icon.

Turn it off and that popup stops: Mission Control still notifies under its own identity while it
is running, and the TUI still pops its own while it is on screen.

The trade is that a machine running neither UI stops notifying at the OS level, which is why it
ships on.

**Appearance** defaults to the brand orange accent with graphite ink and a warm light ground (or
lifted graphite on a dark ground).

It sets the accent from a swatch or a custom hex value, picks the repomind agent and model, and
holds **Sort projects by activity**: with it on, sidebar project groups order by their most
recent lane activity so whatever you are working in floats to the top.

Only the groups move.

Lane order inside a group is deliberately left alone, because sorting lanes by activity makes
them bubble around on every line an agent prints.

**Policies** holds the standing rules: **Approvals** and **Supervision** defaults, as its own
sub-tabs.

The control center's `⌘K` search can jump straight here via "Open Policies".

Playbooks, standing duties, and the journal are not settings; they live in the Repomind panel
(`mod+9`), and the tab says so at the bottom.

**Keyboard** is the shortcut reference, with search, a conflict check (flags any two shortcuts
that share both a chord and a scope), a warning on Windows and Linux about the
Ctrl-reaches-the-terminal caveat (see Known gaps), and a **Print cheat sheet** button that opens
a plain, printable page listing every shortcut.

Most settings are stored by the daemon and shared with the TUI; the modal saves those with
**Save**.

Usage tracking, price refresh, and model-rate edits apply immediately.

The sidebar cost switch is a local desktop preference and also applies immediately.

If the daemon connection drops for more than a few seconds, a banner appears rather than letting
the UI sit silently stale; it clears as soon as the connection is restored.

</details>

## When the daemon will not start

Open **Settings > System > Bundled Daemon** and read the reported cause.

**Time: 2 minutes.**

1. Read **Bundled Daemon** in Settings > System.
2. Choose **Show log** to inspect `repomond.out.log`.
3. Choose **Copy diagnostics** when reporting the failure.

**You know it worked when:** the diagnostics include the endpoint, resolved daemon path, error and log tail.

<details>
<summary>Details</summary>

The app spawns `repomond` detached and windowless, so a spawn that fails or a daemon that exits
at once used to show up as a connection pill saying "Retrying" and nothing else.

It now watches the child for its first three seconds and reports what actually happened:

- **The binary is not there.** A broken or partial install; the pill names the path it looked at.
- **The OS refused to start it.** The pill carries the spawn error.
- **It started and exited immediately.** The pill carries the exit code (in hex too, since that is
  the form every Microsoft page uses), and the tail of the daemon log goes into the diagnostics
  block.
- **It is running but nothing answers the endpoint.** The pill carries the connect error and the
  endpoint as the platform names it, which on Windows is the pipe name, not a file path.

Two Windows causes get a plain sentence instead of a status code.

`0xC0000135` (`STATUS_DLL_NOT_FOUND`, a missing Visual C++ runtime) and `0xC000007B`
(`STATUS_INVALID_IMAGE_FORMAT`, a wrong-architecture one) both resolve to "install the x64
redistributable", with the link.

A named pipe another session already owns (access denied, or all pipe instances busy) resolves
to "end the stale `repomond.exe` in Task Manager".

The retrying rail offers **Show log**, which opens `repomond.out.log` in the system's text
viewer, and **Copy diagnostics**, which puts the app version, the OS, the endpoint, the last
error, the resolved daemon path, the log path, and the log tail on the clipboard in one block.

</details>

## Hiding projects

Hide a project when you want its lanes out of the sidebar without removing them.

**Time: 30 seconds.**

1. Select the project and press `mod+shift+h`.
2. To restore it, expand **Hidden (N)** and choose its unhide button.

**You know it worked when:** the project disappears and then returns with its lanes intact.

<details>
<summary>Details</summary>

A project you are not working in can be hidden from the sidebar with the `⊘` button on its
header or `mod+shift+h`.

Hiding is not removing: the repo stays registered, stays watched, and keeps every lane and
worktree it owns.

Its lanes leave the sidebar and stop counting toward the needs-you and running totals, and a
**Hidden (N)** list at the bottom of the sidebar brings any of them back.

The flag lives in the daemon, so the TUI honors it too and it survives a restart.

The TUI has no unhide view of its own, so a project hidden there stays hidden until you restore
it here.

</details>

## Repo notes

Open the selected project’s notes with `mod+shift+b`.

**Time: 2 minutes.**

1. Select a project and press `mod+shift+b`.
2. Write its conventions, build commands or review requirements.
3. Save within the 8 KB byte limit.

**You know it worked when:** reopening the notes shows the saved text.

<details>
<summary>Details</summary>

Every registered project has a notes file: conventions, build and test commands, merge
preferences, gotchas, anything worth telling a worker every time. repomind reads them when it
plans and folds them into the prompts of agents it spawns there, so this is where you write
something the orchestrator will still know next week.

Open them from `mod+shift+b`, or right-click a project header in the sidebar and choose **Repo
notes**.

They are plain markdown on disk under the daemon's data directory and stay editable outside the
app, so the editor loads fresh each time rather than caching.

The 8 KB cap is enforced by the daemon; the editor counts bytes (not characters) against it so a
doomed save is refused before it is sent.

</details>

## The orchestration journal

Open **Repomind > Memory > Activity** to inspect recent actions.

<details>
<summary>Details</summary>

repomind writes every action it takes to a journal the daemon owns: what it did, which lane and
repo it touched, and whether it worked.

The Repomind panel's **Memory** section has an **Activity** button that opens it newest first,
so the record sits beside the memory it describes rather than behind a settings modal.

</details>

## Playbooks

Open a draft and read it before choosing **Approve** or **Reject**.

**Time: 3-5 minutes.**

1. Open **Repomind > Playbooks** and read a draft.
2. Choose **Approve** to make it reusable, or **Reject** to retain it under rejected drafts.

**You know it worked when:** the file moves out of drafts and the panel shows its new state.

<details>
<summary>Details</summary>

When repomind finishes a multi-lane goal it drafts a playbook: the pattern, the per-repo steps,
the worker prompts that worked, the failure modes it hit.

The Repomind panel's **Playbooks** section lists them.

A draft is inert. repomind is only offered a playbook back once you approve it, which is
deliberate: instructions the orchestrator wrote feeding into its own future prompts unreviewed
is a self-poisoning path.

Approve is only reachable once you have opened a playbook and its text is on screen, so nothing
can be waved through from the list.

A playbook that was approved and then re-drafted reads **approved · revision pending**: the old
approved text is still what repomind follows, and the revision waits for you.

Deleting asks first, since the procedure took real work to earn; approving does not, because
reading it and clicking Approve is the review.

</details>

## Standing orchestrations

Open **Repomind > Standing duties > Add** to schedule a goal.

**Time: 2 minutes.**

1. Choose **Add** under **Repomind > Standing duties**.
2. Enter a schedule such as `daily 09:00`, a goal, and optionally an action cap.
3. Save the duty.

**You know it worked when:** the duty shows its next run; later results appear in notifications and the journal.

<details>
<summary>Details</summary>

The Repomind panel's **Standing duties** section runs repomind on a timer without you starting
it.

**Add** opens an inline form there: a spec, a goal, and optionally an action cap.

Results arrive as notifications and land in the journal.

The spec grammar is `daily HH:MM`, `weekdays HH:MM`, `weekends HH:MM`, `every Nm`, or `every
Nh`.

The app deliberately does not re-implement that grammar to pre-validate your input, because a
second copy would drift from the daemon's; a bad spec comes back with an error that names the
accepted forms.

Unattended runs are bounded harder than attended ones: a lower action cap, and repomind refuses
to merge or delete a lane when nobody is watching.

It reports and recommends instead.

Leaving the cap blank uses the daemon's conservative default rather than sending zero, which
would produce a schedule that fires and does nothing.

</details>

## Approval policy

Open **Settings > Policies > Approvals** to review or revoke learned rules.

<details>
<summary>Details</summary>

**Settings > Policies > Approvals** lists the command patterns repomind may approve on your
behalf, grouped by project.

These are learned: after you approve the same pattern in the same repo enough times, repomind
proposes a rule and you confirm it.

Revoke any of them here.

Two limits are structural, not settings.

Destructive commands always reach you no matter what is listed here, and a denial is never
generalised into an auto-deny, it just keeps escalating.

Rules are per-repo, so `cargo test` approved in two projects is two rules and revoking one
leaves the other standing.

</details>

## Repomind

Press `mod+9` to open Repomind’s plans, playbooks and memory controls.

**Time: 2-3 minutes, plus agent startup.**

1. Open the panel with `mod+9` and choose **Start**.
2. Choose **Focus pane** for a controller to give it an instruction.
3. Use **Add goal** in Plans to record a title and one line of intent.

**You know it worked when:** the home path is visible, a controller has a pane, and the goal appears in Plans.

<details>
<summary>Details</summary>

The repomind panel is one tab of the right-rail panel host (alongside git and the editor, above)
and opens with `mod+9`.

`mod+shift+9` blows it up to full screen, and Escape or **Exit** brings it back; going full
screen opens the panel if it was closed, so it has somewhere to shrink back to.

**Repomind lives in a lane.** Starting repomind creates (once) a home repo at `~/repomind` - its
memory: plans, playbooks, per-repo notes, a journal - registers it like any other repo, and runs
the agent in that repo's lane, marked as the *controller lane*.

Everything the app already does with a lane therefore works on it: the terminal bay renders its
pane, Multitasking lists its agents (under their own **Repomind** group in the pane picker
rather than under a project), and Supervision edits its policy like any lane's.

That lane cannot be deleted or merged from the fleet tools, and a worker agent cannot spawn into
it.

The one place it is treated differently is the fleet sidebar, which gives it the pinned row
described above instead of a project group.

**Supervision defaults.** The first time the home is ensured, the daemon seeds the controller
lane with `hold` on the destructive dialog classes (deletion, push to a remote, credential
access, install, device access): a controller holds the full fleet catalog, so a permission
prompt in its lane is about the whole fleet rather than about one worktree.

Supervision itself stays off until you turn it on, exactly as for any other lane, and the seed
is written once - relax a class in **Settings > Policies > Supervision** and your choice survives every
restart.

**The panel is a control room, not a second chat.** The conversation with a controller happens
in its pane in the terminal bay, which is a real terminal with scrollback, dialogs and colour;
the panel holds the state that conversation is about.

It carries no composer, no live feed and no transcript, and every row here that names a
controller ends in a way back to its pane.

**The header** states the controller lane in the fleet's own vocabulary, the number of
controllers in it, and the lifecycle: **Start** or **Stop**, plus a **+** that spawns another
controller into the lane up to the configured maximum.

The line under it names the lane and the home path on disk.

Below that, five sections in one scrolling column:

- **Plans** lists one row per file in `plans/active`, with its title, its next step, its owner and
  when it last moved; clicking one opens that file in the editor on the home lane. **Add goal**
  takes a title and one line of intent, writes `plans/active/<slug>.md` in the home's frontmatter
  conventions, and then tells the primary controller the goal exists. A home with no controller
  running still gets its file, and the panel says so rather than pretending the handoff happened.
  **Done** on a row asks for a one-line outcome, then moves the file into `plans/done/` with
  `status: done` and that outcome appended.
- **Playbooks** is the approval gate. Drafts from `playbooks/drafts/` sit at the top with
  **Approve** and **Reject**; approved ones follow, each opening its file. Rejecting moves the
  draft into `playbooks/rejected/` rather than deleting it, so the text stays readable in the
  home's history. An approved playbook with a revision waiting appears on both lists: the approved
  text is what agents get, and the revision is still a decision you owe.
- **Standing duties** lists the schedules, each with its spec, its goal, its action cap, when it
  last ran and when it runs next, and **Remove** behind a confirmation. **Add** opens an inline
  form in the same section: a schedule, a goal, and an action cap, validated the way the daemon
  validates them.
- **Memory** answers whether the memory feeding all of this is current. The boot line says when the
  context was last assembled, how big it came out, and what the token budget left out, with
  **Regenerate** and **Open**. The export line says when the daemon's one-way export last ran,
  whether one is pending, and what failed if anything did, with **Export now**. Below them the
  journal browser picks a day out of `journal/` (newest first, with archived months behind a
  disclosure) and shows that day's entries, with **Open** for the day file itself.
- **Controllers** lists the agents in the lane with their status pill and the daemon's reason for
  it, each with **Focus pane**, which selects the lane and brings that controller's pane to the
  front of the terminal bay.

Every section reads the home through the ordinary lane file RPCs, because the home is an
ordinary lane.

They re-read when the home's own counts move, when an export lands, or when you act on something
here, never on a heartbeat of their own.

**A controller between instructions reads idle.** A worker that stops talking is waiting to be
picked up, so it reads "needs you".

A controller is a standing coordinator: sitting at the end of its turn is its resting state, and
you do not owe it an answer for that. "Needs you" on a controller therefore means a pending
dialog or an explicit question, and nothing else.

The pinned sidebar row, the toolbar dot and this panel all read it the same way, from the
attention word the daemon puts on the session.

**What a fresh repomind already knows.** It does not start blank.

Before every start, the daemon assembles a boot document from the home and hands it to the
agent, so the first thing in its head is your `REPOMIND.md` house rules, whatever standing facts
you keep in `profile/`, one line for each goal in `plans/active/` (its status, its owner, its
next step), yesterday's and today's journal, and a snapshot of every lane in the fleet with its
branch, agent count, and state.

That means you can open a brand new repomind and ask "what goals are active" or "who needs me"
and get a real answer without it calling a single tool.

The document is capped at about 12k tokens; when your home outgrows that, the oldest journal
goes first, then profile notes, then plans, and the document says at the end exactly what it
dropped.

It lives at `~/repomind/.repomind/boot.md`, is rewritten every time repomind starts, and is not
yours to edit: change the files it is built from instead.

Day files in `journal/` older than 90 days roll into `journal/archive/`, so the journal stays
the recent past rather than everything that ever happened.

**Answering a controller's prompt.** A controller sometimes stops on something only you can
answer, like Claude Code's "Do you trust this folder?" trust prompt.

That happens in its pane, and it is answered there: **Focus pane** in the Controllers section
puts the pane in front, where the full terminal, its dialog and its keys are.

Supervision can also answer routine classes for you, per lane, from **Settings > Policies > Supervision**.

</details>

## Git explorer

Select a lane, then press `mod+1` to inspect its changes.

**Time: 1 minute.**

1. Select a lane and press `mod+1`.
2. Click a changed file to read its unified diff, or click a commit for its metadata and patch.
3. Close the detail to return to the list.

**You know it worked when:** the selected file or commit’s patch appears in the panel.

<details>
<summary>Details</summary>

The git panel is another tab of the right-rail panel host (`mod+1`), scoped to the lane that is
currently focused: branch status against the repo's base branch (commits ahead, with diffstat),
the working-tree's changed and untracked files, and commit history.

Clicking a working-tree file opens a unified diff for it; clicking a commit in Branch or History
opens that commit's detail (message, author, full patch) via the `commit.show` RPC.

Opening a diff or a commit replaces the panel's list views rather than nesting a second scroll
region inside them; closing it returns to the list.

</details>

## In-app editor

Press `mod+2` for the compact editor, or click **Editor** for the full workspace.

<details>
<summary>Details</summary>

The editor is the third tab of the right-rail panel host (`mod+2`): a lazy file tree over the
focused lane's worktree, multi-file tabs with dirty tracking, and a CodeMirror 6 editor themed
to match every Repomon theme.

Saving is conflict-safe: if an agent (or anything else) changed the file on disk since it was
opened, the save is rejected and a banner offers **reload** (discard your edits and take the
on-disk version) or **keep mine** (leave your buffer as-is, still marked dirty, and try again)
instead of silently overwriting either side.

A file deleted on disk while you had it open shows the same banner, offering to write your
buffer back out as a fresh file.

Opening a `.pdf` shows the document itself rather than the "not a text file" notice: a slim
toolbar (file name, size, and an **Open in system viewer** button) over the webview's own PDF
renderer, which handles paging and zoom natively on macOS and Windows.

The file is streamed straight off disk through the Tauri asset protocol - scoped, on first open,
to that lane's worktree root - rather than round-tripped through an RPC's base64 payload, so
there is no size cap tied to the editor's normal read limit.

Linux's WebKitGTK webview has no built-in PDF renderer, so there the toolbar's button is the
only way to view the file; the same fallback appears on macOS and Windows if the preview fails
or never finishes loading.

A PDF tab is always read-only: it is never marked dirty, never saved, and closes without a
confirmation prompt.

</details>

## Extensions

Press `mod+6`, then choose the account and scope you want to manage.

<details>
<summary>Details</summary>

The Extensions view manages Claude Code marketplaces, plugins, and skills, either globally or
scoped to one repository.

It is account-aware.

If you run more than one Claude account (a default `~/.claude` plus a variant such as
`~/.claude-work`), an account picker appears and every listing and action targets the account
you choose.

Codex is listed too, but it uses a different extension model, so it shows an empty state rather
than pretending to have Claude-style plugins.

</details>

## Usage

Press `mod+3`, then choose **Today**, **7 days**, **30 days** or **Custom**.

**Time: 2 minutes.**

1. Press `mod+3` and choose a date range.
2. Read the cost and token cards, then click a chart bar to narrow the period.
3. Open a session row to inspect its token split or select its lane.

**You know it worked when:** the range dates, chart and session rows describe the same window; an unused window shows no usage.

<details>
<summary>Details</summary>

`mod+3`, or the Usage button in the header, opens the usage ledger: what every managed agent
burned in tokens, what those tokens would cost at published API rates, and what is worth
changing.

The ledger is local and passive.

A background pass reads the transcripts the agents already write to disk (Claude Code's
`~/.claude/projects` and any `~/.claude-*` account root, Codex's `~/.codex/sessions`,
Antigravity's brain transcripts, and OpenCode's SQLite store), records one row per billable
turn, and attributes each row to a repo and lane by the directory the turn ran in.

Nothing is sent anywhere and no API key is needed.

Sessions repomon did not start still appear, marked as running outside a lane.

Claude Code writes a subagent's turns to its own transcript, one directory below the session
file at `<project>/<session>/subagents/agent-<id>.jsonl`.

Those turns are read too and folded into the session that spawned them: same lane, same repo,
same row, with the sessions table's **Sub** column saying what share of the row's tokens the
subagents spent.

It writes an assistant message as one line per content block, all repeating the same running
`usage`, so a message is counted once, at the highest figure its lines reported, rather than
once per block.

A correction to how a transcript is counted also corrects the history already recorded: each
source carries the reader revision that read it, and ingest re-reads the ones an older revision
wrote, replacing what they produced.

That runs a bounded number of sources per pass, so a fresh version converges over a few minutes
rather than in one stall; `usage.status` says how many sources are still waiting.

The Usage view shows "Recounting N of M transcripts" while that queue is nonempty.

Each pass attempts at most 25 old sources and promptly schedules another pass when a full batch
leaves work.

Missing or unsupported sources retain previous events and leave the queue.

Unreadable sources retain their events and resume offset; after three failed attempts at least a
minute apart they also leave the queue, so progress cannot stay stuck forever.

The view carries:

**Reference 1**

Headline figures for the window: equivalent API cost first, then tokens and cache hit rate, with
turns and the estimated share behind them.

**Reference 2**

The window itself, spelled out under the controls: which range is on, and the dates it resolves
to.

**Today**, **7 days** and **30 days** are the named ranges; **Custom** opens a calendar for any
two days, with **This month** and **Last month** as presets.

The calendar is keyboard navigable: arrows walk days and weeks, PageUp and PageDown walk months,
Enter takes a day.

A named range means local calendar days, not UTC ones: "today" starts at midnight where you are,
and "7 days" is the six local days before it plus today, which is the same way the agent CLIs
report their own totals.

The desktop resolves a named range in the browser's zone and sends the two instants outright; a
client that sends only the name gets it resolved in the daemon's zone, which on a local install
is the same one.

**Reference 3**

A timeline of cost or tokens per bucket, stacked by whichever dimension the "Split by" control
names, with a measure toggle above it.

Every bucket in the window is drawn, empty ones included, so the axis is a timeline rather than
a list of the hours that happened to be busy.

Weekends are shaded on day buckets, hovering shows a crosshair and a readout (including "No
usage"), and a legend entry isolates its series.

**Reference 4**

Clicking a bar narrows the window to it: a day opens as hours, an hour as quarter hours.

The breadcrumb above the chart puts the window it came from back.

**Reference 5**

A breakdown table for that same dimension: agent, model, repo, lane or account.

**Reference 6**

Findings: the top cost drivers, models reading at a low cache hit rate, sessions that spent many
turns retrying, and light sessions that a cheaper model would have handled.

A finding names its session by task rather than by identifier, links to that session's row, and
folds repeats of the same shape into one line with a count and a total.

**Reference 7**

A sessions table with the task headline pulled from the transcript, the lane named the way the
sidebar names it, turns, tool calls, the subagent share, retries, duration, tokens and cost.

Headings sort, and a row opens to the token split, the subagent tokens, the lane, and the window
the session ran in.

Clicking a session's lane focuses it.

**Reference 8**

Export to CSV or JSON.

The daemon writes the file under its data directory and names the path.
Numbers are formatted once, everywhere, including the CLI: tokens as k, M or B with one decimal
("12.6B", never "12580.0M"); money as whole dollars above a thousand and cents below a hundred;
durations with an empty unit dropped, so three hours reads "3h".

A session's headline is the first thing the operator actually wrote.

The extractor skips the blocks the CLIs inject into a turn (`<local-command-caveat>`,
`<system-reminder>`, `<USER_REQUEST>`, `<task-notification>`, `<agent-message>`) and lines that
are slash commands, takes the first real sentence, falls back to the first assistant sentence,
and otherwise says "untitled session".

The raw turn is kept as the row's tooltip.

Costs are what the tokens would have cost on the provider's API.

On a subscription plan such as Claude Max or Google AI Pro that is the value the plan returned
rather than an invoice, which is why the figure is labelled "equivalent API cost" and sits
beside the plan's own quota percentages on the sidebar's rate-limits card.

A model with no published rate still contributes its tokens and is named under the breakdown
rather than quietly costing nothing.

A free-tier model id (one ending in `-free`) is priced at zero instead, because zero is its
published rate.

Antigravity keeps no token counts anywhere the CLI can read, so its rows are estimated from
content length at four characters per token and counted in the "estimated" share.

</details>

### Configuring the ledger

Use **Settings > Usage** for tracking and price refresh, or edit the `[usage]` configuration.

<details>
<summary>Details</summary>

The `[usage]` table in `~/.config/repomon/config.toml`:

- `enabled` (default `true`) turns ingest on and off.
- `scan_interval_secs` (default `600`) is the floor between full scans.
- `max_files_per_scan` (default `200`) bounds the work in one pass, newest files first.
- `refresh_prices` (default `true`) fetches LiteLLM's public price list once a day and caches it
  under the data directory. Set it to `false` to keep the ledger fully offline; the built-in rate
  table is always there as a floor either way.
- `[usage.price_overrides."<model>"]` corrects a rate. Every field is optional, so naming just
  `input_per_mtok` leaves the rest of that model's rates alone. Cost is computed at query time, so
  a correction re-prices history.

`repomon usage today|week|month`, `repomon usage report`, `repomon usage ingest` and `repomon
usage status` answer the same questions from a terminal, with `--group-by` and `--csv`.

The sidebar Rate limits card keeps its refresh control and reading age.

To hide or show its Today cost row, use **Settings > Usage > Show today's cost in the sidebar**.

This preference is on by default, applies immediately, and persists with the other sidebar
preferences.

</details>

### Model rates in Settings

Open **Settings > Usage** and filter the **Model rates** table by model ID.

**Time: 1 minute.**

1. Find the model and choose **Edit**.
2. Enter only the rates you need to change, in USD per million tokens.
3. Save; use **Reset** to remove that model’s entire override.

**You know it worked when:** historical costs update without restarting the daemon.

```sh
repomon usage rates set claude-sonnet-5 --output 9
repomon usage rates set future-model --input 2 --output 10 --cache-read 0.2 --cache-write 2.5
repomon usage rates reset claude-sonnet-5
```

<details>
<summary>Details</summary>

Open **Settings > Usage** to turn tracking or daily price refresh on and off, inspect price
provenance, or refresh prices now.

The **Model rates** table includes every model the ledger has seen and every model with an
override.

Unpriced rows come first by default; filter by model id or sort any column, including last seen
and 30-day tokens.

The Usage view's unpriced warning opens this tab with the first unpriced model already in the
filter.

Rates are USD per million tokens.

Choose **Edit**, type only the rates you want to change, and save.

Blank fields keep the currently resolved value, including any existing override; zero explicitly
sets a free rate.

**Reset** removes the entire override for that model.

**Add model** accepts a model id or family prefix before its first run.

Changes take effect immediately, including costs for past usage, without restarting the daemon.

For the same model id, an override wins over the LiteLLM snapshot, then the built-in table.

Resolution checks an exact id first, then known aliases, then the longest matching family
prefix.

For example, `claude-haiku-4-5` can price `claude-haiku-4-5-20251001` when no exact row exists.

A published exact row takes priority over a broader family override; edit the exact id to
correct it.

A partial override inherits the other rates from the resolved row.

The CLI examples above use the same live config patches.

</details>

### Where prices come from

Read the Usage pricing footnote before changing an unfamiliar model’s rate.

<details>
<summary>Details</summary>

A price is resolved in this order: `[usage.price_overrides]` first, then the daily LiteLLM
snapshot, and the rate table repomon ships with as the floor everything else falls back to.

A few model ids a CLI logs (a Codex internal review model, a tiered Gemini label) don't match a
LiteLLM key by name; a small alias table in `pricing.rs` maps those to the closest LiteLLM entry
rather than leaving them on the generic built-in rate.

A model unpriced right after a refresh gets one retry ten minutes later; if it's still unpriced
after that, it stays flagged rather than being retried forever.

The Usage view's pricing footnote and `repomon usage rates` both report where the active rates
came from (LiteLLM, overrides, built-in, and how many of each), when the snapshot was last
fetched, and the last fetch's error if one is in progress; a Refresh button (and `repomon usage
rates --refresh`) forces an immediate fetch.

Turn the daily fetch off entirely with `refresh_prices = false` in `[usage]`.

No other change is needed, and nothing about the ledger itself requires network access.

</details>

## Known gaps

Match the symptom below to its workaround.

<details>
<summary>Details</summary>

- Agent terminal panes can occasionally show visual corruption (garbled or stale rows). The
  v0.7.0 rendering fixes eliminated the main causes, but rare cases remain. Workaround: resize
  the pane or window slightly, which forces a clean refit and redraw.
- Agent terminal screens can occasionally freeze and stop updating even though the agent is
  still running underneath. Workaround: quit and reopen the app; the tmux-backed session is
  durable, so nothing is lost and the pane comes back live.
- On Windows and Linux, `mod` is Ctrl, which is also the terminal's own control modifier. A bound
  Ctrl chord pressed while a terminal is focused currently fires the GUI action **and** reaches the
  agent. macOS is unaffected, since Cmd is not a terminal control key.
- Hiding a project can only be undone from Mission Control. The TUI honors the flag but has no
  reveal list, so it cannot unhide.
- The iOS companion app is built but unreleased.

</details>

## Permissions for agents

On macOS, grant Accessibility and Screen Recording to the app responsible for launching an
agent. A command that works in Terminal can still fail in a Repomon lane because Terminal's
grant is separate. In a measured Repomon lane, macOS attributed both requests to Repomon.app,
even though the lane's tmux server had detached under launchd.

1. Open **System Settings > Privacy & Security > Accessibility** and enable the installed
   **Repomon.app**. Use the add button if it is missing.
2. Open **Screen & System Audio Recording** and enable Repomon for screen recording. Older
   macOS versions call this **Screen Recording**. If macOS asks for **Automation > System
   Events**, allow that separately for UI scripting.
3. Quit and reopen Repomon if macOS requests it, then test from a Repomon lane:

   ```sh
   osascript -e 'tell application "System Events"' -e 'set frontApp to first application process whose frontmost is true' -e 'tell frontApp to get count of windows' -e 'end tell'
   screencapture -x /tmp/repomon-permission-probe.png
   rm -f /tmp/repomon-permission-probe.png
   ```

   The first command returns a window count when Accessibility access works. The second
   creates a screen image; the third removes it. Merely asking System Events for a process
   name does not prove Accessibility access.

A locally rebuilt or replaced app can retain an enabled settings entry whose stored signing
requirement belongs to an older executable. If permission still fails, remove the stale
Repomon entry from each affected permission list and add the current app again. Detached
agent sessions can also retain the previous app's responsibility after its window closes.
Save work and finish active commands before restarting the fleet or logging out and back in;
reopening the window alone does not restart an existing tmux server.

| Launch context | Grant to check |
|---|---|
| Repomon app launched the lane's process tree | The current Repomon.app |
| Terminal started a fresh tmux server | Terminal |
| An independently installed daemon or another launcher started the fleet | The responsible app or executable reported for that process tree; do not assume Terminal's grant applies |

The observed failure was a stale ad hoc signing requirement, not evidence that tmux always
loses app attribution. Stable signed releases avoid tying the app's identity to one local
build. See Apple's [Accessibility](https://support.apple.com/en-gb/guide/mac-help/mh43185/mac)
and [Screen Recording](https://support.apple.com/en-mide/guide/mac-help/mchld6aa7d23/mac)
permission instructions and its [responsible-code and signing explanation](https://developer.apple.com/forums/thread/678819).
