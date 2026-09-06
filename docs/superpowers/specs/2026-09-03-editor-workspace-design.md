# Editor workspace: from proof of concept to daily driver

Status: approved by the operator on 2026-09-03. Untracked by convention (specs never commit).
Owner: coordinating Claude session (PM). Implementer: Antigravity agent on repomon lane-1.

## Goal

People should not need VS Code or Cursor open beside Repomon for day-to-day editing: reading
agent output, opening the file it names, fixing a value, searching the worktree, creating a
file, and watching the agent's own edits land. LSP-backed diagnostics and go-to-definition are
explicitly out of scope until after phase 3.

## Today

- `apps/desktop/src/components/CodeEditor.tsx`: CodeMirror 6 wrapper, 10 languages by extension,
  everything else plain text. Theme reuses the app's four accent roles via CSS variables.
- `apps/desktop/src/components/FileEditorPanel.tsx`: right-rail tab (`mod+7`), capped at 640px by
  `RIGHT_PANEL_MAX_WIDTH_PX`. Lazy one-level file tree that collapses to a breadcrumb when a file
  is open, multi-tab with dirty tracking, conflict-safe save (mtime check, reload/keep-mine banner).
- Daemon (`crates/repomon-daemon/src/files.rs`, RPC arms in `rpc.rs`): `file.list` (one level,
  gitignore-aware, 2000 entries), `file.read` (2 MiB cap, binary sniff), `file.write` (expected
  mtime). All local-socket only (see `remote.rs`). `event.file.changed` fires only on our own write.

## Design

### Phase 1: real editor, real space

**Editor workspace mode.** A center-pane mode toggled from the toolbar (and `mod+7` when the rail
is not the target), mutually exclusive with Multitasking the same way `openPanelTab` already
turns Multitasking off. Layout: a resizable tree column (default 240px, min 180, persisted) on the
left, a tab strip plus editor filling the rest. The rail `FileEditorPanel` stays for quick edits
beside a terminal; both surfaces share one editor store so tabs, dirty buffers, cursor and scroll
position survive switching between rail and center mode.

**Editor store** (`apps/desktop/src/stores/editor.ts`): open files per lane (path, content,
savedContent, mtime, cursor, scroll, conflict state), active tab per lane, tree expansion per
lane, tree column width, view toggles (wrap, whitespace). Persist open paths, active path, cursor
and expansion to localStorage under a versioned key; never persist buffer content. Lane switch
keeps the other lane's tabs in memory and restores them when the lane is reselected (the existing
"discard and switch" dirty prompt goes away since nothing is discarded).

**Every language.** Replace the fixed `EditorLanguage` switch with `@codemirror/language-data`'s
`languages` list and `LanguageDescription.matchFilename`, loaded lazily (`desc.load()` returns a
promise; reconfigure the language compartment when it resolves). Keep the current explicit
packages as the fast path for the 10 languages already bundled so their highlighting is
synchronous. Filenames without extensions map by name (Dockerfile, Makefile, Justfile, .gitignore,
.env*, Cargo.lock as TOML, shebang sniff for `sh`/`python`/`node` on the first line).

**Editing features** (CodeMirror extensions, all themed through the existing `appTheme` and
`highlightStyle` approach, no hard-coded colors):
- `@codemirror/autocomplete`: `autocompletion()` with language completions where the grammar
  provides them plus word-list completion from the open document; `closeBrackets()`.
- `@codemirror/language`: `foldGutter()`, `foldKeymap`, `indentUnit` from a per-file guess
  (2 vs 4 spaces vs tab, sniffed from the first 200 lines).
- `@codemirror/commands`: `toggleComment` on `Mod-/`, `indentLess`/`indentMore`, move line
  up/down on `Alt-ArrowUp/Down`, duplicate line on `Shift-Alt-ArrowDown`.
- `@codemirror/view`: `rectangularSelection()` and `crosshairCursor()` for column selection,
  `EditorState.allowMultipleSelections` with `Mod-d` select-next-occurrence, `highlightWhitespace`
  behind a toggle, `EditorView.lineWrapping` behind a toggle, indent guides (a small custom
  ViewPlugin drawing `border-left` markers, no third-party package).
- `@codemirror/search`: keep the stock panel but restyle it in `appTheme` to match the app (inputs,
  buttons, checkboxes using existing tokens), plus `gotoLine` on `Mod-g`.
- Status line under the editor: language name (click to override), line:col, selection count,
  indent unit, wrap and whitespace toggles, encoding note when the file is not UTF-8.

**Non-text files.** `file.read` already refuses binaries; extend the result with `kind:
"text" | "binary" | "image"` (image by extension: png, jpg, jpeg, gif, webp, svg, bmp, ico). Image
opens in a checkerboard preview with natural size and zoom-to-fit; binary shows size and a
"not a text file" notice. Add `file.read_raw` returning base64 for images under the same 2 MiB cap.

