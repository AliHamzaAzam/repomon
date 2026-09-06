# Brief D: Editor phase 2, navigation and file operations

Read the spec first: `docs/superpowers/specs/2026-09-03-editor-workspace-design.md`, section
"Phase 2: navigation and file operations" is the source of truth. Phase 1 is merged to main
(commits a35c6f3..82ece7e). Start from current main.

Branch: `feat/editor-phase-2` in a fresh worktree at `/private/tmp/repomon-feat-editor-phase-2`
(`git -C /Users/azaleas/Developer/Claude/repomon worktree add -b feat/editor-phase-2 /private/tmp/repomon-feat-editor-phase-2 main`).
Use `/teamwork-preview`: D1 and D2 (daemon) are independent of each other; D3 depends on D1 and
D2; D4 and D5 (frontend) depend on their daemon RPCs; D6 depends on D3. One integrator owns the
branch and the final gate. Commit work in progress on the branch whenever a part reaches a green
state so nothing is lost if you lose quota; if you do lose quota, reply to the coordinator with
where you stopped.

## Lessons from the phase 1 review (apply everywhere)

- Zero hex color literals in frontend code; every color is `var(--signal|--attention|--muted|--foreground|--fault|--surface|--raised|--line|--background)` or `color-mix()` of those.
- Never `<Show when={activeFile()} keyed>` around anything expensive; store updates produce new
  object references on every edit.
- Every async result that reconfigures UI carries a request token or path check so a stale
  result cannot win.
- Shell cwd is the main checkout: use absolute paths under the worktree or `cd <worktree> &&`.
- Live QA only with the isolation recipe in `apps/desktop/e2e/isolated.sh` (unique
  `tmux_session`, throwaway config and data dirs, fresh DB, own socket). Never a copy of the
  production DB, never `/tmp/repomon-azaleas.sock`.
- No emoji glyphs (icons are SVG in `icons.tsx`), no em-dashes in any copy or comment.

## Daemon parts (Rust, `crates/repomon-daemon/src/files.rs` + `rpc.rs` + `remote.rs` + `docs/protocol.md`)

**D1. `file.index { lane_id }` -> `FileIndexResult { paths: Vec<String>, truncated: bool, generation: u64 }`.**
Recursive, files only, relative paths with `/` separators, gitignore-aware (reuse the batched
`git check-ignore` approach from `list_dir` or walk with `ignore`-style rules; never descend into
`.git`, and skip ignored directories entirely so `node_modules` and `target` cost nothing), capped
at 50 000 entries with `truncated`. Cache per lane in `Ctx` (a `HashMap<lane_id, CachedIndex>`
behind a `Mutex`) with the `generation` bumped on every watcher event (D2) and on our own
`file.write`/`file.create`/`file.rename`/`file.delete`; a call whose cache is fresh returns it
without walking. Local-only (add to the exclusion list in `remote.rs` next to `file.read_raw`).
Tests: nested dirs, ignored dir skipped, `.git` never listed, cap and `truncated`, cache hit vs
invalidation.

**D2. Worktree watcher + `event.file.changed` broadcasts.** For every lane that is currently in
some connection's viewport (`viewport.set` already tracks this; hook where the capture loop
decides what to stream), run a `notify-debouncer-full` watcher (already a workspace dependency,
see `Cargo.toml` and how `repomon-core` uses `notify`) over the lane's worktree root, debounced
250 ms, ignoring `.git/` and gitignored paths. Broadcast
`event.file.changed { lane_id, path, op: "modified" | "created" | "removed" | "renamed", from?: String }`
(keep the existing `event.file.changed { lane_id, path }` shape backward compatible: `op` is
additive; our own `file.write` now sends `op: "modified"`). Stop the watcher when the lane leaves
every viewport or is deleted. Tests: create/modify/remove produce the right ops; ignored path
produces nothing; watcher stops on lane removal (use a tempdir repo and a short debounce in tests).

**D3. File operations, all local-only, all broadcasting `event.file.changed` with the right `op`:**
- `file.create { lane_id, path, is_dir: bool }` -> `{ path }`; refuses if the path exists.
- `file.rename { lane_id, from, to }` -> `{ from, to }`; refuses if `to` exists; both paths must
  pass `worktree_path_allowed`.
- `file.delete { lane_id, path, recursive?: bool }` -> `{ path }`; files and empty directories
  by default, non-empty directories only with `recursive: true`; never follows symlinks out of
  the worktree; refuses `.git`.
Reuse `worktree_path_allowed` for every path. Add the three to `remote.rs`'s exclusion list and
its test, document in `docs/protocol.md`, unit-test each (happy path, escape attempt, exists
conflict, recursive guard).

