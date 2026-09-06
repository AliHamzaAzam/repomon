# Brief A: Multitasking panes all render tall regardless of footprint

Priority: do this first, before the editor work. Small, isolated, user is looking at it now.
Branch: `fix/multitask-row-ratchet` in a fresh worktree at `/private/tmp/repomon-fix-multitask-row-ratchet`.

## Symptom

Fleet Workspace with 8 panes selected, every footprint at 1x, "Tall" not pressed on any pane.
Expected: three columns, rows sized so several rows fit the bay. Actual: every pane spans the full
bay height, so only three panes are visible and the rest are below the fold.

## Root cause (verified by reading the code, reproduce it before you change anything)

The multitasking row height is an output of the terminal's rendered size, and the rendered size is
an output of the row height. That loop only ever grows.

1. `apps/desktop/src/index.css`: `.terminal-layout.is-multitasking { grid-auto-rows:
   minmax(var(--multitask-row-min-height, 14rem), 1fr) }` and `.multitask-pane { min-height:
   var(--multitask-row-min-height) }`.
2. `TerminalWorkspace.tsx` `multitaskRowMinimum` = max over all visible panes of the value each
   pane reported through `onMinimumHeight`.
3. `TerminalPane.tsx` `reportMinimumHeight()` reports `chrome + .xterm-screen height`. The screen
   height is `terminal.rows * cellHeight`, and `terminal.rows` was just set by `applyGrid()` from
   the daemon's arbitrated `agent.fit` answer.
4. If any single pane ever renders with a large row count, even transiently, its reported height
   raises the shared row minimum for every row, which enlarges every pane, which makes every pane
   propose more rows, which the daemon accepts, which raises the reported height again. Nothing
   in the loop can shrink because a pane's minimum is always at least its current rendered height.

The transient trigger: entering Multitasking from a focused single pane that had ~60 rows. The
first `ResizeObserver` pass runs before `viewport.set` installs this viewport's fit claims (see the
comment above the `syncViewport().finally(notifyLayoutChanged)` effect), so `agent.fit` still
returns the old 60-row grid for that pane. It renders 60 rows tall, reports ~1000px, and the
whole grid is now 1000px per row forever. Any pane whose fit is won by another viewer (TUI, a
second desktop) triggers the same ratchet permanently.

## Fix

Make the row height an input derived from the footprint and the terminal's cell metrics, not from
whatever the terminal happens to be rendering.

1. In `TerminalPane.tsx`, replace `reportMinimumHeight()`'s screen measurement with a computed
   floor: `chrome + MIN_ROWS * cellHeight`, where `cellHeight` comes from the renderer's cell
   dimensions (`terminal._core._renderService.dimensions.css.cell.height`, or
   `screenRect.height / terminal.rows` as a fallback when that private path is unavailable) and
   `MIN_ROWS` mirrors the daemon's `MIN_PANE_ROWS` (24, `crates/repomon-core/src/agent/tmux.rs`).
   Export the constant from a shared frontend module rather than duplicating the literal in two
   files. This value is stable across renders and independent of the current row count.
2. Keep the chrome measurement (header, composer inset) exactly as it is; that part was correct.
3. `grid-auto-rows` stays `minmax(var(--multitask-row-min-height), 1fr)`. With the floor now
   fixed at 24 rows plus chrome, 1x panes size by `1fr` (rows share the bay) and a "Tall" pane
   (`grid-row: span 2`) is genuinely double. Verify that 8 panes at 1x on a 1300px-tall bay give
   three rows of roughly equal height and that reducing the window height compresses rows down to
   the 24-row floor and no further.
4. When the daemon's fit answer is larger than what fits the cell (another viewer won), do not let
   that leak into the layout: the pane clips (it already does via `overflow-hidden`), and
   `followTailIfNeeded()` keeps the composer in view. Add a comment on `applyGrid` stating this
   explicitly.
5. Prune `paneMinimumHeights` entries for windows no longer in `visibleTargets()` so a pane that
   left the view cannot keep the max elevated.

## Tests

- `TerminalWorkspace.test.tsx`: a pane reporting a large minimum then a small one must lower the
  row minimum (today it cannot). A pane leaving the view drops out of the max.
- `TerminalPane.test.tsx` (or a pure helper test): the floor is `chrome + 24 * cellHeight` and does
  not change when `terminal.rows` changes.
- Live: build the desktop preview (`bun run tauri build --config tauri.preview.conf.json` per
  `repomon-local-desktop-builds`, never plain `tauri:build`), open Multitasking with 8 panes at 1x
  from a focused 60-row pane, screenshot before and after. Attach both screenshots to your report
  under `/private/tmp/repomon-fix-multitask-row-ratchet/qa/`.

## Gate

`bun run check`, `bun run test` in `apps/desktop`. One-line Conventional Commit, e.g.
`fix(desktop): derive multitasking row floor from cell metrics, not rendered rows`. No AI
co-author trailer. Report to the coordinator address in the mail you received, with the branch
name, the commit hash, and the screenshot paths. Do not merge; the coordinator reviews and merges.
