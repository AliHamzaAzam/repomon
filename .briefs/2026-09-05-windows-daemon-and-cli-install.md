# Brief W1: Windows daemon boot reliability and CLI install from the GUI

Operator report (2026-09-05): installing `Repomon_0.8.1_x64-setup.exe` on a Windows VM, the GUI
opened but the daemon was not running. Verified facts: the installer contains
`repomon-desktop.exe`, `repomond.exe` (26 MB), and `repomon-agent-host.exe` side by side; the
app resolves the daemon next to its own exe (`repomon_core::service::repomond_path`) and
spawns it detached with `CREATE_NO_WINDOW` (`repomon_core::launch::spawn_daemon`) from
`ensure_daemon` on connect. A spawn that fails or a daemon that exits at once is invisible to
the user: the connection pill only says "retrying". No `crt-static` is configured for the
MSVC targets, so `repomond.exe` and `repomon-agent-host.exe` need `VCRUNTIME140.dll` at
runtime. Product intent: download the GUI, it includes the daemon and just works; the CLI is
optional, installable from the GUI or from GitHub.

Branch: `feat/windows-daemon-boot` in a fresh worktree at `/private/tmp/repomon-feat-windows-boot`
from current main. Nothing here can be executed on Windows locally; correctness is proven by
unit tests, by the CI smoke job in item 3 (the operator will push the branch to run it), and by
the operator's VM.

## Scope

1. **Self-contained Windows binaries.** Add `.cargo/config.toml` with
   `[target.x86_64-pc-windows-msvc] rustflags = ["-C", "target-feature=+crt-static"]` (and the
   aarch64 msvc target) so every sidecar and the CLI carry the CRT. Confirm Tauri's own exe
   builds with it (it does in current Tauri; note any caveat found in tauri docs). Document in
   `docs/windows-validation.md`.
2. **Boot diagnostics that reach the user.** In `repomon_core::launch`: after spawning, wait up to
   3 s for the child to either stay alive or exit; if it exits, capture the exit code and the
   tail of the daemon log and return a typed error (`DaemonExited { code, log_tail, log_path }`,
   `DaemonMissing { path }`, `SpawnFailed { source }`). In the desktop app: the connection pill's
   retrying state shows that message and offers "Show log" (opens the log in the editor or the
   system viewer) and "Copy diagnostics"; the onboarding System check gets a row "Daemon binary
   launches" that runs `repomond --version` from the bundle and reports the exact error when it
   does not. On Windows also detect the two common causes explicitly: missing VC runtime (spawn
   error 0xC0000135 or "VCRUNTIME140.dll") with a one-line hint and the redistributable link,
   and a named pipe already owned by another session or a stale daemon (connect refused on
   `\\.\pipe\repomon-<user>`), with the hint to end the stale `repomond.exe`. Tests for the
   error mapping and the pill rendering.
3. **CI smoke test on Windows** in `.github/workflows/desktop-preview.yml` and
   `desktop-release.yml` (windows-latest job, after the bundle step): run the built
   `target/<triple>/release/repomond.exe --version`; start it on a temporary pipe name with a
   temp data dir, connect over the pipe with the built `repomon.exe` (`repomon --socket <pipe>
   lane list` or the lightest read RPC), then shut it down; install the NSIS output silently
   (`/S`) into a temp dir and assert the three exes exist next to each other. Fail the job on
   any of these. Keep the job under 5 minutes.
4. **CLI from the GUI.** Bundle the `repomon` CLI/TUI binary as a third external binary on every
   platform (`binaries/repomon`, with the Windows conf listing all three), built in the same CI
   steps that build the daemon sidecar. Add a Tauri command `cli.install` / `cli.status` /
   `cli.uninstall` (Rust, in `apps/desktop/src-tauri/src/`): macOS and Linux symlink
   `repomon` (and `repomond` so `repomon daemon install` works) into `~/.local/bin`, creating it,
   and report whether that directory is on PATH with the exact line to add to the shell rc;
   Windows copies the three exes into `%LOCALAPPDATA%\repomon\bin` and adds that directory to the
   user PATH (registry `HKCU\Environment` plus a `WM_SETTINGCHANGE` broadcast), never touching
   the machine PATH. Settings > System gets an "Command-line tools" card with status (installed
   version, path, on PATH or not), Install, and Remove; the onboarding Done step offers it;
   `repomon --version` from the installed copy is the verification. Tests for the path logic
   per platform (pure functions), the card's states, and the wizard hook.
5. **Docs**: README install section restructured as "Desktop app (includes the daemon)", then
   "Command line: from the app (Settings > System) or from GitHub (install.sh, install.ps1,
   Homebrew)"; `docs/desktop.md` and `docs/windows-validation.md` updated.

## Rules and gate

Worktree only; never edit the main checkout; never bind a daemon to
`/tmp/repomon-azaleas.sock` or copy the production database; never kill processes by name
pattern; never `git add -A`. No hex colors, no emoji, no em-dashes. Gate: `cargo test -p
repomon-core -p repomon-daemon -p repomon-tui`, `cargo test -p repomon-desktop`; in
`apps/desktop` `bun run check`, `bun run test`, `bun run bindings:check`; run the sidecar
prepare script locally for the macOS target to prove the third binary is picked up
(`TAURI_ENV_TARGET_TRIPLE=aarch64-apple-darwin bun run sidecar:prepare`). Commits: 1-line
Conventional Commits per numbered item, no co-author trailer. Do not merge, push, or build the
bundle. Report commit hashes, test names, gate tails, and exactly what still needs the Windows
CI run or the operator's VM to confirm.

## Evidence added 2026-09-05 14:30 (operator's VM log)

`%APPDATA%\repomon\data\logs\repomond.out.log` contains only `[launch]` lines: "daemon at
\\.\pipe\repomon-azama not ready after 6.1s (attempt 80, ...)", tier 2/3/4 retries, then
"failed to connect ... after 25.14s (126 attempts)", repeated on the next launch. No line from
the daemon itself, although its stdout and stderr are redirected into that file. Conclusion:
`spawn_daemon` succeeded (the log dir was created and the connect loop ran) and the child died
before any Rust code ran, consistent with a missing `VCRUNTIME140.dll` (loader exit 0xC0000135)
on a fresh VM. Item 1 (crt-static) is the fix; item 2 must map exit code 0xC0000135 (and
0xC000007B) to the plain hint and show the pipe name and log path in the pill.
