# Brief H (F7): PDF viewing in the editor

Goal: opening a `.pdf` in the editor (center mode or rail) shows the document instead of the
"not a text file" notice, without loading it through base64 and without a heavy dependency.

Branch: `feat/editor-pdf-viewer` in a fresh worktree at `/private/tmp/repomon-feat-editor-pdf`
from current main (cec58d4 or later).

## Design

1. **Daemon kind.** `crates/repomon-daemon/src/files.rs`: `read_file` returns
   `kind: "pdf"` for the `.pdf` extension (checked before the binary sniff, like images), with
   empty `content`. Unit test, integration test in `tests/file_rpcs.rs`, `docs/protocol.md`
   updated. No new RPC.

2. **Streaming via the Tauri asset protocol** (no `file.read_raw`, no base64, no 8 MiB cap).
   - `apps/desktop/src-tauri/tauri.conf.json`: enable `app.security.assetProtocol`
     (`"enable": true, "scope": []`); `csp` is already `null`, so nothing else blocks it. Confirm
     `tauri.preview.conf.json` and `tauri.release.conf.json` only override bundle settings so
     they inherit this.
   - A Tauri command in `apps/desktop/src-tauri/src/` (follow the existing command style there),
     e.g. `allow_worktree_assets(path: String)`, that calls
     `app.asset_protocol_scope().allow_directory(&path, true)` after checking the path is an
     existing directory. The frontend calls it once per lane worktree root the first time a PDF
     (or, later, any streamed asset) is opened in that lane; keep a per-root `Set` so it is not
     repeated. Register the command in the builder's `invoke_handler`.
   - Frontend builds the URL with `convertFileSrc(absolutePath)` from `@tauri-apps/api/core`
     where `absolutePath` is the lane worktree root joined with the relative path (the fleet
     store exposes the lane's `worktree.path`; verify the field name).

3. **`PdfViewer.tsx`** (`apps/desktop/src/components/`), used by both `EditorWorkspace.tsx` and
   `FileEditorPanel.tsx` where `ImageViewer`/`BinaryViewer` are chosen by `kind`:
   - An `<iframe>` with the asset URL, `title` set to the file name, filling the tab; the
     webview's built-in PDF renderer handles paging and zoom on macOS (WebKit) and Windows
     (WebView2).
   - A slim toolbar above it in the app's style: file name, size, and an "Open in system viewer"
     button using `openPath` from `@tauri-apps/plugin-opener` (already a dependency).
   - Linux (`navigator.userAgent` contains "Linux" and not "Android"): WebKitGTK has no built-in
     viewer, so render the toolbar plus a short note "PDF preview is not available on Linux" and
     make "Open in system viewer" the primary button; do not render the iframe.
   - If the iframe fails to load (listen for `error`, and treat a load that never fires within
     5 seconds as failed), swap to the same fallback note and button.
   - No emoji, no em-dashes, colors via CSS variables only. Run `/frontend-design` and
     `/impeccable` for the toolbar and fallback states.

4. **Store and tabs.** `stores/editor.ts` already carries `kind` from `file.read`; make sure a
   `pdf` tab is never marked dirty, never saved, is excluded from the git gutter, autocomplete,
   and search-replace, and that closing it needs no confirm. `openAt` on a PDF just activates the
   tab.

5. **Docs**: one paragraph in `docs/desktop.md` under the in-app editor section.

## Tests

- `files.rs`: `.pdf` classified as `pdf`, not `binary`.
- `PdfViewer.test.tsx`: renders the iframe with a `convertFileSrc`-derived URL on macOS and the
  fallback note on a Linux user agent (mock `navigator.userAgent`); the open-externally button
  calls the opener with the absolute path; the load-failure fallback appears when the iframe
  fires `error`.
- `stores/editor.test.ts`: a `pdf` tab is read-only and never dirty.
- Rust: a unit test for the command's directory check (rejects a file or a missing path).

## Verification

Gate: `bun run check`, `bun run test`, `bun run bindings:check` in `apps/desktop`;
`cargo test -p repomon-daemon` and `cargo check -p repomon-desktop` (or whatever the src-tauri
crate is named in Cargo.toml) at the worktree root. Live check, best-effort: build the debug
desktop binary in the worktree (`cargo build -p <src-tauri crate>` plus `bun run build`) and use
the isolation recipe in `apps/desktop/e2e/isolated.sh` (unique tmux session, throwaway config and
data dirs, own socket, a fixture repo containing a small PDF) to open the PDF; screenshots may be
blocked by macOS screen-recording permission, in which case say so and rely on the tests.

## Rules

Work only in the worktree; the main checkout at /Users/azaleas/Developer/Claude/repomon must not
change. Never run a daemon against `/tmp/repomon-azaleas.sock`, never kill processes by name
pattern, never `git add -A`. Commits: 1-line Conventional Commits (`feat(daemon): ...`,
`feat(desktop): ...`), no body, no co-author trailer. Do not merge, push, or build the Tauri
bundle. Report with commit hashes, gate tails, and anything left out.
