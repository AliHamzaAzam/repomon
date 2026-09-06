# Brief W1-fixes: review findings on feat/windows-daemon-boot

Same worktree `/private/tmp/repomon-feat-windows-boot`, same branch. The branch was rebased onto
main (now 5095fcb) and carries one coordinator commit (edd819b, `.github/workflows/windows-artifact.yml`,
a manual Windows-only build + smoke + artifact workflow); do not rewrite history. Fix all eight,
one commit per item (or grouped where they touch the same function), 1-line Conventional Commits,
no co-author trailer. Rules as before: worktree only, no production socket or DB, no kill by name,
no `git add -A`, no hex, emoji, or em-dashes, do not merge, push, or build the bundle.

1. **CRITICAL, `apps/desktop/src-tauri/src/cli.rs`:** `cli_status`, `cli_install`, `cli_uninstall`
   are sync `#[tauri::command]`s, so `user_path()` (login shell `-ilc printf %s "$PATH"`) and
   `installed_version()` run on the main thread with no timeout: a slow rc chain freezes the
   window, a blocking rc hangs the app. Make the three commands async, do the filesystem and
   subprocess work in `tauri::async_runtime::spawn_blocking`, bound the shell probe with the
   `wait_with_timeout` helper already in `boot.rs` (2 s), and on timeout fall back to the app's
   own PATH with `on_path: null` plus a note in the card ("could not read your shell PATH").
   Test: a probe against a shell script that sleeps returns within the bound.
2. **CRITICAL, `desktop-preview.yml` and `desktop-release.yml`:** the Windows smoke step runs after
   `tauri-action` has already published the installer and latest.json, so it gates nothing. Fix by
   running the smoke before publish: build with `tauri-action` `releaseDraft: true` on Windows and
   undraft in a final step that `needs` the smoke, or add a gating Windows build+smoke job the
   publishing matrix `needs`. Keep the macOS/Linux behavior unchanged and keep the serialized
   latest.json handling intact; explain the chosen shape in a comment.
3. **`.github/workflows/ci.yml`:** the Windows leg runs clippy and tests without `--target`, which
   is the bare-build case the crt-static doc says must never happen (proc macros cannot link a
   static CRT). Add `--target x86_64-pc-windows-msvc` to the cargo steps on the Windows matrix leg
   (conditional) and keep the doc's claim.
4. **`cli.rs` install/uninstall asymmetry:** `cli_install` removes any existing `~/.local/bin/repomon`
   or `repomond` (a real binary from install.sh included) before linking, while `cli_uninstall`
   refuses to remove non-symlinks; Install then Remove leaves the user with nothing. Fix: on
   install, if the target is a regular file (not a symlink) rename it to `<name>.bak` and report
   that in the result (the card shows "moved your existing repomon to repomon.bak"); guard
   `from == to`; on Windows rename-then-delete so a running `repomond.exe` does not block reinstall.
   Tests for both.
5. **`cli.rs` Linux AppImage:** `current_exe()` lives under the temporary squashfs mount, so the
   symlinks dangle after quit. Detect `APPIMAGE`/`APPDIR` env and copy instead of symlink, with the
   card saying the copy will not follow app updates (deb/rpm keep the symlink behavior). Test on
   the pure path logic.
6. **`crates/repomon-core/src/launch.rs::spawn_and_watch_boot`:** on an observed child exit, probe
   the endpoint once more before returning `DaemonExited`, so a duplicate that lost the bind race
   (AddrInUse) is reported as a healthy daemon, not a boot failure. Test with a fake endpoint that
   becomes reachable.
7. **`cli.rs` Windows registry:** a failed read of `HKCU\Environment\Path` is treated as an empty
   PATH and then written back, which would replace the user's whole user PATH with one entry. Only
   `NotFound` means empty; every other error propagates to the card. Test on the pure function.
8. **`.github/scripts/windows-smoke.ps1`:** `$expected` lacks `repomon.exe`, the binary this branch
   adds; add it.

Also, optional and tiny: reap the child in `spawn_and_watch_boot`'s Ok path (`let _ = child.try_wait()`).

Gate as before (`cargo test -p repomon-core -p repomon-daemon -p repomon-tui -p repomon-desktop`,
`bun run check`, `bun run test`, `bun run bindings:check`, sidecar prepare for aarch64-apple-darwin).
Report commit hashes per item, test names, gate tails, and what still needs Windows CI or the VM.
