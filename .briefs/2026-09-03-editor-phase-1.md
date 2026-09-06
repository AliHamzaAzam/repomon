# Brief B: Editor phase 1, real editor with real space

Start after Brief A is reported. Read the full design first:
`docs/superpowers/specs/2026-09-03-editor-workspace-design.md` (sections Today, Phase 1,
Constraints). This brief is the phase 1 task list; the spec is the source of truth when they
disagree.

Branch: `feat/editor-workspace` in a fresh worktree at `/private/tmp/repomon-feat-editor-workspace`.
This is a multi-part task with independent pieces. Use `/teamwork-preview` to parallelize the
independent parts (the operator asked for this): B2 and B3 do not depend on each other, B4 is
independent of both, and B5 depends only on B1. Keep one integrator that owns the branch and the
final gate.

## Parts

**B1. Editor store.** New `apps/desktop/src/stores/editor.ts` holding, per lane: open files
(path, content, savedContent, mtimeMs, cursor, scrollTop, loading, loadError, saving, saveError,
conflict), active path, expanded dirs, dir cache; plus global tree column width and the wrap and
whitespace toggles. Move the file-loading, saving, conflict, and tree-loading logic out of
`FileEditorPanel.tsx` into the store so both surfaces share it. Persist open paths, active path,
cursor, expansion, and toggles per lane under `repomon.editor.v1` in localStorage; never persist
buffer content. Lane switch keeps other lanes' state in memory; the "discard and switch" confirm
is removed because nothing is discarded. Unit tests with the store patterns in `workspace.test.ts`.

**B2. Every language.** In `CodeEditor.tsx`, resolve languages with `@codemirror/language-data`
(`LanguageDescription.matchFilename`, then `matchLanguageName` for overrides, lazy `desc.load()`
reconfiguring the language compartment on resolve). Keep the ten already-bundled packages as the
synchronous fast path. Name-based mapping for extensionless files (Dockerfile, Makefile,
Justfile, .gitignore, .env*, Cargo.lock as TOML) and a first-line shebang sniff. Add
`languageOverride` prop. Extend the `extensionToLanguage` tests to cover yaml, toml, shell, go,
sql, dockerfile, and an unknown extension resolving to plain.

**B3. Editing features.** All CodeMirror, themed via the existing `appTheme` and `highlightStyle`
with CSS variables only: `autocompletion()` (language plus document words) and `closeBrackets()`;
`foldGutter()` and `foldKeymap`; indent unit sniffed per file; `Mod-/` toggle comment,
`Alt-ArrowUp/Down` move line, `Shift-Alt-ArrowDown` duplicate line; `rectangularSelection()`,
`crosshairCursor()`, multiple selections with `Mod-d` select next occurrence; `Mod-g` go to line;
whitespace and wrap toggles as compartments; an indent-guide ViewPlugin (no third-party package);
the search panel restyled to the app's inputs and buttons. Add
`@codemirror/autocomplete` and `@codemirror/language-data` to package.json with exact-range
versions like the existing entries.

**B4. Non-text files.** Daemon: `FileReadResult` gains `kind: "text" | "binary" | "image"`
(image by extension list in the spec); new `file.read_raw { lane_id, path }` returning
`{ base64, mime, size }` under the same 2 MiB cap, local-only (add to `remote.rs`'s list next to
`file.read`), documented in `docs/protocol.md`, unit tests in `files.rs`. Frontend: image tab
with checkerboard background, natural size, zoom-to-fit toggle; binary tab with size and a
"not a text file" notice; too-large keeps the existing clear error. Regenerate bindings.

**B5. Editor workspace mode and tree.** A center-pane mode, mutually exclusive with Multitasking
(see `openPanelTab` and `workspace.setMultitasking` in `App.tsx`, and how `is-multitasking`
switches the `mission-grid`). Toolbar entry stays "Editor"; clicking it opens the center mode,
and the rail tab remains reachable from the Editor button's split action or `mod+7` (choose one
clean interaction, document it in `docs/desktop.md`). Layout: resizable tree column (default
240px, min 180px, width persisted in the store) on the left; tab strip, editor, and a status line
(language name clickable to override, line:col, selection count, indent unit, wrap and whitespace
toggles) on the right. Tree gains reveal-active-file, keyboard navigation (arrows, Enter, Left
and Right), SVG file-kind icons from `icons.tsx`, and a filter box over loaded levels. The rail
`FileEditorPanel` is rebuilt on the shared store and keeps its compact tree-as-picker behavior.

## Design rules

Run `/frontend-design` and `/impeccable` for B5 and for the status line and search panel in B3.
No emoji glyphs anywhere; icons are SVG. No em-dashes in any copy. Match the existing surfaces
(the Multitasking toolbar row, the Git panel's tree treatment, the Repomail panel) rather than
inventing a new visual language.

## Gate

In `apps/desktop`: `bun run check`, `bun run test`, `bun run bindings:check`. In the worktree
root: `cargo test -p repomon-daemon`. Live: desktop preview build via `tauri.preview.conf.json`,
then walk the five acceptance steps in the spec's Phase 1 section on the repomon lane and
screenshot each under `/private/tmp/repomon-feat-editor-workspace/qa/`. Never point a debug
daemon at `/tmp/repomon-azaleas.sock`.

Commits: one per part, 1-line Conventional Commits (`feat(desktop): ...`, `feat(daemon): ...`),
no AI co-author trailer. Report to the coordinator address with branch, commit hashes, screenshot
paths, and anything you deliberately left out. Do not merge.
