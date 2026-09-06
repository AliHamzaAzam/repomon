# Brief U4-polish: refresh and recount follow-ups, cost toggle moves to Settings

Branch `fix/usage-refresh-polish` from current main (2ab77d7 or later) in a fresh worktree
`/private/tmp/repomon-fix-usage-refresh-polish`. Rules as always (worktree only, no production
socket/DB, no kill by name, no `git add -A`, no hex/emoji/em-dashes, 1-line Conventional Commits,
no co-author trailer, no merge/push/bundle). One commit per item.

1. `usage_ingest.rs`: a stale source that exists but fails every read (permission denied, path
   replaced by a directory) is never retired and pins `stale_sources` at 1, leaving the Usage
   view's "Recounting" line and 3 s poll up forever. Retire it after 3 consecutive failed recount
   attempts, keeping its events and error. Test.
2. `usage_watch.rs`: `usage_refresh_inflight` is cleared only by `finish_round`, so if the watcher
   task dies `usage.refresh` answers `cooldown` forever. The deadline task clears it after a hard
   ceiling of `PROBE_TIMEOUT` times the account count. Test.
3. `finish_round` maps every non-Ok outcome to "Usage probe failed; try again"; `NoActiveKind` and
   `Timeout` carry their own reason and wording. Test.
4. **Operator request (2026-09-05, screenshot of the card): the cost visibility toggle lives in
   Settings, not on the card.** Remove the eye icon from the sidebar Rate limits card header (keep
   the refresh icon and the age). Add "Show today's cost in the sidebar" as a switch in
   Settings > Usage (`UsageSettingsView.tsx`), placed with the other usage toggles, using the same
   Switch control the tab already uses, reading and writing the existing uiSettings preference
   (default on). Update the FleetSidebar tests that clicked the icon to flip the preference
   instead, add a UsageSettingsView test for the switch, and update `docs/desktop.md`.

Gate: `cargo test -p repomon-core -p repomon-daemon -p repomon-tui`; in `apps/desktop` `bun run
check`, `bun run test`, `bun run bindings:check`. Mail the operator with commit hashes per item,
tests, and gate tails.
