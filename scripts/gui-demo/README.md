# GUI showcase recorder

The macOS recorder creates a disposable synthetic fleet and captures a Repomon showcase using existing, matching release binaries. It never builds or bundles the app. Fresh worktrees usually lack `target/release`; supply the binary directory explicitly.

## Recording modes

```sh
scripts/record-gui-demo.sh --dry-run --bin-dir /Users/azaleas/Developer/Claude/repomon/target/release
scripts/record-gui-demo.sh --dry-run --tour --bin-dir /Users/azaleas/Developer/Claude/repomon/target/release
scripts/record-gui-demo.sh --still --bin-dir /Users/azaleas/Developer/Claude/repomon/target/release
scripts/record-gui-demo.sh --bin-dir /Users/azaleas/Developer/Claude/repomon/target/release
```

The first command seeds the fleet, launches the app, verifies its daemon connection and loaded fleet, then cleans up. The second
rehearses the actual AppleScript tour with no recording. The last two require Screen Recording
permission for the invoking terminal; the rehearsal and captures also require Accessibility
and Automation access to System Events. Run these from the operator's terminal. `--keep-sandbox`
retains fixtures and `out/*.json` evidence after stopping the demo app, daemon and tmux server.
`--skip-build` remains accepted for older commands; all runs already skip builds.

## Verify the desktop connection

Run `--dry-run --keep-sandbox` before using the tour.

1. Run the first command above with the existing binary directory.
2. Look for `PASS desktop fetched all 5 repos and 8 lanes` and `PASS no second repomond`.
3. Read the retained sandbox's `out/desktop-connection.json` for the app PID, daemon PID, endpoint paths, returned fleet counts and selected viewport.

The check requires the kernel-identified desktop client to receive the exact seeded repo/lane IDs and send a nonempty viewport, while the complete repomond PID set stays unchanged.

The desktop's `REPOMON_SOCKET` and sandbox config `socket_path` point to `app.sock`. A recorder-owned observer forwards original framed bytes to `demo.sock`, where the seeded daemon is already running. Darwin's LOCAL_PEERPID identifies both peers; a different client or daemon fails verification. Fixture and mock-agent RPCs continue to use `demo.sock` directly and cannot satisfy the desktop check.

The observer writes method names, fleet IDs/counts, endpoint paths and peer PIDs to `data/logs/desktop-rpc.jsonl`. It excludes config payloads, tokens, terminal output and mail bodies. `out/launch.json` records the endpoint inputs and hashes of the supplied binaries.

The desktop receives a separate OS guard that denies execution of its sibling `repomond`. A subprocess probe proves that denial before launch. The existing real-home and outbound-IP restrictions remain in place.

The existing desktop binary does not write a native successful-connection log. Verification checks `data/logs/repomond.out.log`, which the native launcher creates if it tries to spawn a daemon, and reports whether it exists. The RPC observer log is recorder-generated evidence, not a native application log.

An investigation with the operator's identical binary hashes found the app connected to the seeded daemon and receiving all five repos/eight lanes, so a wrong endpoint was not reproduced. If these checks pass but the AX dump still lacks rows, retain both the RPC log and AX dump to investigate the renderer or accessibility tree separately.

## Diagnose a missing fleet without Accessibility

Run the guarded WebKit probe first. It uses the supplied app binary and compiles a small diagnostic library with Xcode command-line tools; it does not rebuild or bundle Repomon.

1. Run the storage and DOM check.

   ```sh
   scripts/record-gui-demo.sh --dry-run --diagnose-webview --keep-sandbox --bin-dir /Users/azaleas/Developer/Claude/repomon/target/release
   ```

2. Read `out/webview-check.json` in the printed sandbox. It must show a sandbox Cocoa home/Library, successful localStorage and IndexedDB writes/reads, eight rendered lane buttons and populated status chips.
3. Read `out/webview.jsonl` for JavaScript errors, rejected promises, console messages and button text/bounds. `out/sandbox-denials.json` contains the system log query; `out/sandbox-denials-status.json` records query errors and its exact predicate.
4. To compare the operator's AX failure, run the rehearsal once with the default guard, then repeat with `--no-guard`.

   ```sh
   scripts/record-gui-demo.sh --dry-run --tour --diagnose-webview --keep-sandbox --bin-dir /Users/azaleas/Developer/Claude/repomon/target/release
   scripts/record-gui-demo.sh --dry-run --tour --diagnose-webview --no-guard --keep-sandbox --bin-dir /Users/azaleas/Developer/Claude/repomon/target/release
   ```

