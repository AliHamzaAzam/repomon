# Repomon Desktop M2-M9 Implementation Plan

Date: 2026-07-20
Status: in progress
Spec: `docs/superpowers/specs/2026-07-20-desktop-gui-design.md`

## Outcome

Ship a usable local-daemon desktop client in `apps/desktop` with a typed host boundary, live fleet navigation, interactive terminals, mission-control layouts, daily actions, notifications, release metadata, and isolated end-to-end coverage.

## Delivery sequence

### M2: Types and IPC

1. Add a feature-gated `ts-rs` binding generator to `repomon-core` and commit generated frontend types.
2. Preserve JSON-RPC error code, message, and data through `DaemonClient`.
3. Add Tauri `daemon_call`, `daemon_subscribe`, and connection commands. Byte events remain private to the terminal transport.
4. Add a typed frontend RPC map, event hub, resource stores, tests, and a bindings staleness check.

### M3: Fleet sidebar

1. Render repos, lanes, sessions, attention states, git state, pin state, and activity.
2. Add fuzzy filtering, urgent-only mode, keyboard movement, needs-you jump, and usage status.
3. Preserve selection across refresh and event-driven updates.

### M4: Terminal pipeline

1. Add multi-window byte-watch tracking to `DaemonClient` and reassert all watches after reconnect.
2. Add host terminal watch channels with base64 decode, 16 ms or 32 KiB batching, a 1 MiB pending cap, and capture resync.
3. Add xterm.js 6 with fit, search, webgl fallback, unicode11, clipboard, input coalescing, and resize handling.
4. Test routing, overflow recovery, key translation, and watch lifecycle.

### M5: Mission control

1. Add terminal tabs and focused, split, and grid layouts.
2. Add shell tiles, preview snapshots, and the collapsible repomind pane.
3. Persist layout and renderer preferences locally.

### M6: Daily actions

1. Add spawn, adopt, stop, pin, rename, auto-continue, lane, and repo actions.
2. Add prompt approval and denial with `DIALOG_CHANGED` refresh and retry.
3. Add triage, timeline, sessions, commit search, and settings surfaces.
4. Cover command parameter construction and destructive confirmation paths with tests.

### M7: Notifications

1. Broadcast `event.notification` independently from the remote push gate.
2. Add an in-app feed, unread state, and native notification integration.
3. Keep daemon-native notifications suppressed while a local desktop watcher is present.

### M8: Packaging and updater

1. Configure dmg, NSIS, AppImage, deb, and rpm targets plus the `repomond` sidecar.
2. Add updater metadata and a dedicated signed desktop release workflow.
3. Document signing, notarization, and local packaging prerequisites.

### M9: Verification and Windows on-ramp

1. Add isolated daemon smoke coverage using private config, data, socket, and tmux state.
2. Add Linux WebDriver scaffolding and host-level smoke tests.
3. Add Windows workflow rows and sidecar naming without desktop-crate platform branches.
4. Run formatting, linting, frontend tests/build, Rust tests, and the isolated smoke suite.

## Commit boundaries

Each milestone lands as a focused commit. Generated files and dependency locks ship with the milestone that introduces them. User-owned untracked files remain untouched.
