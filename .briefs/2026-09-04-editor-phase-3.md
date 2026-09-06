# Brief F: Editor phase 3, IDE glue

Spec: `docs/superpowers/specs/2026-09-03-editor-workspace-design.md`, section "Phase 3: IDE glue"
is the source of truth. Phases 1 and 2 are merged (main at dd32ea8 or later). Start from current
main.

Branch: `feat/editor-phase-3` in a fresh worktree at `/private/tmp/repomon-feat-editor-phase-3`
(`git -C /Users/azaleas/Developer/Claude/repomon worktree add -b feat/editor-phase-3 /private/tmp/repomon-feat-editor-phase-3 main`).
Use `/teamwork-preview`: F1 and F2 are independent of each other; F3 depends on F2; F4 and F5 are
independent of everything. One integrator owns the branch and the final gate. Commit each part as
soon as it is green so a quota loss costs nothing; if you lose quota, reply to the coordinator with
where you stopped.

## Rules carried over (hard)

- Zero hex color literals in frontend code; colors are `var(--signal|--attention|--muted|--foreground|--fault|--surface|--raised|--line|--background)` or `color-mix()` of those.
- No `<Show keyed>` around the editor or anything that re-renders per keystroke.
- Every async result that mutates UI is token, path, and lane guarded (the store already has
  `openAt` tokens, `savingPaths`, and lane-routed events; reuse them).
- Shell cwd is the main checkout: absolute worktree paths or `cd <worktree> &&` for every command.
- Never kill processes by name pattern; stop only PIDs you started or full sandbox paths. Never
  touch `/Applications/Repomon.app` or `/tmp/repomon-azaleas.sock`. Live QA only via the
  isolation recipe in `apps/desktop/e2e/isolated.sh`; screenshots are best-effort (skip after two
  permission failures and say so).
- No emoji glyphs (SVG icons in `icons.tsx`), no em-dashes in any copy or comment.
- Run `/frontend-design` and `/impeccable` for every UI part.

## Parts

**F1. Clickable paths in terminal output.** In `apps/desktop/src/components/TerminalPane.tsx`
register an xterm link provider (`terminal.registerLinkProvider`) that recognises, per rendered
line, file references in these shapes: `src/foo.rs:12:4`, `src/foo.rs:12`, `apps/x/y.tsx`,
`./relative/path.ext`, absolute paths under the lane's worktree root, and the Rust/TS diagnostic
forms `--> src/main.rs:10:5` and `at src/app.ts:5:3`. Resolve against the pane's lane worktree
(the pane knows its `laneId`; get the root from the fleet store) and only link paths that exist
in the lane's `file.index` cache (ask the editor store; do not hit the daemon per hover). Hover
underlines with the app's link styling; Cmd-click (Ctrl-click on Windows and Linux) opens the
file in the editor at the line and column via the editor store's `openAt`, opening the center
Editor mode if neither surface is open. Plain click keeps today's behavior. Pure, unit-tested
matcher in `apps/desktop/src/components/terminalPathLinks.ts` (exported `findPathRefs(line)`)
with cases for each shape, trailing punctuation, and Windows drive paths ignored on macOS.

**F2. `file.diff_base` RPC (daemon).** `file.diff_base { lane_id, path }` ->
`FileDiffBaseResult { content: String | null, kind: "text" | "binary" | "missing" }` returning the
HEAD version of the path via `git show HEAD:<path>` in the lane worktree (`missing` for untracked
or newly added files, `binary` by the same null-byte sniff as `file.read`, same 2 MiB cap).
Local-only (exclusion list in `remote.rs` plus its test), `worktree_path_allowed` on the path,
documented in `docs/protocol.md`, unit and integration tests in `files.rs` and
`tests/file_rpcs.rs`. ts-rs type in `model.rs`, bindings regenerated, RpcMap entry.

**F3. Git gutter (frontend).** In `CodeEditor.tsx` add a gutter marking added, modified, and
removed line ranges relative to the HEAD version from F2, computed with a small line-diff
(Myers or a simple LCS on lines; a dependency-free implementation in
`apps/desktop/src/components/lineDiff.ts`, unit-tested). Markers use `var(--signal)` for added,
`var(--attention)` for modified, `var(--fault)` for removed (a thin triangle at the removed
position). Recompute on a 300 ms debounce after edits and after save, and refresh the base when
an `event.file.changed` for the lane arrives for `.git/HEAD` or the file itself (the watcher
ignores `.git/`, so refresh the base on save and on tab activation instead). Clicking a marker
shows a small popover with the original lines and a "Revert hunk" action that applies through a
CodeMirror transaction. Token and path guarded like the language loader.

**F4. Open in editor from the Git panel and search results.** In
`apps/desktop/src/components/GitExplorerPanel.tsx`'s file list and diff view add an "Open in
editor" action (row hover button plus the row's context menu if one exists) that opens the file
in the center Editor mode at the first changed hunk's line; in the diff view, clicking a line
number opens at that line. Reuse the store's `openAt`. Keyboard: Enter on a focused row opens.

**F5. Markdown preview.** For `.md` and `.markdown` tabs add a preview toggle in the status
line (and `mod+shift+v`, registered in `keymap.ts`, documented in `docs/desktop.md`) that
renders the buffer beside the editor in a split (resizable, remembers the ratio in the editor
store). Use a dependency-free renderer under `apps/desktop/src/components/markdown/` covering
headings, paragraphs, emphasis, inline code, fenced code (highlighted with the same CodeMirror
language loader as a read-only mini editor, or plain `<pre>` when the language is unknown),
lists, task lists, links (open externally via the opener plugin), images (resolved through
`file.read_raw` when the path is inside the worktree), tables, blockquotes, and horizontal
rules. Sanitize: no raw HTML passthrough. Scroll sync editor to preview by nearest heading.
Unit tests for the renderer on a fixture document.

**F6. Larger files.** Raise `READ_CAP_BYTES` in `crates/repomon-daemon/src/files.rs` to 8 MiB
and add `large: bool` (over 2 MiB) to `FileReadResult`; the frontend opens large files read-only
with autocomplete, folding, and the git gutter disabled and a status-line note "Large file:
read-only". Update the daemon unit test for the cap and `docs/protocol.md`.

## Gate

`bun run check`, `bun run test`, `bun run bindings:check` in `apps/desktop`;
`cargo test -p repomon-daemon` and `cargo test -p repomon-core` at the worktree root. Grep the
diff for em-dashes and emoji (both empty) and touched frontend files for hex colors (0). Best-effort
live QA in an isolated app: Cmd-click a path printed by an agent, see gutter markers after an edit,
open a file from the Git panel, toggle a markdown preview. Commits: one per part, 1-line
Conventional Commits, no AI co-author trailer, never commit `qa/` or fixtures. Report to the
coordinator address (lane-81/7) with commit hashes, gate tails, and screenshot paths or the
reason they were skipped. Do not merge.
