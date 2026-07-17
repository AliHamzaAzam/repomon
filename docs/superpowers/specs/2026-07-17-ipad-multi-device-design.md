# iPad mission control + multi-device remote — design

Approved 2026-07-17. Companion implementation plan: repomon PRs per workstream (A1–A6 below)
and repomon-ios milestones (B1–B7, tracked in that repo).

## Decisions

1. **iPad experience: mission-control dashboard.** Persistent fleet sidebar, a grid of live
   agent terminal tiles, a detail/chat inspector for the focused agent, hardware-keyboard
   typing into agents. The iPhone flow is unchanged; one universal target switches on size
   class.
2. **Remote power: full fleet control.** Paired devices may spawn/stop/adopt agents and
   create/merge/delete lanes. Enabled by per-device named tokens with individual revocation;
   the legacy shared config token keeps working.
3. **Streaming: hybrid, per connection.** Grid tiles ride per-connection viewport capture
   deltas (~1s, the Mac TUI fleet model). The focused/expanded terminal gets the true PTY
   byte stream. Byte watches are refcounted per window so the Mac TUI, an iPhone, and an
   iPad can stream concurrently, including the same window.

## Daemon architecture (repomon)

- **ConnSession** replaces the daemon-global viewport/focus/watch slots: every connection
  (Unix socket or WebSocket) registers one and `rpc::dispatch` receives it. The capture poll
  loop streams the **union** of live sessions' viewports; focused windows (any fresh session)
  get the fast cadence and cursor capture. Single-connection behavior is unchanged, so
  existing clients are wire-compatible.
- **Byte watches**: `HashMap<window, WatchEntry{refs, fifo, generation}>`. One tmux
  `pipe-pane` per window shared by all watchers via the broadcast bus; last unref stops the
  pipe; connection drop unrefs everything; EOF self-cleanup is generation-guarded.
  `event.agent.bytes` is forwarded only to connections watching that window.
- **Fit arbitration**: a local session with a fresh focus beat owns its window (TUI
  precedence, unchanged); among remotes the freshest `last_interaction` wins — the device
  you last typed on. `agent.resize` stays local-only.
- **Per-device tokens**: `remote_devices` table (name, token, role='full', created/last-seen,
  capped). Sync `Ctx.remote_tokens` cache (std RwLock — the WS handshake callback is
  synchronous) seeded from store + legacy config token. Local-only RPCs `remote.pair` /
  `remote.devices` / `remote.revoke`; CLI `repomon remote pair --name ipad`, `devices`,
  `revoke`. Revocation kicks live connections on their next request.
- **Allowlist**: expands to full control; still blocked: daemon lifecycle, config/secrets,
  host terminal + filesystem, `agent.resize`, orchestrator start/stop, `remote.*`.

## App architecture (repomon-ios)

- Universal target (`TARGETED_DEVICE_FAMILY "1,2"`); `RootView` branches on
  `horizontalSizeClass`: compact → existing TabView, regular → `NavigationSplitView`
  (fleet sidebar / dashboard grid) + `.inspector` (focused agent chat/dialog/approve).
- Tiles render ANSI-parsed capture deltas (extracted `CapturePaneText`); exactly one focused
  tile holds a live SwiftTerm byte stream pinned to the pane's true grid; only the expanded
  overlay negotiates `agent.fit`. Tiles never call fit.
- Hardware keyboard: focused terminal is first responder; raw bytes translate through a pure
  `TerminalInput` mapper to `agent.send_input`/`agent.key`; Cmd-chords stay SwiftUI shortcuts.
- `DaemonFeatures` (from `daemon.status.version`) gates everything so the app degrades
  cleanly against older daemons (polling tiles, single-watch rules, hidden management UI).

## Wire-compat contract

- No envelope changes. `viewport.set` and `agent.watch_bytes` keep their shapes; semantics
  become per-connection (a strict superset for single-connection clients).
- `agent.watch_bytes {on:false}` without `window` unwatches all of the session's watches for
  the lane (the TUI's stop path depends on this).
- New local-only methods: `remote.pair`, `remote.devices`, `remote.revoke`.
- Version gates: the daemon release shipping A1–A4 is the floor for
  per-connection viewports, multi-watch, remote fleet control, and multi-remote fit.
