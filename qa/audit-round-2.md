# Audit Round 2: delete unused work

Date: 2026-09-06. Branch: `chore/audit-r2-delete`.
Base: `23fe89e9c302442e2f30369d5dd38a3b1cec3e2a`, the accepted Round 1 merge on main.
Worktree: `/private/tmp/repomon-audit-r2`.
Scope: accepted audit items 9, 12, and 13. No schema deletion or replacement cache.

## Changes and commits

| Item | Commits | Result |
|---|---|---|
| 9 | `350c071` | Removed daily rollup maintenance, `UsageDailyRow`, `rollup`, `day_key`, and the unused daily reader. Event insertion counts changed rows directly; source deletion uses SQLite's deleted-row count. Preserved event identity, elementwise maximum token updates, and atomic source publication. |
| 12 | `607d5fc`, `73b20c2` | Removed legacy SQL playbook save/search/approve/delete methods, their private reader, and obsolete CRUD fixtures. Preserved `list_playbooks`, expiry policy, row decoding, and `repomind::migrate_records`. Migration fixtures seed historical SQL rows directly and use an isolated database, config, notes, home, memory config, and backend namespace. |
| 13 | `66a8caa` | Deleted the uncalled `TmuxRuntime::attach_args` and `usage_ledger::scan::require_readable`, including their stale comments and the scan module's unused `Error` import. |

Final caller searches found no references to the removed APIs. The only remaining `usage_daily` references in product source are its original table and index declarations. All shipped migration files are byte-for-byte unchanged from the base. Historical rollup rows and the table remain in existing databases, but this release no longer maintains them. Runtime usage APIs continue reading the event ledger. SQL playbook rows remain a migration input; migration never overwrites an existing destination file.

No generated contract, locally used export, worktree helper, or unrelated cache was deleted. These public Rust API deletions are scoped to verified repository consumers, as agreed in the report. Six touched Rust files were formatted to satisfy the gate, including existing formatting drift in those files. Broad formatting remains for Round 3.

## Regression coverage

The full workspace gate includes these relevant checks:

- `recording_the_same_source_offset_twice_inserts_once`: replay keeps one event.
- `a_message_re_read_keeps_the_elementwise_maximum_counts`: a later higher count replaces the partial count; a subsequent lower count changes nothing.
- `dropping_a_sources_events_returns_the_deleted_count`: source removal returns one, then zero, and leaves no events.
- `source_replacement_rolls_back_events_digests_and_cursor_then_recovers_after_reopen`: insertion/cursor failures retain events and session metadata, then retry succeeds after reopening. Only assertions about the deleted rollup were removed.
- `recount_failures_survive_reopen_and_success_resets_the_streak` and `a_source_an_older_reader_wrote_is_re_read_and_its_events_replaced`: recount interval/persistence and authoritative event replacement remain covered.
- `legacy_playbook_reader_preserves_migration_fields_and_expiry_policy`: approved rows and pending revisions survive; fresh drafts retain their fields; expired drafts are swept; output remains sorted.
- `migrate_records_writes_playbook_rows_as_files`: directly seeded approved/draft SQL rows become the correct files, draft content remains inert, source rows survive, and repeating migration preserves subsequent file edits.

Five obsolete daily-rollup tests were removed. Six SQL CRUD/expiry tests were replaced by one reader contract test, leaving 10 fewer Rust tests overall. Existing usage summaries, session totals, migration, TUI, backend, and MCP suites remain in the workspace gate.

## Gate

| Check | Result |
|---|---|
| `cargo test --workspace` | Exit 0; 1,331 passed, 2 ignored, 0 failed across 40 result groups; 84.748 s. |
| `cargo clippy --workspace --all-targets` | Exit 0; existing warnings only; 1.120 s. Tail: `Finished dev profile [unoptimized + debuginfo] target(s) in 1.03s`. |
| Touched-file formatting: `rustfmt --edition 2024 --check --config skip_children=true` on all six touched Rust files | Exit 0; no output; 0.106 s. |
| `bun run check` in `apps/desktop` | Exit 0; `tsc --noEmit`; 3.537 s. |
| `bun run test --maxWorkers=2` in `apps/desktop` | Exit 0; `Test Files 108 passed (108)`, `Tests 1156 passed (1156)`; 44.707 s wall time. |
| `bun run bindings:check` in `apps/desktop` | Exit 0; 107 binding exports passed, no tracked drift; 24.962 s. |
| `git diff --check` and migration-file comparison against base | Exit 0; no whitespace errors or migration changes. |
| Added-line literal review | No hex color, emoji, or em dash candidates. |

Evidence is in this worktree's `qa/evidence/`. The first workspace pass also passed (148.440 s); the final pass was rerun because the last fixture isolation edit landed while the first test binary compiled. Frontend source and generated bindings did not change after their successful gates.

The gate uses worktree-local config, data, source fixtures, socket, and tmux directories. No production database was copied or opened, and the production socket was not used. Runtime cleanup addresses only exact socket paths under this round's `qa/gate-tmux` directory. No process-name matching is used. Cleanup found 54 owned socket paths: two remaining servers were stopped and 52 had already stopped.

## Remaining validation and handoff

Native Windows CI or the VM should run the workspace tests, particularly the file-backed migration fixture and ledger replacement/reopen regressions. This macOS pass does not execute Windows-only ConPTY host and process paths. No frontend behavior changed in this round, and no bundle was built or installed.

The branch is ready for operator review and merge. Round 3 will start from current main only after that merge is confirmed. No merge or push was performed.
