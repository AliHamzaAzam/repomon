# repomon — Completion Status

_Snapshot: 2026-07-06 · branch `main` · 248 commits · github.com/AliHamzaAzam/repomon (public)_

repomon is a Rust TUI for running a fleet of AI coding agents across many repos,
branches, and worktrees, backed by a session-owning daemon (tmux on macOS/Linux, per-agent
host processes on Windows). **Verdict: all planned milestones (M0–M13) are complete and the
quality/perf gates pass — ready for hands-on testing.**

**Native Windows support has landed** (branch `release/windows-preview`, size-M port across
Tracks A–I). It is **code-complete and CI-green** on `x86_64-pc-windows-msvc`: the workspace
builds, `cargo fmt`/clippy are clean, and the test suite passes on the `windows-latest` CI leg
(tmux-only tests self-skip; the new host and backend integration tests run). The design keeps
the JSON-RPC wire protocol frozen (iOS-safe) and full durability parity: a `SessionBackend`
trait with a tmux backend on Unix and a host-process backend on Windows, named-pipe IPC, and
`repomon-agent-host.exe` (ConPTY + server-side vt100) whose detached hosts survive daemon
restarts and are re-adopted on start. **Two gates remain before a Windows release is tagged:**
a physical Windows 11 end-to-end pass ([docs/windows-validation.md](docs/windows-validation.md))
and binary signing (unsigned binaries trip SmartScreen). There is a preview build on
`release/windows-preview` and **no published Windows GitHub release yet**.

## Plan completion (M0–M13) ✅

| Phase | Milestones | Status |
|---|---|---|
| **1 — Foundation + Fleet** | M0 scaffold · M1 core + SQLite store · M2 git layer (gix + worktree) · M3 registry/lane/watch · M4 daemon (socket + JSON-RPC + pubsub + launchd) · M5 Fleet + Split TUI · M6 CLI | ✅ |
| **2 — Agent multiplexer** | M7 tmux runtime · M8 Claude monitor + needs-you · M9 live UI (viewport streaming, babysit grid, focus, pin, needs-you jump, attach, merge) · M10 Codex/Aider + `docs/agents.md` | ✅ |
| **3 — Dashboard / history** | M11 history indexer · timeline · jaccard correlations · session detection · markdown export · commit search | ✅ |
| **4 — Polish (Rust-only)** | M12 accent-color slot + mouse · M13 docs + perf | ✅ |

10 views, 5 CLI subcommands, 40 RPC methods wired.

## Quality bar (verified)

- **288 tests passing** (macOS; the Linux CI leg adds Linux-only ones) — core unit + rpc unit +
  daemon integration (incl. MCP stdio e2e + orchestrator lifecycle) + TUI snapshots.
- **clippy `-D warnings` clean**, `cargo fmt` clean.
- **Perf gates met:**
  - daemon cold start **108 ms** (target < 500)
  - idle daemon CPU **0.0%** (target < 1%)
  - warm first paint **~103 ms** (target < 100 — essentially at the line; includes process spawn)
- Docs: README, `docs/architecture.md`, `docs/protocol.md`, `docs/agents.md`.

## Beyond the original plan (added during the build, all working)

- Single `repomon` command (auto-starts a detached daemon, falls back to in-process).
- Interactive repo browser (`fs.browse`) to add repos without leaving the TUI.
- Live key passthrough with **insert / command modes** + ANSI colors; `Esc`, `Shift+Tab`,
  `Ctrl-C` all forwarded; leave insert with `^O`. In-place interaction in Split; `→` drills in.
- **Agent manager** — add / edit / delete custom launch commands and set a default.
- **Multi-account Claude autodetect** — `~/.claude` + any `~/.claude-*` (e.g. `claude-work`
  via `CLAUDE_CONFIG_DIR`); detection and adopt are account-aware.
- **External-session detection + adopt** — sessions running in other terminals show as `·ext`;
  adopt resumes the exact session (`claude --resume <id>`) with the right account/flags.
- **Multiple concurrent sessions per worktree**, filtered to live processes (so `/exit`ed
  sessions drop off); Focus auto-exits when its agent ends.
- **Plain terminals** per worktree (`t`), multiple allowed.
- Revamped **Grid** (real navigation, Instagram-style position dots, clean exit).
- repomon's tmux runs on a **dedicated socket**, isolated from the user's own tmux.
- **Notifications hardened** — desktop popups land faster (daemon edge-detector tick 8s→2s),
  attach-return no longer replays a duplicate backlog (re-seed instead of re-diff), and a new
  `notify_subagents` toggle (default off) suppresses alerts when worktree-isolated *subagents*
  finish — you're alerted only when the *main* agent finishes.
- **Expand agent rows** (opt-in `expand_agents`) — a lane running several agents can render as a
  tree in the sidebar (header + one row per agent, each with a 1-4 word summary + status) instead
  of an `×N` badge; sub-rows are individually selectable. Press `R` to rename an agent; the label
  persists in the daemon keyed by transcript id (`session.rename` RPC, `session_labels` table).
