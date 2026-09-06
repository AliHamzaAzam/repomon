# Audit Round 4: measured lane-filtered usage sessions

Date: 2026-09-06. Branch: `chore/audit-r4-measured`.
Base: `a4e568f187b6b77dc3cba30e6eee36745b6cfb5f`, accepted Round 3 on main.
Worktree: `/private/tmp/repomon-audit-r4`.

## Decision and scope

Item 17 is justified on this fixture. Commit `ad9b937` replaces the optional lane `OR` predicate with a direct `e.lane_id = ?3` clause when a lane is supplied, and omits the clause for an unfiltered query. Both forms share the existing SELECT, grouping, dominant-model subquery, ordering, row mapping, and limit. All values remain bound parameters. No index, migration, cache, or query result field was added.

The selective monthly query improves from 3.452 ms to 0.463 ms without planner statistics (7.5x), and from 7.367 ms to 0.471 ms after ANALYZE (15.6x). Full-history lane lookup improves from 28.849 ms to 1.726 ms without statistics. Unfiltered median timings stay within 3% in these paired runs. These are SQL fixture measurements, not a claim about whole-app latency.

Item 18 is deferred. The small predicate change requires no module extraction or editor-loading change.

## Fixture and method

- macOS 27.0 (26A5425a), arm64; project rusqlite 0.40 with bundled SQLite 3.53.1. The probe links the workspace development artifacts, so these are not release-build timings.
- A fresh 61 MiB synthetic database has 180,000 events, 3,600 sessions, 120 lane ids, and some unattributed events. Each session has 50 events with varying models, tokens, estimates, subagent flags, and metadata. Timestamps span June through August 2026. Only shipped usage migrations 23 through 29 construct the relevant schema and indexes; no production database is copied.
- Ranges are one hour beginning August 1, August through September 1, and June through September 1. Each runs unfiltered, for existing lane 7, and for absent lane 999, with limit 200. Test both before and after ANALYZE.
- Each pair has three warmups and 21 timed samples, alternating which form runs first. Timing includes statement preparation, stepping, and copying every returned column into Rust values. It excludes daemon scheduling, pricing, JSON encoding, and rendering. The filesystem and SQLite cache are warm.
- Every column and the returned row order are compared between old and new queries in every sample. All 18 scenarios match exactly. The production SQL forms were compared with the measured SQL and match after whitespace normalization.

## Timings

All values below are milliseconds. The full sample vectors and full before/after plans for every scenario are attached in [session-query-measure.json](audit-round-4/session-query-measure.json).

| ANALYZE | Range | Lane | Rows | Before median | After median | Before p95 | After p95 |
|---|---|---|---:|---:|---:|---:|---:|
| No | hour | All | 2 | 0.144 | 0.140 | 0.200 | 0.158 |
| No | hour | 7 | 0 | 0.066 | 0.062 | 0.072 | 0.069 |
| No | hour | 999 | 0 | 0.066 | 0.062 | 0.066 | 0.062 |
| No | month | All | 200 | 55.664 | 55.874 | 59.177 | 60.981 |
| No | month | 7 | 7 | 3.452 | 0.463 | 3.763 | 0.492 |
| No | month | 999 | 0 | 3.131 | 0.066 | 3.197 | 0.076 |
| No | history | All | 200 | 261.945 | 261.031 | 266.492 | 264.032 |
| No | history | 7 | 28 | 28.849 | 1.726 | 31.567 | 2.002 |
| No | history | 999 | 0 | 27.989 | 0.089 | 29.923 | 0.138 |
| Yes | hour | All | 2 | 0.148 | 0.144 | 0.153 | 0.151 |
| Yes | hour | 7 | 0 | 0.070 | 0.066 | 0.092 | 0.087 |
| Yes | hour | 999 | 0 | 0.081 | 0.079 | 0.095 | 0.088 |
| Yes | month | All | 200 | 61.324 | 60.370 | 64.995 | 64.727 |
| Yes | month | 7 | 7 | 7.367 | 0.471 | 7.689 | 0.507 |
| Yes | month | 999 | 0 | 6.958 | 0.079 | 7.674 | 0.100 |
| Yes | history | All | 200 | 123.392 | 124.186 | 130.195 | 129.619 |
| Yes | history | 7 | 28 | 23.782 | 1.794 | 25.577 | 1.886 |
| Yes | history | 999 | 0 | 22.762 | 0.095 | 24.838 | 0.155 |

## EXPLAIN QUERY PLAN

Monthly lane-7 query before, without ANALYZE:

