# Brief A1: Full code audit (Codex)

Context: between 2026-09-03 and 2026-09-05 the repo absorbed a large amount of agent-written
work (editor phases 1 to 3, PDF and image viewers, sidebar and status rework, Repomind Home
R0 to R6, usage ledger and its fixes, onboarding v2, shortcuts guide, Windows boot work). Each
piece passed its gate, but nobody has looked across the whole. This audit is read-first: produce
a report, agree the fix list with the coordinator, then fix in bounded rounds.

Branch: `chore/audit-2026-09` in a fresh worktree at `/private/tmp/repomon-audit` from current
main. Never edit the main checkout; never bind a daemon to `/tmp/repomon-azaleas.sock` or copy
the production database; never kill processes by name pattern; never `git add -A`.

## Phase 1: report only (no code changes), written to `qa/audit-report.md` in the worktree

1. **Dead code and duplication.** Rust: `cargo clippy --workspace --all-targets -- -W dead_code
   -W unused` plus a manual pass for `pub` items with no callers (`cargo +nightly udeps` is
   allowed if installed; otherwise reason from `grep`). TypeScript: run `knip` or `ts-prune`
   via `bunx` in `apps/desktop` for unused exports and files. List duplicated helpers (known
   examples: `chordFor` exists in App.tsx, ControlCenter.tsx, and Onboarding.tsx; several
   `formatX` helpers across usage and settings; two "isolated config" test helpers), and
   modules that grew past 1,500 lines (rpc.rs, fleet.ts, EditorWorkspace.tsx, UsageView.tsx)
   with a suggested split.
2. **Comments.** The codebase has many long doc comments written by agents. Flag comments that
   are wrong (describe behavior that changed), stale (reference removed items: the
   `orchestrator` window, `Automation` settings, old chord numbers, `file.read_raw` for
   images), redundant (restate the code), or overlong (more than about six lines for a private
   fn). Do not rewrite yet; list file:line with a one-line verdict. Keep comments that explain a
   non-obvious why.
3. **Performance.** Daemon: the notify/status tick, pane capture cadence and line counts,
   `lane.list` payload size, the usage ingest and re-digest bounds, SQLite indices versus the
   queries in `usage_query.rs` and the store (run `EXPLAIN QUERY PLAN` on the heavy ones
   against a copy of a fixture database, not the production one), lock hold times in
   `Ctx` (`repomind_lock`, `rate_limits`, `lane_watchers`). Desktop: the 1.2 s fleet poll,
   memo and effect churn in fleet.ts and TerminalWorkspace.tsx, the Usage view's per-render
   work (stacking, tables), the editor's gutter debounce, bundle size (`bun run build`
   chunk report; pdf.js worker is expected). Report measurements, not guesses.
4. **Correctness risks.** Places where two sources of truth can drift (usage_daily versus
   usage_events, session digests versus events, playbooks files versus any remaining store
   rows, keymap registry versus local bindings), error paths that swallow errors (`let _ =`
   on writes, `unwrap_or_default` hiding failures), and platform branches (`cfg(windows)`)
   that lack tests.
5. **Tests.** Flaky or timing-based tests (`transport::reclaims_a_stale_socket_file_with_no_live_listener`,
   `lineDiff` timing, `TerminalWorkspace.multitasking` waitFor), tests that assert
   implementation details rather than behavior, missing coverage for the risks in item 4.
6. **Hygiene.** `cargo fmt --all --check` drift (known), clippy warning count and the top
   categories, `bun run check` strictness, em-dashes and emoji in code or docs (both banned),
   hex color literals in the frontend (banned), `TODO`/`FIXME` inventory, docs sections that
   describe removed behavior.

Deliver the report with a ranked fix list: each item has severity, effort (S/M/L), files, and
whether it is safe to do without behavior change. Stop and report to the coordinator before
changing anything.

## Phase 2: fix rounds (after the list is agreed)

One branch per round from current main, one round per area (dead code, comments, performance,
tests, hygiene), each with its own gate: `cargo test --workspace`, `cargo clippy --workspace
--all-targets`, `cargo fmt --all --check` for touched files; `bun run check`, `bun run test`,
`bun run bindings:check` in `apps/desktop`. Behavior-preserving changes only unless the fix
list says otherwise; every performance change carries a before/after measurement in the commit
message body is NOT allowed (1-line commits), so put measurements in the report and the PR
notes file `qa/audit-round-N.md`. Commits: 1-line Conventional Commits (`refactor: ...`,
`perf: ...`, `docs: ...`, `test: ...`), no AI co-author trailer. Do not merge or push; the
coordinator reviews and merges each round.

## Operator's framing (2026-09-06), apply to every file you touch and to the report

Think from first principles about what we're trying to achieve here. Interrogate what was built
before calling it done:

1. Is anything here unnecessary, overly complicated, or based on weak assumptions? Challenge them.
2. What can be deleted entirely?
3. What can be simplified now that unnecessary pieces are gone?

Then make the changes. Prefer deleting over simplifying, simplifying over optimizing, and
optimizing over automating. It might be done too: you do not HAVE to go and make changes; if it
is good, leave it alone.

Scope note: main is at 0.9.0 (version bumped, unpublished). Since the brief was written, main
also absorbed: the Windows boot/CLI-install work, the model rates settings, the sidebar refresh
and recount convergence, the usage table pass, the brand theme and responsive pass (D1), and the
Windows system check. Two known follow-ups to fold into the fix rounds: `fail_usage_recount`
should count strikes per minute, not per pass (60 s guard on `scanned_at`); `cargo fmt --all
--check` and one CI-only flaky frontend test (`TerminalPane clickable path links`) make main's
CI red. Worktree for Phase 1: `/private/tmp/repomon-audit` on `chore/audit-2026-09`.
