# Brief E: Editor phase 2 review fixes (before merge)

Branch `feat/editor-phase-2`, worktree `/private/tmp/repomon-feat-editor-phase-2`. All gates are
green (497 vitest, 362 core, 332 daemon, bindings, tsc) and the daemon parts D1 to D4 are approved.
The frontend review found the items below. Fix all of them on the same branch in a few logical
`fix(desktop): ...` commits, re-run the full gate, and report. Add a test for every item marked
(test) using the existing patterns in `stores/editor.test.ts`, `CodeEditor.test.tsx`,
`FileFinder.test.tsx`, `EditorWorkspace.test.tsx`.

## Critical

1. **`openAt` token race** (`apps/desktop/src/stores/editor.ts` around line 638). The second
   `setOpenAtTarget` after `await openFile(path)` always publishes with a fresh higher token, so a
   slow earlier call finishing last overrides a newer click. Capture the token at call start and
   publish only if `token === openAtToken` still holds after the await; otherwise return without
   publishing. (test: two overlapping `openAt` calls on one file where the first load resolves
   last; the editor must end on the second target.)

2. **Replace All hangs on zero-width regex matches** (`apps/desktop/src/components/CodeEditor.tsx`
   around lines 851 to 866). The `while ((m = pattern.exec(docText)))` loop never advances
   `lastIndex` on an empty match (`a*`, `\s*`, `(?:)`), freezing the UI. After a zero-length
   match, set `pattern.lastIndex = m.index + 1` and skip it (do not insert replacements at
   empty matches); also cap the loop at the document length. (test: regex `a*` with replacement
   on a small doc terminates and replaces only non-empty matches.)

## Important

3. **Project search responses not guarded** (`apps/desktop/src/components/ProjectSearchPanel.tsx`
   `triggerSearch`). `file.search` runs in `spawn_blocking`; a slow earlier query can resolve
   after a fast later one. Use a request counter (and lane id) and drop responses that are not
   the latest. (test: two searches resolving out of order; the newer query's hits win.)

4. **Finder index not lane-guarded** (`apps/desktop/src/components/FileFinder.tsx` `fetchIndex`).
   Switching lanes while the finder is open lets the old lane's index overwrite the new one.
   Capture the lane id per request and ignore responses for a different lane or a superseded
   request. (test)

5. **Context menu has no Escape dismissal** (`apps/desktop/src/components/EditorWorkspace.tsx`
   around lines 1036 to 1119). Add a window keydown listener while the menu is open that closes
   it on Escape, removed on close and in `onCleanup`. Also return focus to the tree row that
   opened it. (test)

6. **`mod+shift+f` collides with the terminal's find bar.** `TerminalPane.tsx` (around line 395)
   handles the chord with `preventDefault` but not `stopPropagation`, and `App.tsx`'s global
   `onShortcut` still fires. Fix at the global level so any component-level handler wins: in
   `App.tsx`'s keydown handler, return early when `event.defaultPrevented`. Keep the chord for
   project search (it matches editor conventions) and document in `docs/desktop.md` that inside
   a focused terminal the chord searches the terminal. (test: a synthetic keydown with
   `defaultPrevented` set does not trigger the panel.)

## Moderate

7. **Inline create and rename can double-submit** (`EditorWorkspace.tsx` `commitInlineCreate`
   and `commitInlineRename`, wired to both Enter and blur). Add an in-flight flag per operation:
   the second trigger returns immediately while the first RPC is pending, and the input is
   disabled during the call. (test: Enter followed by blur issues exactly one RPC.)

## Critical (live refresh, D8, `apps/desktop/src/stores/editor.ts`)

8. **Rename and delete events from a non-active lane mutate the active lane's tabs** (around
   lines 740 to 750). `handleFileRenamed`/`handleFileDeleted` take no lane id and edit the live
   `openFiles`/`activePath`/`expandedDirs` unconditionally; only the `modified` branch routes by
   lane. Multitasking keeps several lanes in the viewport, so this happens routinely. Route every
   op by lane: for the active lane mutate the live signals, for a background lane mutate that
   lane's entry in `laneStates`, and ignore lanes the store does not track. (test: a `removed`
   and a `renamed` event for lane 2 while lane 1 is active leave lane 1's tabs untouched and
   update lane 2's stored state.)

9. **Self-echo of our own save can flag a false conflict.** The daemon broadcasts
   `event.file.changed` before `file.write` returns, `saveFile` records no in-flight marker, and
   `syncExternalChange` snapshots the file before its own `file.read` await, so a stale
   `savedContent` makes a just-saved buffer look dirty and the later resolution can leave
   `conflict` set. Fix: keep a `savingPaths` set (or per-tab `saving` flag already present) and
   have the event handler skip `modified` events for a path whose save is in flight, and after
   the save resolves compare the event's mtime with the new `mtimeMs` before doing anything;
   also re-read the file record after the await in `syncExternalChange` instead of using the
   pre-await snapshot. (test: save in flight, echo event arrives, save resolves: no conflict,
   cursor preserved.)

## Important (D8)

10. **Debounced tree reload is not lane-scoped** (`queueDirReload`, around lines 703 to 716).
    `pendingReloadDirs` holds bare directory strings and the flush uses the last caller's lane
    id; `loadDir` also stamps `dirCache` with "loading" before its lane guard, so a mismatched
    flush can freeze a directory row on the active tree in the loading skeleton. Key the pending
    set by `${laneId}:${dir}` (or a `Map<laneId, Set<dir>>`), flush each with its own lane, and
    move `loadDir`'s `activeLaneId` guard before the "loading" write (background lanes update
    their `laneStates` entry instead). (test: interleaved events for two lanes in one 300 ms
    window reload each lane's directories with the right lane id, and the active tree never
    shows a stuck loading row.)

## Gate

`bun run check`, `bun run test`, `bun run bindings:check` in `apps/desktop`; `cargo test -p repomon-daemon`
at the worktree root. Grep the diff for em-dashes and emoji (both must be empty) and the touched
files for hex colors (must be 0). Never kill processes by name pattern; never touch
`/Applications/Repomon.app` or `/tmp/repomon-azaleas.sock`. Report with commit hashes and gate
tails. Do not merge.
