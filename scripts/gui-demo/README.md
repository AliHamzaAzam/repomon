# GUI showcase recorder

Use existing matching release binaries on macOS. The recorder does not build or bundle Repomon.

```sh
scripts/record-gui-demo.sh --dry-run --tour --keep-sandbox --bin-dir target/release
scripts/record-gui-demo.sh --keep-sandbox --bin-dir target/release
scripts/record-gui-demo.sh --still --keep-sandbox --bin-dir target/release
```

The first command rehearses without capture. The next commands write `docs/gui-demo.gif`
and `docs/preview.png`. Keep the outputs uncommitted until reviewed.

## Tour drivers

The default `--tour-driver webview` loads the recorder's small Objective-C helper into the
copied executable. It drives ordinary DOM buttons and keyboard handlers inside that app's
WKWebView. It only accepts the phases `opening` and `tour` from a file in the disposable root.
It exposes no network or inspector port, changes no product code, and needs no System Events
Automation permission. Xcode command-line tools compile the recorder helper, not an app bundle.
Screen Recording permission is required for GIF and PNG capture.

`--tour-driver ax` retains the System Events AppleScript tour for native Accessibility testing.
That driver also requires Accessibility and System Events Automation permission for the
invoking terminal or host app. It walks all windows belonging to the demo PID because a WebKit
popover can become a tiny native front-window dialog while its controls remain in the main
window's tree. Pane-picker actions are scoped to that picker, not the sidebar's Hide buttons.

Both drivers use the same hero lane and showcase sequence. Every required view must pass its
content checks. `out/tour.log` records measured beat times. Views have readable dwell times.
The recorder uses ScreenCaptureKit on macOS 15 or newer, verifies the target window's owner PID,
and excludes the cursor, child windows, shadows and audio. A sandbox stop file tells the helper
to finish a timestamped PNG frame sequence after the last beat. Frames are captured at the
window's native pixel scale (2880x1800 on a 2x display), without a lossy video intermediate. macOS still composites a sharing
badge over the native traffic lights. Before recording, the recorder screenshots those static
76x34 logical pixels from the same window, then restores that region at native resolution
before GIF scaling. The pointer
is parked outside the window before capture so hover controls do not appear in either output.

## Isolation and evidence

Each run has a private directory under `/private/tmp/repomon-gui-demo.*` containing copied
executables, repositories, worktrees, config, data, Cocoa/WebKit stores, usage ledgers, fake
agents, a tmux server and two sockets. No HOME or CODEX_HOME override is used.

- Desktop `REPOMON_SOCKET`, `REPOMON_MCP_SOCKET` and config `socket_path` use `app.sock`.
- A recorder-owned observer forwards unchanged RPC frames to the private daemon's `demo.sock`.
- Kernel peer identities must match the copied desktop and daemon PIDs.
- Guards deny real-home access, outbound IP traffic, `/tmp/repomon-*.sock`, its `/private/tmp`
  alias and the current user's production socket. The desktop can connect only to Unix
  sockets inside its own disposable root and cannot execute its sibling daemon.
- An owned sentinel proves the production-pattern denial without touching production.
- The launcher closes inherited descriptors before running the app. lsof must resolve every
  desktop Unix peer to an internal socket pair or the recorder's `app.sock`; foreign owners,
  unknown peers and IP sockets fail verification. Checks repeat after opening and the tour.
- The copied desktop has a unique executable name to avoid ambiguous System Events references.
- Cleanup stops tracked child PIDs and only this run's private tmux server. It never kills by
  process-name pattern. `--keep-sandbox` retains fixtures and logs, not live demo processes.

`out/desktop-connection.json`, `out/desktop-lsof.txt`, `out/desktop-socket-audit.json` and
`out/launch.json` retain endpoint, process and binary evidence. The RPC method log is
`data/logs/desktop-rpc.jsonl`; it excludes tokens, terminal payloads and message bodies.

For a capture-free storage and DOM check:

```sh
scripts/record-gui-demo.sh --dry-run --diagnose-webview --keep-sandbox --bin-dir target/release
```

`out/webview-check.json` verifies private Cocoa paths, localStorage, IndexedDB, eight rendered
lane buttons, Needs you 2 and Running 4. The full tour uses those same checks. There is no
`--no-guard` option.

## Fixtures and capture

The fleet contains five repos and eight lanes, counting the pinned Repomind home separately
from the four project repositories. Seven fake terminal agents represent six agent kinds and
four statuses. The fixture has three authenticated messages, two supervision hold rows, two
Repomind plans, a playbook draft and 42 synthetic usage sessions.

Worktrees, commits and dirty files are prepared before daemon startup. This avoids recording
the daemon's initial clean Git state from its 180-second cache. The hero's live lane state must
report unstaged and untracked files, and must not be marked merged.
The disposable config disables daily price refresh so model rates use the built-in offline
table without a blocked-network error. The pane picker must be closed before its scene begins.

The tour shows the hero, four multitasking panes with footprints that fill both rows, Git diff, TypeScript editor, file finder,
SVG preview, usage chart and sessions, model rates, Repomail, supervision audit, Repomind,
shortcuts and the opening hero again. All agent output is synthetic; no real agent CLI runs.

Capture is restricted to the demo PID's window ID. The measured window bounds determine the
aspect ratio for one final Lanczos resize from native Retina pixels. A 1440x900 window produces a 1440x900 still and a 1200x750 GIF,
without stretching or a bottom capture band. There is no full-display fallback. GIF encoding
uses two palette passes with dithering disabled to keep text clean. It tries 12 fps/256 colors
first, then smaller settings, and refuses outputs of 15 MB or more. Alpha is flattened before
palette conversion so unchanged pixels can use GIF delta compression. Lossless source frames
and their timing manifest are retained under `out/frames/` with `--keep-sandbox`.
PNG capture uses ScreenCaptureKit's screenshot API with child windows and the cursor excluded.

See `docs/gui-demo-report.md` for the latest measured results and retained evidence paths.
