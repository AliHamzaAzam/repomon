# Repomon Desktop - design spec

Approved 2026-07-20. A multiplatform desktop GUI for repomon: a full replacement daily-driver
(not a companion), shipping on macOS + Linux + Windows, with fully interactive embedded
terminals. Open source now, premium features later. v1 talks only to the local daemon.

## Decisions

- **Stack**: Tauri 2 shell; Rust host crate links `repomon-core` (reusing `DaemonClient`,
  protocol, model types); SolidJS + Tailwind frontend; xterm.js (`@xterm/xterm` 6) terminals.
- **Layout**: mission control - persistent fleet sidebar, tabbed/split/grid terminal center,
  collapsible repomind panel. TUI keybindings carry over.
- **Placement**: `apps/desktop/` in this repo; `src-tauri` joins the Cargo workspace.
- **Toolchain**: bun + Vite 7 + solid-js 1.9 + Tailwind 4; ts-rs 12 generates TS types from
  the Rust model (feature-gated `ts` feature in repomon-core; bindings committed + CI-gated).

## Architecture

The frontend never speaks JSON-RPC. The Rust host is the only protocol peer:

- One `DaemonClient` (auto-reconnect, keepalive, subscribe replay) owned in Tauri state.
- Daemon bootstrap (`ensure_daemon`/`spawn_daemon`/`connect_with_retry`) lifted from
  `repomon-tui/src/lib.rs` into `repomon-core::launch`; `repomond` bundled as a Tauri sidecar
  (`service::repomond_path()` already prefers a binary beside the executable). A running
  daemon always wins; version-mismatch banner off `daemon.status.version`.
- Thin command surface: `daemon_call` (JSON-RPC passthrough with structured errors),
  `daemon_subscribe` (JSON event channel, excluding `event.agent.bytes`),
  `term_watch`/`term_unwatch` (raw byte channels), `connection_status`.

## Terminal pipeline

- Out: `agent.watch_bytes` -> `event.agent.bytes` -> base64-decode in Rust -> per-terminal
  coalescing buffer (16ms flush or >32KB) -> `Channel<InvokeResponseBody::Raw>` -> xterm.write.
  1MB pending cap with drop + capture resync. Fallback transport (localhost WebSocket) isolated
  behind `term.ts`/`term.rs` if a platform underperforms.
- In: lift `translate_key` into `repomon_core::input`; host mirrors the TUI coalescer
  (printable runs -> `agent.send_input`, control keys -> `agent.key`, 8ms debounce); shell
  tiles send per-key `agent.key {window}`.
- Resize: fit addon + ResizeObserver (100ms) -> `agent.resize`/`agent.fit`; orchestrator pane
  -> `orchestrator.resize`.
- Addons: webgl (context-loss -> DOM fallback; canvas addon no longer exists in xterm 6), fit,
  search, unicode11, clipboard. Renderer setting auto/webgl/dom. Defer addon-image.
- Reconnect: `DaemonClient` replays only one byte watch; the host re-asserts every open
  terminal's watch on reconnect.
- Previews (non-focused grid tiles, sidebar peek): `viewport.set` + `event.agent.output`.

## Feature parity map

Fleet sidebar (repos -> lanes -> sessions; attention/gate/stalled/rate-limit/external badges);
terminals (tabs/split/grid + shell tiles); repomind panel (`orchestrator.*`); spawn/adopt/
stop/pin/rename/auto-continue; lane create/delete/merge/diff; repo add/remove/discover +
`fs.browse`; approve/deny via `agent.prompt`/`agent.answer {expect_summary}` with `-32010
DIALOG_CHANGED` retry; triage queue; filters + urgent-only + needs-you jump; timeline/
sessions/search/commits; settings over `config.get/set`; usage corner; notifications feed.
Theming: accent -> CSS custom properties, light/dark.

## Daemon-side change (additive)

Hoist the `event.notification` broadcast out of the `[remote] enabled` gate in
`repomon-daemon/src/notify_watch.rs` (APNs stays gated). GUI popups via
tauri-plugin-notification; `local_watcher_seen` keeps daemon-native popups suppressed while
the GUI runs.

## Windows

Rides on `release/windows-preview` (named-pipe transport inside repomon-core;
`DaemonClient::connect` signature unchanged). Zero Windows-specific code in the desktop crate:
endpoint only via `config::socket_path(cfg)`, boot only via `repomon_core::launch`. Windows
matrix rows + NSIS + `repomond.exe` sidecar are workflow-file changes once the branch merges.

## Packaging / CI / testing

- Bundles: notarized dmg, NSIS, AppImage + deb/rpm; tauri-plugin-updater (minisign) with
  `latest.json` on GitHub releases; separate `desktop-release.yml` on the same `v*` tags;
  Homebrew cask later.
- CI: existing checks job gains Linux webkit deps; new frontend job (bun, tsc, vitest, build);
  bindings staleness gate (regenerate + git diff).
- E2E: isolated daemon harness (verify-skill pattern) + tauri-driver on Linux; macOS manual
  pass + host-level smoke.

## Milestones

M1 skeleton/connect, M2 types + IPC layer, M3 fleet sidebar, M4 terminal pipeline,
M5 mission-control layout, M6 actions and modals, M7 notifications, M8 packaging/updater,
M9 e2e + Windows on-ramp. Implementation plans: `docs/superpowers/plans/2026-07-20-desktop-gui-*.md`.