**Tree improvements.** Stays lazy per level. Adds: reveal-active-file (auto-expand and scroll to
the active tab's path), keyboard navigation (arrows, Enter, Left/Right to collapse/expand), icons
by file kind (SVG from `icons.tsx`, no emoji), a filter box that narrows already-loaded levels.

**Acceptance for phase 1:**
1. Open the editor mode on the repomon lane, open `Cargo.toml`, `docs/desktop.md`, a `.yaml`
   under `.github/workflows`, a shell script, and `Dockerfile`-style file: all highlighted.
2. Fold a Rust fn, toggle a comment with `Mod-/`, `Mod-d` twice to multi-select, `Mod-g` to line
   200, `Mod-f` find and replace one occurrence, save with `Mod-s`, reopen: cursor restored.
3. Open a PNG: preview renders. Open a 3 MiB file: clear too-large message, no crash.
4. Switch lanes and back: tabs, dirty state, cursor all restored; rail and center mode show the
   same tabs.
5. `bun run check`, `bun run test`, `bun run bindings:check` green; `cargo test -p repomon-daemon`
   green for the `file.read` kind change.

### Phase 2: navigation and file operations

Daemon (all local-only, added to `remote.rs`'s exclusion list, each with a `files.rs` unit test):
- `file.index { lane_id }`: recursive gitignore-aware path list (files only), capped at 50k
  entries with `truncated`, cached per lane and invalidated by the watcher below.
- `file.search { lane_id, query, regex?, case_sensitive?, glob?, max_results? }`: content search
  over the worktree using the `grep` crate or a `walkdir` + line scan, skipping binaries and
  ignored paths, returning `{ path, line, column, preview }` capped at 2000 hits.
- `file.create { lane_id, path, is_dir }`, `file.rename { lane_id, from, to }`,
  `file.delete { lane_id, path }` (files and empty dirs only; non-empty dirs require
  `recursive: true`). All broadcast `event.file.changed` with an `op` field.
- Worktree watcher: for every lane in the current viewport, a `notify-debouncer-full` watcher
  (already a workspace dependency) that broadcasts `event.file.changed { lane_id, path, op:
  "modified" | "created" | "removed" | "renamed" }`, debounced 250ms, ignoring `.git/` and
  gitignored paths.

Frontend:
- `Mod-p` fuzzy file finder (own scorer, no dependency; prefer basename matches, then path
  segments), opens in the current surface.
- `Mod-shift-f` project search panel in the tree column: query, regex and case toggles, glob
  filter, grouped results, click to open at line and column, replace-in-file for the current
  buffer only (no bulk replace in this phase).
- Tree context menu (right click and a kebab on hover): new file, new folder, rename, delete
  (confirm), reveal in Finder/Explorer via the opener plugin, copy relative path.
- Live refresh: watcher events reload affected tree levels and clean buffers silently; dirty
  buffers get the existing conflict banner. Tabs whose file was removed show a "deleted on disk"
  state.

### Phase 3: IDE glue

- Terminal link provider in `TerminalPane`: paths like `src/foo.rs:12:4`, `apps/x/y.tsx`, and
  absolute paths under the lane worktree become clickable and open in the editor at the line.
- Git gutter: `file.diff_base { lane_id, path }` returns the HEAD version; a CodeMirror gutter
  marks added, modified and removed ranges using the app's signal/attention/fault tokens.
- "Open in editor" from `GitExplorerPanel`'s diff view and from search results.
- Markdown preview toggle for `.md` (render with a small dependency-free renderer or `marked`
  if size is acceptable; sanitize output).
- Raise `READ_CAP_BYTES` to 8 MiB with a read-only mode above 2 MiB (no autocomplete or folding).

## Constraints

- Design skills are mandatory for every UI task: `/frontend-design` and `/impeccable`. No emoji
  glyphs anywhere in product surfaces (draw icons as SVG). No em-dashes in any copy.
- Follow existing patterns: Solid signals and stores, `daemonCall` RpcMap entries, ts-rs
  bindings regenerated with `bun run bindings:generate` and checked with `bun run bindings:check`,
  RPC docs in `docs/protocol.md`, user docs in `docs/desktop.md`.
- Commits: 1-line Conventional Commits matching the repo (`feat(desktop): ...`,
  `feat(daemon): ...`), no AI co-author trailer.
- Work happens in a separate worktree under `/private/tmp/repomon-<branch>`, never in the main
  checkout at `/Users/azaleas/Developer/Claude/repomon`.
- Never run a debug `repomond` against the production socket `/tmp/repomon-azaleas.sock`; use a
  test socket path for live verification.
