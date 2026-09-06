# Brief U4-fixes: review findings on fix/sidebar-usage-refresh

Same worktree `/private/tmp/repomon-fix-sidebar-usage`, same branch. The coordinator rebased it onto
main (now 38e4873, which merged the usage table pass and the Windows work; one test-file conflict
was resolved by keeping both test blocks), so the three commits are now 7f668d0, 2ca80b1, ffa8176.
Build on top, no history rewrite. One commit per item, 1-line Conventional Commits, no co-author
trailer, no merge/push/bundle. Rules as before.

1. **CRITICAL, `usage.refresh` blocks the connection.** The daemon dispatches one request at a
   time per connection (`crates/repomon-daemon/src/socket.rs`, the `in_rx.recv()` loop), and the
   desktop rides one shared client for everything: terminal keystrokes (`agent.send_input`), the
   1.2 s fleet poll, all views. A manual refresh that waits up to 15 s inside the handler
   (`rpc.rs` "usage.refresh" -> `usage_watch.rs::refresh_with_timeout`) therefore freezes input and
   every UI update for the whole wait, and the queued calls race their own 15 s CALL_TIMEOUT so the
   sidebar can show a spurious "lane.list timed out". Fix: make `usage.refresh` non-blocking again.
   It validates the gates synchronously and returns at once with `{refreshed: false, reason:
   "pending" | "probe_disabled" | "no_active_kind" | "cooldown", detail, snapshot}` (cooldown only
   when another manual round is already in flight), and the watcher broadcasts a new pubsub topic
   `event.usage.refreshed { reason: "ok" | "timeout" | "error", detail, snapshot }` when the round
   finishes or when 15 s pass without completion. The desktop subscribes (the fleet store already
   subscribes to daemon events) and holds the spinner until that event or a 20 s client-side
   ceiling, then re-snapshots and shows the inline notice. Update the bindings, `docs/protocol.md`,
   and the tests (each reason; a keystroke RPC issued during a refresh completes immediately: test
   at the socket level with a slow probe stub). Keep `usage.refresh` off the remote allowlist or
   amend the allowlist comment in `remote.rs` deliberately.
2. **CRITICAL, rotation starves the newest transcripts.** `usage_ingest.rs::rotated_window` only
   includes index 0 when the offset is 0 or the window wraps, so with more sources than
   `max_files_per_scan` the newest files are walked once every N passes (50 min on a 1000-source
   fleet). Fix: always walk the newest half of the budget, and rotate only the remaining half over
   the tail (`newest(budget/2)` union `rotated_window(&by_recency[budget/2..], budget/2, rotation)`).
   Test: 1000 sources, budget 200, the newest 10 are visited on every pass, and a changed file at
   index 900 is visited within 10 passes.
3. **`stale_sources` can never reach zero.** A failed recount rewrites the cursor at the old
   version (right for a transient error, wrong forever for a deleted file), `stale_batch` skips
   cursors with no discoverable path or unknown agent_kind without touching them, and Claude prunes
   transcripts after 30 days, so on a long-lived install the "Recounting N of M" line and the 3 s
   poll never stop. Fix: a stale cursor whose file no longer exists is stamped at INGEST_VERSION
   with its `error` kept and its events kept (or its cursor row deleted); a stale_batch entry that
   resolves to nothing is treated the same; `next_ingest_delay` gates the fast loop on attempts
   (`recount_attempts == REINGEST_BATCH && stale_remaining > 0`), not on successes. Tests: a
   deleted stale source leaves stale at zero after one pass; one unreadable source in a batch does
   not drop the loop back to the slow interval.
4. **"Probe timed out" while the probe succeeds.** `finish_round` fires only after every account is
   probed and each probe can take 5 to 12 s, so with two or more accounts the 15 s wait usually
   reports a timeout and then the numbers update anyway. With item 1's event shape, the client
   copy for a still-running round is "Still probing, this can take a moment" (no failure wording),
   and only an actual round error says so. Also consider emitting the event per completed account
   so the card updates progressively.

Gate as before (`cargo test -p repomon-core -p repomon-daemon -p repomon-tui`; `bun run check`,
`bun run test`, `bun run bindings:check`). Report commit hashes per item, tests, gate tails.
