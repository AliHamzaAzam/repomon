# Repomon 0.6.0 status

Snapshot: 2026-08-13, branch `main`

Repomon 0.6.0 is at the desktop preview gate. The verified feature train, desktop sound,
durable fleet messaging, and the OpenCode and Antigravity backend work are merged on `main`.
The desktop preview is the next distribution point. No stable CLI tag is part of this release
task.

## Landed train

- The 27-commit verification train landed in merge `f1ec3c7`.
- Identity binding and immediate spawn input landed from `fix/spawn-agent-talk-flow`.
- Live untracked-file lane diffs, the complete CLI lane controls, and the remote lane-diff
  protocol correction landed.
- `fix/agent-identity-desync` remains deferred as a superseded subset.
- `repomind` remains deferred as the older predecessor to the implementation on the train.
- Mission Control was exercised against an isolated daemon and tmux fleet, including repomind,
  playbooks, approvals, standing orchestrations, notification fallback, layout modes,
  extensions, reconnect, and terminal input.

## Desktop sound

- `notify_sound` remains the master switch and daemon fallback policy.
- Defaults are master on, volume `0.25`, unfocused-only on, and all six cue toggles on.
- The six Web Audio cues cover agent attention, agent completion, repomind attention, error or
  stall, incoming mail, and an available update.
- Native notifications are silent whenever custom audio is scheduled or policy suppresses a cue.
  One system sound is allowed only when an otherwise eligible custom cue cannot be scheduled.
- Unit and isolated live coverage includes event mapping, focus policy, dedupe, sound arbitration,
  all six previews, volume, and master and per-cue suppression.

## Fleet messaging

- Canonical lane, slot, label, repomind, and operator addressing is implemented.
- Messages and MCP identities are durable. Only MCP identity hashes are stored.
- Managed agents receive a restricted MCP surface for fleet status and mail. Message RPCs remain
  local-socket-only.
- Body size, sender rate, thread hop, injection policy, idle delivery, read state, and dedupe
  guardrails are enforced.
- CLI, TUI, and desktop Control Center mail surfaces are implemented with lane badges, jump and
  read actions, native notifications, and the incoming-message cue.
- Isolated Claude and Codex exchange, plus OpenCode and Claude exchange, proved framed injection,
  reply routing, and durable thread continuity.

## Agent backends

| Backend | Spawn | Mail MCP | Attention | Exact resume | Repomind |
|---|---:|---:|---:|---:|---:|
| Claude Code | Yes | Yes | Rich transcript and pane state | Yes | Yes |
| Codex | Yes | Yes | Managed pane state | Yes | Yes |
| OpenCode 1.15.5 | Yes | Yes, no approval | Read-only SQLite waiting and end-of-turn | `--session` proven | Omitted |
| Antigravity 1.1.12 | Yes | Global registration required | Pane permission proven | `--conversation` builder | Omitted |

OpenCode is omitted from repomind because an orchestrator session cannot yet be pinned to a safe
attention signal. Antigravity is omitted because its isolated qualification hit trust and
permission prompts before the repomon MCP could run without approval. Usage is degraded for both
backends because neither supplied stable quota percentages and reset data compatible with the
existing usage model.

## Verification

- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass.
- `cargo fmt --all --check`: pass.
- `cargo test --workspace --locked`: pass.
  - `repomon-core`: 279 unit tests plus 2 reconnect integration tests.
  - `repomon-daemon`: 125 passed with 2 live usage probes ignored.
  - Daemon integration: 19 passed.
  - MCP stdio: 12 passed.
  - `repomon-mcp`: 37 passed.
  - `repomon-tui`: 95 unit tests plus 5 integration tests.
  - All remaining orchestrator, remote, standing, host, and documentation suites passed.
- Desktop bindings and TypeScript checks: pass.
- Desktop tests: 161 passed across 25 files.
- Desktop production build: pass.
- A 20,000-line ANSI terminal sweep preserved the final marker and `λ中✓` glyphs after resize in
  both DOM and WebGL renderers.
- Windows remains a required CI platform for the workspace and desktop preview workflows.

## Remaining release and founder actions

- Publish and verify `desktop-v0.6.0-preview.1`, including macOS, Linux, and Windows artifacts and
  the signed `latest.json` updater manifest.
- Perform physical Windows 11 end-to-end validation.
- Add Authenticode signing for the Windows executables and installer.
- Decide whether the daemon and CLI changes justify a later stable `v0.6.0` tag. This task does
  not create that tag.
