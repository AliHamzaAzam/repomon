# Audit Round 1: correctness

Date: 2026-09-06. Branch: `chore/audit-r1-correctness`.
Worktree: `/private/tmp/repomon-audit-r1`.
Base: current main `5b6eaecad677390faa2f285f347be7445400fa4c`.
Scope: approved audit items 1 through 8 and 11. Items 2 and 5 were rechecked against this base and both remained present.

## Changes and commits

| Item | Commit | Change and regression coverage |
|---|---|---|
| 1 | `fd80822` | Complete the MCP `AgentSession.attention_kind` fixtures. All 41 MCP library tests pass. |
| 2 | `87b73af`, `17edd09` | Run one fleet load at a time with one coalesced follow-up. Keep each completed active-generation snapshot; reject stopped-generation results. Cancel queued debounce on stop and dispose late old subscriptions. Tests cover slow loads over multiple heartbeats, stop/restart, and error recovery. The second commit gives the deferred fixture promise its explicit snapshot type. |
| 3 | `8f2c8c9`, integration coverage in `38d8eba` | Count recount strikes only when at least 60 seconds have elapsed since `scanned_at`. Controlled-time store coverage tests rapid failures, the exact minute boundary, reopen persistence, and reset on success. The daemon test backdates its fixture cursor between accepted strikes and verifies rapid extra passes do not alter it. |
| 4 | `5cb8671` | Introduce a fixture owner for the audited integration, Repomind boot, and Repomind export suites. It retains its temporary directory, supplies config/notes/home/basic-memory paths, and replaces the default backend namespace with a unique one. Explicit test namespaces and explicitly supplied paths remain supported. Regression verifies two fixtures cannot share default namespace or writable paths. All 32 tests in these three suites pass. |
| 5 | `a46a64a` | Share the file-read-to-tab mapping among open, reload, and active-tab restoration. Restore kind and size. Parameterized coverage checks PDF, image, binary, text, and legacy responses with no kind, including reopening an already cached restored tab. |
| 6 | `4796d3c` | Delete the ingest curl fetcher. `usage_rates` owns serialized refresh; validated snapshots and metadata publish via a unique adjacent file and atomic rename. Write failures clean temporary files. Tests cover concurrent readers, failed publication, invalid response preserving the previous cache/ETag, serialized refresh ETags, and ingestion making no price request. |
| 7 | `997b373` | Commit each source's events, session digest updates, and success cursor in one store transaction. Recount deletion is part of that transaction. Existing incremental and versioned session counter semantics are preserved. Injected insert/cursor failures verify event, daily rollup, digest, and cursor rollback; reopening and retrying then converges. |
| 8 | `38d8eba` | Repo/lane attribution reads return errors, and run before discovery or stale-cursor retirement. An error aborts the pass without cursor advancement. Fixture schema faults for each read verify unchanged cursors and no events, followed by properly attributed recovery. `rusqlite` is a test dependency for these fixture faults; the workspace already depends on it. |
| 11 | `231aa2a` | Clickable-path test waits for actual index/provider readiness instead of six microtasks. Path existence checks and click/open assertions remain. |

Touched Rust files were formatted to satisfy the round's touched-file formatting gate. This includes pre-existing formatting drift within those files; broad repository formatting remains reserved for Round 3. Added-line review found no hex color literals, emoji, or em dashes.

## Behavior and measurements

The same injected-source probe used for the audit takes 1,300 ms per load and observes 4,500 ms. Before: four starts, three completions, peak two concurrent loads, `synced=false`. After: four starts, three completions, peak one concurrent load, `synced=true`; the fourth load was still active at observation time. This is a store scheduling probe, not native daemon or browser performance.

The source transaction retains the daily rollup until the approved deletion round. Session digest counters retain their current version-aware replacement/addition contract. No migrations or generated binding contracts changed.

Price cache publication keeps the prior complete file until rename succeeds. Snapshot and metadata are separate files; publication is serialized within the daemon process, but they are not a single crash-atomic pair. Metadata persistence failures are logged. This round does not introduce cross-process file locking or change offline/refresh settings.

## Gate

All required checks passed on the final source tree. Logs are retained in `qa/evidence/`.

| Gate | Result |
|---|---|
| `cargo test --workspace` | Exit 0, 1,341 passed, 2 ignored, 0 failed. Final run 84.444 s; the initial full build/run also passed. |
| `cargo clippy --workspace --all-targets` | Exit 0, 5.929 s. Existing warnings remain; the new fixture style warning was removed. |
| Touched-file formatting (`rustfmt --edition 2024 --check` on all nine touched Rust files) | Exit 0, 0.117 s. |
| `bun run check` | Exit 0, 12.515 s. |
| `bun run test --maxWorkers=2` | Exit 0, 108 files / 1,156 tests passed, 85.887 s wall time. |
| `bun run bindings:check` | Exit 0, 107 export tests passed and no generated binding diff, 14.849 s. |

 Runtime tests use dedicated config, data, socket, transcript roots, and a tmux temporary directory. Exact sockets under that owned directory were checked for cleanup after the gate. No production socket or database was used or copied, and no bundle was built.

## Named regressions

- `stores/fleet.test.ts`: `applies slow loads while heartbeats coalesce into one follow-up`; `discards a stopped load and starts only one fresh load after restart`; `runs the coalesced load after an error and clears the error on recovery`.
- `store::tests::recount_failures_survive_reopen_and_success_resets_the_streak`.
- `fixture_defaults_isolate_backend_and_writable_paths` in `tests/integration.rs`.
- `stores/editor.test.ts`: `restores %s tabs with the same read state as open and reload` (five response kinds).
- `usage_rates::tests::atomic_cache_publication_never_exposes_partial_json`, `failed_cache_publication_keeps_destination_and_removes_temp`, `invalid_refresh_keeps_the_previous_snapshot_and_etag`, and `concurrent_refreshes_use_the_last_published_etag`.
- `usage_ingest::tests::ingest_does_not_fetch_prices_when_refresh_is_enabled`.
- `store::tests::source_replacement_rolls_back_events_digests_and_cursor_then_recovers_after_reopen`.
- `usage_ingest::tests::attribution_read_errors_preserve_cursors_and_recover_without_unassigned_events`.
- `usage_ingest::tests::unreadable_stale_source_retires_on_third_failure_preserving_events_and_error` now tests accepted minute-spaced strikes and ignored rapid passes.
- `TerminalPane.test.tsx`: `registers link provider that verifies against file.index and opens in editor on Cmd-click`.

## Remaining validation and handoff

Windows CI/VM must exercise actual host namespaces and named pipes, replacement of an existing cache file while native readers are active, packaged CLI behavior, external ConPTY resize events, and native title-bar/snap/DPI behavior. The latter platform behaviors are unchanged in this round. The clickable-path test needs the previously failing CI platform as well as the local full suite.

Round 2 has not started. Wait for the operator's merge before creating the next branch from main.