- **Account-usage corner** (opt-in `usage_probe`) — limit windows (% used) + reset shown
  bottom-right for the focused agent's account, **provider-aware**: Claude `/usage` (5h + weekly,
  per `~/.claude*` account) and Codex `/status` (5h/weekly or Free monthly). Scraped via a hidden
  throwaway session per account (`usage_watch.rs` + fixture-tested `agent/usage.rs`), with a
  rate-limit countdown fallback. See `docs/agents.md`.
- **repomind** — the MCP-driven fleet orchestrator (`repomon orchestrate` + TUI command-center)
  with a switchable backend (Claude or Codex). Shipped in v0.3.0.
- **Full Linux support** — systemd user service (`repomon daemon install`), notify-send
  notifications with sound, wl-copy/xclip clipboard (OSC52 fallback in tmux), image paste via
  wl-paste/xclip, and a /proc-based liveness probe. CI runs the suite on macOS + Ubuntu;
  releases ship x86_64 and aarch64 Linux binaries.

## Native Windows port (branch `release/windows-preview`)

Code-complete, CI-green, awaiting a physical E2E pass and signing. What landed:

- **Track A, IPC transport + portability.** A `transport` abstraction (Unix socket ⇄ named
  pipe); pipe-name socket default, `USERNAME`/`getrandom`/`$HOME` and PATHEXT-aware lookup,
  detached daemon spawn, and a `windows-latest` CI leg.
- **Track B, `SessionBackend` extraction.** The ~25-method trait lifted from `TmuxRuntime`
  (the single tmux choke point); `Ctx.backend: Arc<dyn SessionBackend>`; `SpawnSpec` replaces
  shell strings; the FIFO byte stream folded into `open_byte_stream`. Zero behavior change on
  Unix.
- **Track C, `repomon-host` crate.** `repomon-agent-host.exe`: ConPTY + server-side `vt100`
  with 50k scrollback + a named-pipe control server, against a frozen
  [PROTOCOL.md](crates/repomon-host/PROTOCOL.md).
- **Track I, `WindowsBackend`.** Host spawning, registry scan + `hello` verification + stale
  GC, **re-adoption on daemon start** (durability parity), and a Windows liveness arm that asks
  the hosts instead of `ps`/`lsof`.
- **Tracks D1–D4, platform services.** `Set-Clipboard`/`Get-Clipboard` + image paste, WinRT
  toasts, a Task Scheduler service arm, and PowerShell `shell-init` with a `REPOMON_CD_FILE`
  temp-file cd-on-exit.
- **Track E + follow-up, packaging.** `install.ps1` and a `windows-latest` release job; the
  `x86_64-pc-windows-msvc` leg is now **required**, aarch64 stays best-effort.
- **Track F, attach.** `repomon attach-host` raw byte proxy + the embedded focus view, popped
  out into a Windows Terminal tab.

## Cutting a Windows release

The Windows binaries are built and packaged by `.github/workflows/release.yml`:

- **Trigger.** A `v*` tag runs the whole release (macOS, Linux, Windows). A
  `workflow_dispatch` on any branch runs **only** `build-windows`, so the Windows packaging can
  be validated before a real tag (it stamps a `0.0.0-<sha>` dev version).
- **Artifacts.** `build-windows` produces `repomon-<version>-<target>.zip` (containing
  `repomon.exe` + `repomond.exe` + `repomon-agent-host.exe`) and a matching `.zip.sha256`,
  uploaded per target.
- **Targets.** `x86_64-pc-windows-msvc` is **required** (the port has landed); the
  `aarch64-pc-windows-msvc` leg is **best-effort** (`experimental: true`, `continue-on-error`),
  and the release step uses `nullglob` so a release without the ARM64 zip still succeeds.
- **Publish.** The tag-gated `release` job attaches every `*.zip`/`*.zip.sha256` (alongside the
  macOS/Linux tarballs) plus `install.sh` and `install.ps1` to the GitHub release.
- **TODO before shipping.** Binary **code-signing** is not wired yet; unsigned binaries trip
  SmartScreen on first run. Sign the three exes before or as part of the first Windows release.
  (Do not edit `release.yml` for docs work; this note only describes the existing flow.)

## Deferred (explicitly out of scope in the plan)

SwiftUI menu-bar app · native repomon-owned PTY mode · web dashboard. None blocking.

## Known limitations to keep in mind while testing

- The Grid tiles **agent lanes**, not the plain `t` shells (possible follow-up).
- **Codex** detection is tmux-alive-only (no transcript); **Aider** is coarse (history-file
  mtime). Claude is the rich path (status, needs-you, multi-account).
- **cd-on-exit (`c`)** only acts when the `repomon` shell function is installed (it sets
  `$REPOMON_CD_FD`); otherwise it shows a hint instead of quitting.
- In agent views (Focus/Split/Grid) the daemon refreshes ~1 s and runs a cached liveness
  probe (`ps`+`lsof` on macOS, `/proc` on Linux; on Windows the hosts report child liveness
  directly, no scan); light, but slightly more active than the 0% Fleet idle.

## Suggested next steps

1. Plain-shell **terminals as Grid tiles** (observe several shells at once).
2. Tighten warm first paint comfortably under 100 ms.
3. Optional: richer Codex/Aider status if/when their on-disk formats stabilize.