**D4. `file.search { lane_id, query, regex?: bool, case_sensitive?: bool, glob?: String, max_results?: u32 }` -> `FileSearchResult { hits: Vec<FileSearchHit>, truncated: bool }`**
with `FileSearchHit { path, line: u32, column: u32, preview: String }` (preview is the full line
trimmed to 240 chars around the match). Walk the same ignore-aware file set as D1 (reuse its
cache), skip binaries (null-byte sniff) and files over the read cap, cap results at 2000 (or
`max_results`), use the `regex` crate for regex mode and a plain substring scan otherwise
(case-insensitive by default). Run in `spawn_blocking` and stop early at the cap. Local-only.
Tests: substring, regex, case sensitivity, glob filter (`*.rs`), binary skipped, cap and
`truncated`.

All new result types go in `crates/repomon-core/src/model.rs` with the `ts` derive so
`bun run bindings:generate` produces `apps/desktop/src/bindings/*.ts`; add each to
`bindings/index.ts` and to `RpcMap` in `apps/desktop/src/ipc/rpc.ts`.

## Frontend parts (Solid, `apps/desktop/src`)

**D5. Fuzzy file finder (Cmd-P).** New `components/FileFinder.tsx` opened from the editor
workspace and the rail panel (and from a `finder.open` keymap entry, `mod+p`, registered in
`keymap.ts` and documented in `docs/desktop.md` like the other chords). Own scorer, no
dependency: prefer basename matches, then path-segment prefixes, then subsequence; stable
ordering; show the top 50 with the matched characters emphasized; arrow keys plus Enter open in
the current surface (center mode if open, otherwise the rail), Esc closes, typing again re-filters
without flicker. Data comes from `file.index`; refresh when an `event.file.changed` arrives for
the lane. Tests for the scorer (pure function) and for keyboard navigation.

**D6. Project search (Cmd-Shift-F) in the tree column.** A search mode of the tree column in
`EditorWorkspace.tsx` (and a compact version in the rail panel): query input, regex and
case-sensitive toggles, glob filter, results grouped by file with line previews, click opens the
file at line and column (add an `openAt(path, line, column)` to the editor store that opens the
tab, then sets the selection and scrolls it into view once the doc is loaded, using the request
token pattern). Replace-in-file for the current buffer only: a replace field that applies to the
open document through a CodeMirror transaction (no daemon-side bulk replace in this phase).
Keymap `search.project`, `mod+shift+f`. Tests for grouping and for `openAt` selecting the right
range.

**D7. Tree context menu and file operations.** Right click and a hover kebab on tree rows:
New file, New folder, Rename, Delete (with the existing `ConfirmDialog`), Reveal in Finder (the
opener plugin, `@tauri-apps/plugin-opener`, already a dependency), Copy relative path. Inline
rename and create (an input in place of the row, Enter commits, Esc cancels). Update open tabs
when a file is renamed or deleted (rename retargets the tab, delete marks it deleted-on-disk with
the existing conflict banner). Tests for tab retargeting on rename and delete.

**D8. Live refresh from `event.file.changed`.** Subscribe once in the editor store: reload the
affected tree levels (parent dir of `path`, and both parents on rename), invalidate the finder
index, and for open tabs: a clean buffer reloads silently from disk (preserving cursor and
scroll where the line still exists), a dirty buffer gets the existing conflict banner, a removed
file marks the tab deleted-on-disk. Debounce bursts (an agent writing many files) so the tree
reloads at most every 300 ms. Tests with a mocked `subscribeDaemon` stream.

## Design rules

Run `/frontend-design` and `/impeccable` for D5, D6, D7. Match the existing surfaces (the
Multitasking picker, the Repomail panel's inputs, the Git panel's tree). The finder and search
panels use the app's inputs and buttons, not browser defaults.

## Gate

`bun run check`, `bun run test`, `bun run bindings:check` in `apps/desktop`;
`cargo test -p repomon-daemon` and `cargo test -p repomon-core` at the worktree root. Grep the
diff for em-dashes and emoji (both must be empty) and `CodeEditor.tsx`, `FileFinder.tsx`,
`EditorWorkspace.tsx` for hex colors (must be 0). Live QA in an isolated app: open the finder,
search the worktree for a symbol and jump to it, create, rename, and delete a file from the tree,
then touch a file from a shell in the isolated worktree and watch the tree and an open clean tab
refresh; screenshots under the worktree's `qa/` directory (untracked). Commits: one per part,
1-line Conventional Commits (`feat(daemon): ...`, `feat(desktop): ...`), no AI co-author trailer.
Never commit `qa/` or fixtures. Report to the coordinator address with commit hashes, gate
tails, and screenshot paths. Do not merge.
