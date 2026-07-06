# Full Linux support

**Status:** Approved
**Date:** 2026-07-06
**Branch:** `feat/linux-support`

## Problem

repomon is macOS-first. Everything already compiles on Linux and the core loop
(daemon auto-spawn, tmux agents, git worktrees, XDG paths, inotify watching,
APNs push) works, but the edges do not:

- `repomon daemon install` hard-errors: the non-macOS arm of
  `repomon-core/src/service.rs` is a stub ("service management is currently
  macOS-only (launchd)").
- Desktop notifications reach `notify-send` but ignore the sound and
  click-to-focus settings; the settings chime preview is a no-op.
- The tmux drag-select copy binding hardcodes `pbcopy`, so copy silently fails.
- Clipboard image paste is pngpaste/AppleScript only and returns nothing.
- Session liveness probing shells to `lsof`, which may be absent on Linux.

Releases already ship a Linux x86_64 binary and `install.sh` handles Linux, so
users can hit all of these today. The goal is real parity, not just
compilation, verified in a genuine Linux environment via Apple containers
(aarch64 VMs) since development happens on macOS.

## Design

Style rules, matching the existing codebase: shell out to platform tools
rather than adding integration crates (no zbus/notify-rust); keep pure
"builder" functions (unit text, argv vectors) compiled and tested on every
platform, with thin `cfg`-gated `Command` wrappers; every Linux path is
best-effort or probe-gated so headless environments (no DBus, no display, no
systemd) never fail.

### Shared helpers (repomon-core)

- `exec.rs`: `find_in_path(bin)` over `$PATH`, with a pure `find_in(path_var,
  bin)` core testable against a synthetic PATH.
- `clipboard.rs`: `copy_argv()` probed once (OnceLock). macOS: `pbcopy`.
  Linux: `wl-copy` when `$WAYLAND_DISPLAY` is set and the tool exists, else
  `xclip -selection clipboard -in`. Pure `select_copy_argv(wayland, has)` for
  tests, plus `copy_pipe_command()` (space-joined, for tmux) and
  `copy_text(text)`.
- Consumers: the tmux copy binding uses `copy_pipe_command()`; when no tool is
  present it binds `copy-selection-and-cancel` instead, and the existing
  `set-clipboard on` keeps copy working via OSC52. The TUI's
  `copy_to_clipboard` delegates to `clipboard::copy_text`.

### Notifications (repomon-core/src/notify.rs)

- Pure `notify_send_args(title, body, sound)`: `-a repomon -u normal`, plus
  `-h string:sound-name:message-new-instant` when sound is on; title and body
  come last.
- `sound_argv()` probe: `canberra-gtk-play -i message-new-instant`, else
  `paplay` with the freedesktop `message.oga` when that file exists. Runs
  alongside notify-send so the chime works even where the hint is ignored.
- `play_chime()` gains a Linux arm so the settings sound preview works.
- `click_focus` is documented macOS-only; there is no portable way to focus a
  terminal from a daemon on Linux.

### systemd user service (repomon-core/src/service.rs)

- Cross-platform pure parts, unit-tested everywhere: `UNIT_NAME =
  "repomon.service"`, `generate_unit(program, socket)` producing
  `ExecStart`, `Restart=always`, `RestartSec=2`,
  `StandardOutput/StandardError=append:<data_dir>/logs/repomond.{out,err}.log`
  (so `repomon daemon logs` and the TUI log view keep working unchanged), and
  `WantedBy=default.target`; `systemctl_user_args(ServiceOp)` builds each
  `systemctl --user ...` argv.
- `#[cfg(target_os = "linux")] mod platform`: `service_file_path()` under
  `$XDG_CONFIG_HOME/systemd/user/`; `systemd_available()` checks
  `/run/systemd/system` and `systemctl --user is-system-running`,
  distinguishing "no systemd (e.g. containers)" from "no user session, try
  `loginctl enable-linger`", and always mentions that the TUI auto-starts
  `repomond` so a service is optional; install writes the unit then
  `daemon-reload` + `enable --now`; uninstall/start/stop/status mirror the
  launchctl shapes.
- The old `not(macos)` stub narrows to `not(any(macos, linux))`.
  `plist_path` is renamed `service_file_path` across all platform mods.
- CLI doc strings become platform-neutral; successful install on Linux prints
  a `loginctl enable-linger` hint.
- Tests assert unit text and argv shapes only; nothing invokes live systemctl.

### Clipboard image paste (repomon-tui/src/app.rs)

Shared wrapper plus per-platform `write_clipboard_png(path)`: macOS keeps the
pngpaste/AppleScript body; Linux tries `wl-paste --type image/png` then
`xclip -selection clipboard -t image/png -o`. The temp file moves off
hardcoded `/tmp` to `std::env::temp_dir()` and the result is validated
against the PNG magic bytes.

### Process probe (repomon-daemon/src/rpc.rs)

Linux `live_claude_cwds` scans `/proc` instead of `ps` + `lsof`: a process
matches when its `comm` or the basename of cmdline argv[0] equals `claude`;
cwd comes from `read_link("/proc/<pid>/cwd")`; unreadable entries are skipped.
This drops the lsof dependency entirely. An inline test asserts
`/proc/self` resolves to the current process and cwd.

### CI, release, install.sh

- New `.github/workflows/ci.yml` on PR and push-to-main: matrix `macos-14` and
  `ubuntu-latest`; pinned toolchain via `rustup show`; tmux installed on
  Linux so the daemon integration tests actually run; `cargo fmt --check`,
  `cargo clippy --workspace --all-targets --locked -- -D warnings`,
  `cargo test --workspace --locked`.
- `release.yml` adds a `build-linux-arm` job (`ubuntu-24.04-arm`,
  `aarch64-unknown-linux-gnu`), matching the Apple-container test
  architecture.
- `install.sh` learns the Linux `aarch64|arm64` arch case.

### Docs

README platforms line (macOS and Linux, x86_64/aarch64, WSL2), a "run the
daemon as a service" subsection (launchd vs systemd user unit, linger note,
service is optional), platform-notes bullets (libnotify, sound players,
wl-clipboard/xclip, OSC52 fallback, click-to-focus macOS-only);
architecture.md launchd mentions become launchd/systemd; STATUS.md header
refresh (stale commit/test counts) plus a Linux milestone.

