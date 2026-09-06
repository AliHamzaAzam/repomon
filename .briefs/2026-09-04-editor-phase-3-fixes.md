# Brief G: Editor phase 3 review fixes (before merge)

Branch `feat/editor-phase-3`, worktree `/private/tmp/repomon-feat-editor-phase-3`. All gates are
green (558 vitest, 338 daemon, 364 core, bindings, tsc). Fix the items below in one or two
`fix(desktop): ...` / `fix(daemon): ...` commits, re-run the gate, report.

1. **Bound the git gutter diff** (`apps/desktop/src/components/lineDiff.ts`, `myersDiff` around
   lines 84 to 117). The implementation copies the whole `v` array per `d` step (O(D^2) time and
   space) and nothing caps it except the 2 MiB `large` flag. Do both: (a) switch to the
   linear-space Myers variant or at least stop storing a full `v` snapshot per step (store only
   the slice that changed, or reconstruct by re-running forward passes); (b) add a hard cap: if
   `lines_a + lines_b > 20000` or the edit distance search exceeds `d > 4000`, return
   `{ kind: "too-large" }` and have `CodeEditor.tsx` clear the gutter markers and show a
   muted status-line note "Diff markers off: change too large" until the next successful diff.
   (test: a 3000-line document versus a fully rewritten version completes under 200 ms in vitest
   and returns hunks; a document over the cap returns `too-large` and renders no markers.)

2. **Rail Save button bypasses the diff-base refresh** (`apps/desktop/src/components/FileEditorPanel.tsx`
   around lines 459 to 469 calls `saveFile` directly; only CodeEditor's `Mod-s` binding calls
   `refreshDiffBase`). Move the base refresh into the store's `saveFile` completion (or a
   `onSaved` callback CodeEditor subscribes to) so every save path refreshes the base. (test: a
   save triggered from the store, not the keymap, causes one `file.diff_base` call.)

3. **`file.diff_base` reads the whole blob before the size check**
   (`crates/repomon-daemon/src/files.rs` `diff_base`). Run `git cat-file -s HEAD:<path>` first
   and return `ReadError::TooLarge` when it exceeds `READ_CAP_BYTES`, before `git show`. Keep
   the `missing` mapping for a non-zero exit. (test: existing diff_base unit test plus a cap case
   using a small temporary cap or a mocked size, whichever the file's test style supports.)

4. **Commit message style**: the F6 commit `b50ab5c` uses scope `editor` and "8mib"; the repo
   uses `desktop`/`daemon` scopes and "MiB". Do not rewrite history on the shared branch; just
   make sure your new commits follow `fix(desktop): ...` / `fix(daemon): ...`.

## Gate

`bun run check`, `bun run test`, `bun run bindings:check` in `apps/desktop`;
`cargo test -p repomon-daemon` at the worktree root. Grep the diff for em-dashes and emoji (must
be empty) and touched frontend files for hex colors (0). Never kill processes by name pattern,
never touch `/Applications/Repomon.app` or `/tmp/repomon-azaleas.sock`, never `git add -A`.
Report commit hashes and gate tails. Do not merge.