```text
SEARCH e USING INDEX idx_usage_events_at (at>? AND at<?)
SEARCH s USING INDEX sqlite_autoindex_usage_sessions_1 (agent_kind=? AND session_id=?) LEFT-JOIN
USE TEMP B-TREE FOR GROUP BY
CORRELATED SCALAR SUBQUERY 1
SEARCH x USING INDEX idx_usage_events_session (agent_kind=? AND session_id=?)
USE TEMP B-TREE FOR GROUP BY
USE TEMP B-TREE FOR ORDER BY
USE TEMP B-TREE FOR ORDER BY
```

After, on the same database and parameters:

```text
SEARCH e USING INDEX idx_usage_events_lane (lane_id=? AND at>? AND at<?)
SEARCH s USING INDEX sqlite_autoindex_usage_sessions_1 (agent_kind=? AND session_id=?) LEFT-JOIN
USE TEMP B-TREE FOR GROUP BY
CORRELATED SCALAR SUBQUERY 1
SEARCH x USING INDEX idx_usage_events_session (agent_kind=? AND session_id=?)
USE TEMP B-TREE FOR GROUP BY
USE TEMP B-TREE FOR ORDER BY
USE TEMP B-TREE FOR ORDER BY
```

After ANALYZE, the old lane query uses `idx_usage_events_model (ANY(model) AND at>? AND at<?)` instead; the new query still uses `idx_usage_events_lane (lane_id=? AND at>? AND at<?)`. The unfiltered before/after plans match: the time index without ANALYZE, and the model/time skip-scan with ANALYZE for the monthly range. Session joins, correlated model lookups, and temporary grouping/order trees remain. No additional index is justified by these results.

## Regression and gates

`session_lane_filter_preserves_aggregates_metadata_and_range_limits` passes. It checks separate groups for identical session ids from different agent kinds, inclusive/exclusive time boundaries, null sessions, unattributed lanes, missing/zero lane ids, zero/one limits, empty ranges, missing digests, metadata fields, estimated/subagent/token totals, and newest-first results. It also preserves the existing dominant-model lookup across the whole session, even when a lane filter excludes some of its events. Existing session, recount, and transactional replacement tests remain.

| Gate | Result |
|---|---|
| `cargo test --workspace` | Exit 0; 1,332 passed, 2 ignored, 0 failed across 40 result groups; 122.996 s including compilation. |
| `cargo clippy --workspace --all-targets` | Exit 0; existing warnings only; 24.310 s. |
| `cargo fmt --all --check` | Exit 0; no output; 0.586 s. |
| `bun run check` in `apps/desktop` | Exit 0; `tsc --noEmit`; 3.529 s. |
| `bun run test --maxWorkers=2` in `apps/desktop` | Exit 0; `Test Files 109 passed (109)`, `Tests 1158 passed (1158)`; 26.177 s wall time. |
| `bun run bindings:check` in `apps/desktop` | Exit 0; 107 exports passed, no tracked drift; 28.009 s. |
| Migration comparison, SQL comparison, and `git diff --check` | Pass; no migration changes, measured and production SQL agree. |

Exact socket cleanup found 27 owned test sockets: one remaining server was stopped and 26 had already stopped. No process-name matching was used.

The committed runner was also executed successfully against a second fresh fixture after all gates finished. All 18 scenarios again returned identical rows. The monthly lane-7 median without ANALYZE was 3.581 ms before and 0.470 ms after. [Reproduction samples](audit-round-4/session-query-recheck.json) are attached separately; the primary measurements above remain the original run.

## Reproduction and evidence

Run `python3 qa/audit-round-4/run.py qa/evidence/query-recheck` from this checkout. The script builds repomon-core, links the probe to its bundled rusqlite, and requires a fresh output directory under this worktree's qa directory. It never overwrites an existing fixture. The source, frozen SQL forms, and original result JSON are committed beside this report; the generated database and executable remain local QA artifacts.

- [Probe source](audit-round-4/measure.rs) and [runner](audit-round-4/run.py).
- [Before SQL](audit-round-4/session-before.sql), [filtered after SQL](audit-round-4/session-filtered-after.sql), and [unfiltered after SQL](audit-round-4/session-all-after.sql).
- Original [synthetic fixture database](evidence/session-query-fixture.db), retained locally; logs: `qa/evidence/session-query-measure.log` and the workspace/frontend gate logs.

## Remaining validation and handoff

The fixture has many selective lanes and is cache-warm; it does not establish gains for a production database dominated by one lane or for cold storage. Native Windows CI should run the workspace and regression tests; the macOS gate does not execute Windows-only ConPTY paths. No Windows performance claim is made.

Gate runtime paths are isolated to this worktree. No production DB/socket, bundle, merge, or push was used. The branch is ready for operator review and merge; no broader optimization round is implied.
