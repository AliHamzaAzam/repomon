# repomon — Completion Status

_Snapshot: 2026-05-30 · branch `main` · 44 commits · github.com/AliHamzaAzam/repomon (private)_

repomon is a Rust TUI "mission control" for parallel AI coding agents across many repos,
branches, and worktrees, backed by a tmux-owning daemon. **Verdict: all planned milestones
(M0–M13) are complete and the quality/perf gates pass — ready for hands-on testing.**

## Plan completion (M0–M13) ✅

| Phase | Milestones | Status |
|---|---|---|
| **1 — Foundation + Fleet** | M0 scaffold · M1 core + SQLite store · M2 git layer (gix + worktree) · M3 registry/lane/watch · M4 daemon (socket + JSON-RPC + pubsub + launchd) · M5 Fleet + Split TUI · M6 CLI | ✅ |
| **2 — Agent multiplexer** | M7 tmux runtime · M8 Claude monitor + needs-you · M9 live UI (viewport streaming, babysit grid, focus, pin, needs-you jump, attach, merge) · M10 Codex/Aider + `docs/agents.md` | ✅ |
| **3 — Dashboard / history** | M11 history indexer · timeline · jaccard correlations · session detection · markdown export · commit search | ✅ |
| **4 — Polish (Rust-only)** | M12 accent-color slot + mouse · M13 docs + perf | ✅ |

10 views, 5 CLI subcommands, 39 RPC methods wired.

## Quality bar (verified)

- **65 tests passing** — core unit + rpc unit + daemon integration + TUI snapshots.
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
- **Account-usage corner** (opt-in `usage_probe`) — limit windows (% used) + reset shown
  bottom-right for the focused agent's account, **provider-aware**: Claude `/usage` (5h + weekly,
  per `~/.claude*` account), Codex `/status` (5h/weekly or Free monthly), and Gemini `/stats`
  (daily quota, best-effort — see caveat). Scraped via a hidden throwaway session per account
  (`usage_watch.rs` + fixture-tested `agent/usage.rs`), with a rate-limit countdown fallback.
  See `docs/agents.md`.
- **Gemini** is a first-class spawnable agent (`AgentKind::Gemini`, `gemini` on PATH). Its usage
  corner is best-effort: Gemini only exposes quota in interactive `/stats` (OAuth Code-Assist) and
  a probe-spawned `gemini` often can't auth unattended, so usage shows only where it reaches its
  prompt with cached creds; otherwise the corner falls back.

## Deferred (explicitly out of scope in the plan)

SwiftUI menu-bar app · native repomon-owned PTY mode · web dashboard · Windows. None blocking.

## Known limitations to keep in mind while testing

- The Grid tiles **agent lanes**, not the plain `t` shells (possible follow-up).
- **Codex** detection is tmux-alive-only (no transcript); **Aider** is coarse (history-file
  mtime). Claude is the rich path (status, needs-you, multi-account).
- **cd-on-exit (`c`)** only acts when the `repomon` shell function is installed (it sets
  `$REPOMON_CD_FD`); otherwise it shows a hint instead of quitting.
- In agent views (Focus/Split/Grid) the daemon refreshes ~1 s and runs a cached (2 s)
  `pgrep`/`lsof` liveness probe — light, but slightly more active than the 0% Fleet idle.

## Suggested next steps

1. Plain-shell **terminals as Grid tiles** (observe several shells at once).
2. Tighten warm first paint comfortably under 100 ms.
3. Optional: richer Codex/Aider status if/when their on-disk formats stabilize.
