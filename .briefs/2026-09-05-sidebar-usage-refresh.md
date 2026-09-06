# Brief U4: sidebar Rate Limits refresh, cost visibility toggle, re-ingest convergence

Run `/frontend-design` and `/impeccable` before touching UI and apply them throughout. Operator
observations from the live app on 2026-09-05 (screenshot: the sidebar "Rate limits (main)" card
with "just now", a refresh icon, "Today $318.9", 5-hour and weekly quota pills).

Branch: `fix/sidebar-usage-refresh` in a fresh worktree at `/private/tmp/repomon-fix-sidebar-usage`
from current main (5095fcb or later). This brief is the complete scope. Another agent is working in
parallel on Settings > Usage (brief U3: SettingsModal.tsx new tab, usage rates RPC, CLI); do not
edit SettingsModal.tsx or the rates code, and keep UsageView.tsx edits minimal so the merge is easy.

## 1. The refresh button does nothing visible

Diagnosis: `FleetSidebar.tsx` (the card near line 1077) calls `fleet.refreshUsage()`, which sends
`usage.refresh` and then re-snapshots the fleet. The daemon handler (`rpc.rs` "usage.refresh")
only does `ctx.usage_refresh.notify_one()` and returns `Null` at once, so the re-snapshot runs before
the watcher (`usage_watch.rs`) has probed anything, the spinner stops within milliseconds, and the
numbers, the "just now" age, and "Today $" all stay as they were. The watcher also silently skips
when `[usage_probe]` is off, when no active agent kind is eligible, or inside its five-minute
cooldown, and the user is never told.

Fix, daemon: `usage.refresh` performs (or waits for) one probe pass with a bounded timeout (about
15 s) and returns the outcome: `{ refreshed: bool, reason: "ok" | "probe_disabled" | "no_active_kind"
| "cooldown" | "timeout" | "error", detail?: string, snapshot }`, bypassing the cooldown for a manual
request; it also wakes the usage ingest (`ctx.usage_ingest_wake`) so cost-today is re-read on the
same click. ts-rs type, bindings regenerated, `docs/protocol.md`. Tests: each reason, and that a
manual refresh ignores the cooldown.

Fix, desktop: the button stays spinning until the RPC returns, then the card re-snapshots (quota,
age, cost). A non-ok reason shows a short inline line under the card header for a few seconds
("Usage probe is off in Settings", "No agent running to probe", "Probe timed out"), in the app's
existing inline-notice style, never a modal. Tests: ok path updates the age and cost, a
`probe_disabled` result shows the notice, the spinner is held until the promise resolves.

## 2. Toggle to show or hide the cost

Add a preference "Show today's cost in the sidebar" (default on). Control: a small icon button on
the card header next to the refresh icon (eye / eye-off drawn as SVG in icons.tsx, with tooltip
and aria-label), toggling the "Today $" row. Persist it in the desktop's local preferences store
(where other sidebar view preferences live; find the existing pattern, `workspace.ts` or similar,
never a new ad-hoc localStorage key). The Usage view is unaffected. Tests: toggling hides the row,
the preference survives a store reload, default is on.

## 3. Re-ingest convergence

`usage_ingest.rs::ingest_watch` sleeps `scan_interval_secs` (600) between passes and each pass
re-reads at most `REINGEST_BATCH` (25) stale sources, so after an `INGEST_VERSION` bump with 282
stale sources the totals take about two hours to become right (observed live today). Fix: when a
pass re-ingested a full batch and stale sources remain (expose that from `ingest_once`'s report),
sleep about one second before the next pass instead of the interval, until none remain; keep the
existing wake and the long interval otherwise. Broadcast `USAGE_CHANGED` after each such pass.
Desktop: while `usage.status.stale_sources > 0` the Usage view shows one muted line under the
header, "Recounting N of M transcripts, totals will settle shortly", and polls status every few
seconds until it reaches zero; then it refetches. Tests: the loop takes the short sleep only when a
full batch was re-ingested with stale left; the notice appears and disappears with the status.

## Rules and gate

Worktree only; never edit the main checkout; never bind a daemon to `/tmp/repomon-azaleas.sock`
or copy the production database; never kill processes by name pattern; never `git add -A`; tests
use fixtures and mocked RPCs. Zero hex colors (CSS variables), no emoji (SVG icons), no em-dashes
in code, comments, or copy; async results token guarded. Gate: `cargo test -p repomon-core -p
repomon-daemon -p repomon-tui`; in `apps/desktop` `bun run check`, `bun run test`, `bun run
bindings:check`. Commits: 1-line Conventional Commits per numbered item, no co-author trailer. Do
not merge, push, or build the bundle. Report commit hashes, per-item summary, test names, gate
tails, and what could not be verified without a screenshot.

## 3b. Evidence added 2026-09-05 18:05 (coordinator, live daemon)

Calling `usage.ingest_now` twenty times in a row took `stale_sources` from 282 to 234 and then
left it at 234 for every further pass (each pass: scanned 5 to 11, events 4 to 35). Cause: a pass
walks at most `max_files_per_scan` (200) sources, newest first, so a stale source outside the
newest 200 is never visited and never re-ingested. Fix as part of item 3: stale sources are
selected from the cursor table directly (oldest ingest_version first, then newest mtime), up to
`REINGEST_BATCH` per pass, independent of the newest-200 walk; and the newest-first walk should
rotate (continue from where the previous pass stopped) so a changed old file is eventually seen.
Test: 300 sources with 250 stale converge to zero within 10 passes.
