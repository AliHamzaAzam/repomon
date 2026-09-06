# Brief R4: Repomind sidebar row, toolbar indicator, and panel

Spec: `docs/superpowers/specs/2026-09-04-repomind-home-design.md`, sections 2, 2b, 4, 5 and
phase R4. R1 to R3 are merged: the controller lane (`Lane.role == "controller"`),
`repomind.status` (home, lane, window, export state, counts, boot), `repomind.boot`,
`repomind.export`, file-first notes and playbooks, the boot document at `.repomind/boot.md`.

Branch: `feat/repomind-panel` in a fresh worktree at `/private/tmp/repomon-feat-repomind-r4`
from current main. This is frontend work in `apps/desktop`; run `/frontend-design` and
`/impeccable` before touching UI and follow them.

## Scope

1. **Hide the home from repo groups.** In the fleet store and sidebar, a repo whose only lane has
   `role == "controller"` is excluded from the repo groups, the lane counts in headers, and the
   "Needs you"/"Running" chip counts (its agents are counted by the pinned row instead).
   Multitasking's picker and Supervision still list its agents. Tests.
2. **Pinned Repomind row** at the top of the sidebar, above the repo groups, first stop of arrow
   navigation. Content: brain icon (SVG in `icons.tsx`, no emoji), "Repomind", a state pill
   using the fleet's one-word vocabulary (OFF when no controller window is live, else the most
   urgent controller state), controller agent count, active goals count (`repomind.status.counts.active_plans`),
   and the needs-you pip. Click focuses the controller lane's agents in the terminal bay like
   any lane. Right-click menu: Start Repomind, Stop, Open panel, Open home in editor (opens the
   center Editor mode on the home lane). Tests for the row's content, the OFF state, the
   context-menu actions, keyboard order.
3. **Toolbar indicator**: the existing "Repomind" toolbar button (`mod+5`) gets a small state dot
   like Repomail's badge: signal color when a controller is running, attention color when one
   needs you, none when off. Tests.
4. **Panel re-based on the controller lane** (`RepomindPanel` or whatever the current panel
   component is; keep its chat/transcript view for the primary controller): header with state,
   controller count, Start/Stop/Spawn controller buttons; sections for Active plans (from
   `plans/active/*.md`, title and next step, click opens the file in the editor), Journal tail
   (today's `journal/YYYY-MM-DD.md`, last entries), Boot context (generated_at, tokens,
   trimmed list, a "Regenerate" button calling `repomind.boot`, "Open boot.md" in the editor),
   Export state (last run, pending, error, "Export now" calling `repomind.export`), and the
   controller agents list with their status pills and reasons. Reading plan and journal files
   goes through the existing `file.read` on the home lane (it is a normal lane), no new RPC.
   `repomind.status` is polled with the fleet heartbeat and refreshed on `event.agent.status`
   for controller windows. Tests with mocked `daemonCall`.
5. **Multitasking and Supervision**: controller agents appear in the Multitasking picker under
   a "Repomind" group and in Supervision like any agent; the supervision policy default for the
   controller lane is `hold` on destructive classes (set via the existing per-lane policy
   store when the lane is created by ensure-home, or documented as the operator's step if the
   daemon cannot set it without a policy write; prefer the daemon path with a small
   `repomind.rs` addition and a test).
6. **Docs**: `docs/desktop.md` Repomind section rewritten for the row, the indicator, and the
   panel; keyboard notes; no emoji, no em-dashes.

## Rules and gate

Work only in the worktree; never edit the main checkout; never bind a daemon to
`/tmp/repomon-azaleas.sock` or copy the production database; never kill processes by name
pattern; never `git add -A`; tests use tempdir homes and mocked RPCs, never the operator's real
`~/repomind`. Zero hex color literals (CSS variables only), no `<Show keyed>` around anything
expensive, async results token guarded, no emoji, no em-dashes. Gate: `bun run check`,
`bun run test`, `bun run bindings:check` in `apps/desktop`; `cargo test -p repomon-daemon` if
the daemon changes (item 5). Screenshots are best-effort. Commits: 1-line Conventional Commits
per numbered item where sensible (`feat(desktop): ...`), no co-author trailer. Do not merge,
push, or build the bundle. Report commit hashes, gate tails, per-item summary, and anything left
out with the reason.