The console probe reports both storage checks and all eight lane buttons. A successful DOM check establishes layout and text; the tour separately tests native accessibility. Compare both runs' `webview-check.json`, `desktop-connection.json`, `tour.log` and any `ax-dump.txt`.

`CFFIXED_USER_HOME` already redirects Cocoa's home to the disposable `cocoa` directory. The recorder prepares its Library/WebKit, Library/Containers, Library/Caches, Library/Preferences and Library/Application Support directories. The normal guard allows these paths because they are outside the denied personal home. The probe checks the app's actual `NSHomeDirectory` and Library results, resolving macOS's `/tmp` alias before comparing paths. No global home or preferences are changed.

The opt-in diagnostic library attaches a WKUserScript before app scripts run. A native WKScriptMessageHandler writes console events and DOM samples into `out/webview.jsonl`. Storage checks use temporary namespaced entries and delete them. The diagnostic variables are passed after `sandbox-exec` because macOS strips DYLD variables at protected system executables. No inspector port opens, and no product source or binary is patched. A supplied hardened binary may reject library loading; the 30-second check fails with the retained logs in that case.

`--no-guard` disables the desktop's entire OS guard for an explicit A/B comparison, including its real-home, outbound-IP and daemon-spawn denials. It retains the same disposable app configuration, Cocoa/XDG roots, endpoint observer, fake agents and PID checks. The daemon and mock agents stay guarded. Normal runs retain every deny rule. `out/launch.json` records which mode ran; an unguarded run does not claim that the desktop spawn-denial probe passed.

The system log query only selects sandbox/kernel messages mentioning this demo app PID or unique sandbox path. An empty query is not proof that no denials occurred: macOS may omit reports, and the log command can be unavailable under an outer sandbox. The status file makes this limitation explicit.

The operator's failing sandbox already contained saved theme and launch-count preferences under its private WebKit directory. In a guarded reproduction with the same supplied app, the console probe also found the full fleet and populated chips without a storage exception. This refutes storage starvation in that reproduction. It does not establish why the operator's native AX dump omitted those rows; the guarded/unguarded rehearsal supplies the next comparison.

## Rehearse after a lookup failure

Run the rehearsal from the operator's terminal before capturing again.

1. Run the updated recorder with retained diagnostics.

   ```sh
   scripts/record-gui-demo.sh --dry-run --tour --keep-sandbox --bin-dir /Users/azaleas/Developer/Claude/repomon/target/release
   ```

2. Check the printed sandbox path. `out/tour.log` records how long fleet readiness took, whether onboarding was skipped, and which AX fields or static-text child matched each button.
3. If a lookup fails, read `out/ax-dump.txt`. It lists every front-window AXButton, AXRadioButton and AXStaticText with role, description, name and value. The first 40 lines also appear in the terminal and `out/tour.log`.

The rehearsal log reports `Fleet ready after ... s`, a match for `nav-focus-trap`, and `PASS tour rehearsal completed without screen capture`.

Both GIF and still use the same opening routine. Before selecting a lane, it polls for an AXStaticText containing `orbit-api` for up to 30 seconds. If `Skip setup` appears during that wait, it presses that control in the isolated app and continues waiting for the fleet.

Onboarding completion and the resume step are browser-local preferences, not daemon TOML settings. The current app normally skips onboarding when the seeded fleet has repos; the explicit Skip setup handling also covers a wizard in the supplied binary. The recorder never edits real preferences.

Button and content lookups retry for 12 seconds. Button matching checks description, name and value independently. If WebKit exposes lane text only in a static child, the recorder follows AXParent to its enclosing button. It does not select an unrelated row merely because aria-current is set.

At the source revision used for this fix, LaneRow renders the title in a text span and the branch in a truncated span inside the button. The button has aria-current but no explicit aria-label. Source markup cannot prove how an existing WebKit binary exposes its AX name, so the runtime match log and failure dump supply that evidence. No product markup change or rebuild is required.

The GIF writes `docs/gui-demo.gif` at 1200x750; `--still` writes `docs/preview.png` at 1440x900.
Both use the same opening routine and crop the top-left 1440x900 content rectangle after
normalizing Retina scale. This removes the 86-pixel bottom band seen in a window-ID capture.
There is no full-display fallback. GIF encoding starts at 12 fps, retries 10 fps before reducing
the palette, and refuses to replace the GIF if the candidate is 15 MB or larger. Capture size
still needs measurement on the operator's terminal; no estimate is a verified recording size.

| Lane | Mock kind | Expected status |
|---|---|---|
| orbit-api / main | Codex | Running |
| orbit-api / feat/rate-limit-headers | Claude Code | Needs you |
| meadow-web / main | Cursor | Running |
| meadow-web / fix/nav-focus-trap | Claude Code | Idle, opening hero |
| forge-cli / main | aider | Running |
| forge-cli / fix/windows-console | OpenCode | Running |
| atlas-docs / main | Antigravity | Rate-limited |
| repomind / main | No controller process | Idle, pinned home |

