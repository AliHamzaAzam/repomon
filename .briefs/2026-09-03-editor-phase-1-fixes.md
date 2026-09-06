# Brief C: Editor phase 1 review fixes (before merge)

Branch `feat/editor-workspace`, worktree `/private/tmp/repomon-feat-editor-workspace`. All gates
were green (tsc, 459 vitest, 16 daemon suites, bindings) and the daemon side is approved. The
frontend review found the items below. Fix all of them on the same branch, one commit per numbered
group or one combined `fix(desktop): ...` commit, then re-run the full gate and report.

## Must fix

1. **Theme regression in `CodeEditor.tsx` (highlightStyle).** The branch replaced main's
   CSS-variable palette with 13 hard-coded One Dark hex colors (`#c678dd`, `#e06c75`, ...).
   That breaks every non-dark theme and the app's four-accent identity. Restore main's
   `HighlightStyle.define` (see `git show main:apps/desktop/src/components/CodeEditor.tsx`,
   all values are `var(--signal)`, `var(--attention)`, `var(--muted)`, `var(--foreground)`,
   `var(--fault)` and `color-mix(...)` of those) and extend it with any new tags you need using
   the same variables. Zero hex literals in the file. Same rule for any new `EditorView.theme`
   entries, the search panel restyle, and the indent guides.

2. **Rail panel rebuilds the EditorView on every keystroke.** `FileEditorPanel.tsx` (around
   line 410) uses `<Show when={activeFile()} keyed>`. Every edit or cursor move produces a new
   `OpenFile` object through `updateOpenFile`, so the keyed Show tears down and recreates
   `<CodeEditor>` (undo history lost, focus lost after each character). Use the non-keyed
   accessor form exactly as `EditorWorkspace.tsx` already does (`{(file) => ... file().path}`).
   Add a test proving the same `.cm-content` DOM node survives typing three characters and a
   cursor move in the rail panel.

3. **Multi-cursor does not exist.** `Mod-d` is bound to `selectNextOccurrence` but
   `EditorState.allowMultipleSelections.of(true)` is missing, so CodeMirror collapses every
   added range to one. `rectangularSelection()` and `crosshairCursor()` are absent too. Add all
   three (from `@codemirror/state` and `@codemirror/view`) and a test that `Mod-d` twice yields
   two selection ranges.

4. **Stale async language load wins.** The language effect depends on `props.value`, so it
   re-runs on every keystroke, and its `.then()` reconfigures the compartment without checking
   the request is still current. Depend on `path` and `languageOverride` only (sniff the shebang
   once from the initial content, not reactively), keep a request token or the requesting path,
   and drop the dispatch if `props.path` changed in the meantime. Test with two pending fake
   loads resolving out of order.

5. **Cursor and scroll restore only on mount.** In the center workspace the same `CodeEditor`
   instance persists across tab switches, so `initialCursor`/`initialScrollTop` applied in
   `onMount` never re-apply. Make restoration reactive on `path` (apply once per activation,
   after the doc replace). Acceptance steps 2 and 4 in the spec depend on this.

6. **Status line: selection count** (brief B5) is missing. Track `state.selection.ranges.length`
   and the selected character count in the existing cursor-activity callback and show
   "N selections" / "N chars" next to Ln/Col when relevant.

7. **Document-word completion** (brief B3) is missing: only bare `autocompletion()` is wired.
   Add a completion source that offers words (3+ chars) from the current document alongside the
   language completions.

## Should fix

8. `setTreeColumnWidth` reads and writes the whole localStorage blob on every mousemove during
   the drag. Update the signal on mousemove; persist once on mouseup.

9. **SVG is text.** The daemon classifies `.svg` as `kind: "image"` and returns empty content,
   so SVGs cannot be edited. Remove `svg` from `IMAGE_EXTENSIONS` in `crates/repomon-daemon/src/files.rs`
   (keep it in `mime_for_path`), update the unit test and `docs/protocol.md`, and re-run
   `cargo test -p repomon-daemon`. SVG preview can come later.

10. One em-dash in a Rust doc comment in `files.rs` ("oversized file — oversized files"): replace
    with a colon or period. Re-grep the whole diff for em-dashes (`git diff main..HEAD | grep "^+" | grep "—"`)
    and for emoji before committing.

11. Make sure no QA fixtures are committed (`git status` must be clean of `Dockerfile`, sandbox
    scripts, screenshots). QA screenshots stay under the worktree's `qa/` directory, untracked.

## Gate

`bun run check`, `bun run test`, `bun run bindings:check` in `apps/desktop`;
`cargo test -p repomon-daemon` at the worktree root. Then redo the spec's acceptance steps 2
(multi-select, cursor restore) and 4 (rail and center share tabs) in your isolated QA app and
replace the affected screenshots. Report with commit hashes, the gate tails, and the screenshot
paths. Do not merge.