## Verification

macOS: full workspace gate (fmt, clippy -D warnings, test), reinstall,
daemon restart, live smoke of notifications and copy.

Linux, inside an Apple container (aarch64):
1. Build + test with tmux and git installed; proves the suite is headless-safe.
2. Runtime smoke: foreground `repomond`, then daemon status, repo add, lane
   list, daemon logs.
3. Graceful degrade: `daemon install` fails with the systemd-absent message
   naming the auto-start fallback; `daemon stop` does not error.
4. Probe fidelity: a fake `claude` shebang script running from a known dir is
   counted by the `/proc` probe.
5. tmux binding: `copy-selection-and-cancel` without tools; switches to
   `copy-pipe-and-cancel "xclip ..."` once xclip is installed.

Ongoing guard: the ubuntu-latest CI leg runs the same suite on every PR.

## Risks

- `XDG_RUNTIME_DIR` absent (containers, some SSH): socket path already falls
  back to the temp dir; systemd install is gated by the availability probe.
- `systemctl --user` needs a live user manager: the probe's error text points
  at `loginctl enable-linger`.
- `Restart=always` vs the socket-shutdown race in `daemon stop`: the trailing
  `service::stop()` settles it, the same shape launchd KeepAlive has today.
- claude comm-name mismatch on exotic launchers: dual comm/argv0 match plus
  the existing 30s sticky grace bounds the damage.