All seven agents are terminal actors, including the Claude hero. They ignore input and flags,
never evaluate commands, and never invoke an agent CLI. The running actors redraw a progress
counter and the classifier's in-flight footer. The permission actor changes its question once,
producing two real supervision hold entries. Mail is sent by three actors using their own
sandbox identities; the token is never printed. Repomind has two active plan files and one
playbook draft. Its basic-memory executable is a no-op, so there is no external memory service.

Usage consists of 42 sessions and 105 events made from the repository's redacted Claude and
Codex fixtures. Dates span the current seven-day range; varied token multipliers produce a
non-flat chart. No actual usage is incurred. The dry run prints measured token and estimated
cost totals and writes lane, summary, sessions, timeline, Repomind and audit JSON to `out/`.

| Time | Beat |
|---|---|
| 0-8 s | Fleet and idle Claude hero |
| 8-16 s | Multitasking with four panes |
| 16-24 s | Git with the uncommitted navigation diff |
| 24-32 s | Full editor with TypeScript syntax |
| 32-38 s | File finder |
| 38-44 s | SVG image preview |
| 44-53 s | Usage: seven-day chart, cards and sessions |
| 53-61 s | Settings: Usage model rates |
| 61-68 s | Repomail between the fake lanes |
| 68-75 s | Supervision hold audit |
| 75-82 s | Repomind plans and playbook draft |
| 82-84 s | Shortcuts overlay |
| 84-90 s | Return to the opening hero |

The tour targets the demo process ID, uses semantic accessibility controls, and fails if a
required beat is missing. Numbered shortcuts match `apps/desktop/src/keymap.ts`: Git 1, Usage 3,
multitasking 5, Supervision 7, Repomail 8, Repomind 9. The full editor uses the Editor toolbar
button because Cmd+2 opens the compact rail editor. Fixed beat deadlines include interaction
time; a slow or missing control aborts the recording rather than silently skipping a scene.

Isolation is enforced by a unique daemon socket, a valid unique tmux label, **a private
`TMUX_TMPDIR`**, disposable XDG/Cocoa/config/data/usage roots, explicit `[repomind] home` and
`BASIC_MEMORY_CONFIG_DIR`, and a macOS sandbox profile denying real-home file access and all
outbound IP connections. Executables are copied into the sandbox so the home deny rule needs
no exception. The guard is tested before daemon launch. The opt-in `--no-guard` comparison disables only the desktop guard, as described above. No HOME or CODEX_HOME override is used.
The production daemon PID set is read before and after; its socket and database are never used.
Cleanup terminates tracked child PIDs and only the private tmux server. Git fixture commands
ignore the operator's global and system Git configuration.

Verification on 2026-09-06: `bash -n`, shellcheck, Python compilation, AppleScript compilation,
Swift type checking, and `--dry-run` passed. The sandbox reported 14,049,618 tokens and
$17.1129354 estimated cost across 42 sessions and 105 events. `--dry-run --tour` passed the
sandbox checks and app launch but macOS denied this runner assistive access (`-25211`). The
visual tour, opening-frame crop, and final GIF remain unverified until the operator rehearses
and captures from their terminal. The existing 50-second GIF is 1.124 MB, so duration-only
scaling suggests about 2.0 MB for 90 seconds; the extra views and motion can increase that.
The encoder's 15 MB check, rather than that estimate, decides whether to replace the asset.

Tour-fix verification on 2026-09-06: the capture-free dry run passed with the same fleet and usage totals above. Shell syntax, shellcheck, Python compilation and AppleScript compilation passed. Synthetic AX tests passed for a generic description with a meaningful name, static-text parent selection, exact and role matching, and a complete dump with a 40-line log preview. These tests use synthetic nodes; the live tour and capture still require the operator's permissions.

Webview-probe verification on 2026-09-06: guarded and desktop-unguarded dry runs both passed with the same supplied binary hashes. Each reported successful localStorage and IndexedDB round trips, eight rendered lane buttons, Needs you 2, Running 4, a 1440x900 webview and no captured JavaScript errors. The kernel peer checks passed and no second daemon appeared. The guarded system-log query returned ten provenance messages for sandbox daemon execution attempts, with no WebKit/Library denial reported; this is limited log evidence, not proof of zero denials. Eight negative/acceptance checker tests, shell syntax, shellcheck, Python/JavaScript syntax and Objective-C compilation passed. Native AX rehearsal remains for the operator.
