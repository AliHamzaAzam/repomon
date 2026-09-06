# GUI showcase recorder

Run from this checkout on macOS using existing, matching release binaries. The recorder never
builds or bundles the app. A fresh worktree normally has no `target/release`, so supply the
binary directory explicitly:

```sh
scripts/record-gui-demo.sh --dry-run --bin-dir /Users/azaleas/Developer/Claude/repomon/target/release
scripts/record-gui-demo.sh --dry-run --tour --bin-dir /Users/azaleas/Developer/Claude/repomon/target/release
scripts/record-gui-demo.sh --still --bin-dir /Users/azaleas/Developer/Claude/repomon/target/release
scripts/record-gui-demo.sh --bin-dir /Users/azaleas/Developer/Claude/repomon/target/release
```

The first command seeds, checks, launches the app for five seconds, then cleans up. The second
rehearses the actual AppleScript tour with no recording. The last two require Screen Recording
permission for the invoking terminal; the rehearsal and captures also require Accessibility
and Automation access to System Events. Run these from the operator's terminal. `--keep-sandbox`
retains fixtures and `out/*.json` evidence after stopping the demo app, daemon and tmux server.
`--skip-build` remains accepted for older commands; all runs already skip builds.

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
no exception. The guard is tested before daemon launch. No HOME or CODEX_HOME override is used.
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
