# Repomon full code audit, Phase 1

Date: 2026-09-06. Auditor: Codex. Branch: `chore/audit-2026-09`.
Audited base: `16cfd70bcb25a030a2f2bb919995a0030ecc4c73`, version 0.9.0.
Scope: the full-code-audit brief and its operator framing. This is a report only; no product source, tests, configuration, dependency declarations, or migrations were changed. Later main commits are outside this snapshot.

The first fixes should restore the workspace build, stop fleet refresh starvation, correct recount retirement, and preserve restored viewer metadata. The strongest deletion opportunities are the old price-cache fetcher and the daily usage rollup maintained without a runtime reader. Keep the event ledger, migration readers, keyed reconciliation, bounded diff, and identity safeguards.

## Ranked fix list

Severity: High means a broken gate, lost primary behavior, or a credible persistent-state/isolation risk; Medium means a recoverable correctness or maintenance problem; Low means cleanup. Effort: S is local, M spans an area, L crosses architectural boundaries. “Safe” means preserves current product behavior within this repository, not compatibility with unknown external consumers of public Rust APIs. No item is authorized for implementation by this report.

| Rank | Severity | Effort | Safe without behavior change? | Finding and files | Proposed bounded action |
|---|---|---|---|---|---|
| 1 | High | S | Yes, test fixtures only | MCP test build fails: `crates/repomon-mcp/src/fleet.rs:362`, `server.rs:1957` omit `AgentSession.attention_kind`. | Complete the fixtures and run the full workspace gate, including MCP. The narrower four-package gate does not compile these MCP test targets. |
| 2 | High | M | No | Fleet loads can starve: `apps/desktop/src/stores/fleet.ts:543`, `:653`, `:664`. Each poll advances the token even when the preceding load is pending. | Allow one load at a time, coalesce a requested follow-up, and retain stop/restart protection. Add a controlled deferred-load test spanning multiple heartbeat periods. |
| 3 | High | S | No | Recount strikes accrue per pass: `crates/repomon-core/src/store/mod.rs:2153`, `crates/repomon-daemon/src/usage_ingest.rs:799`. | Enforce the requested 60-second interval using `scanned_at`. Test rapid failures, a minute-separated third strike, persistence across reopen, and reset on success. |
| 4 | High | M | Yes for product; test setup changes | Several daemon fixtures isolate the home but inherit the production backend namespace and default paths: `crates/repomon-daemon/tests/integration.rs:63`, `tests/repomind_boot.rs:49`, `tests/repomind_export.rs:50`; constructor at `src/lib.rs:474`. | Consolidate explicit fixture paths and a unique backend namespace, or inject a fake backend where process behavior is irrelevant. Validate isolation before broader live-daemon tests. This is an exposure in source, not an observed production modification. |
| 5 | Medium | S | No | Restored active files omit `kind` and `size`: `apps/desktop/src/stores/editor.ts:715`. | Reuse the same file-read-to-tab mapping as open/reload. Test restoring PDF, image, binary, and text tabs and reopening an already cached restored tab. |
| 6 | Medium | M | No | Two price fetchers write one cache with different contracts: `crates/repomon-daemon/src/usage_ingest.rs:469`, `:550`; `usage_rates.rs:165`, `:187`, `:198`. | Delete the legacy curl fetch path, establish one refresh owner, and preserve the enabled/offline setting. Serialize publication and write a validated snapshot atomically with coherent metadata. |
| 7 | Medium | M | No | Recount replacement has a committed gap: `crates/repomon-daemon/src/usage_ingest.rs:655`, `:690`; store methods at `store/mod.rs:1975`, `:1857`. | Make replacement of one source's events and its success cursor transactional. Explicitly define session metadata handling. Add a failure between delete and insert, plus a reopen/retry test. |
| 8 | Medium | M | No | Ingest attribution converts store errors to an empty fleet: `crates/repomon-daemon/src/usage_ingest.rs:495`. | Return an error before advancing cursors when repos or lane metadata cannot be read. Test a failed index read followed by recovery. Keep display-label fallback separate from persisted attribution. |
| 9 | Medium | M | Yes within runtime API scope; schema removal separate | `usage_daily` is maintained but not read by runtime queries: `crates/repomon-core/src/store/mod.rs:1857`, `:1975`, `:2019`, `:2757`; `crates/repomon-daemon/src/usage_query.rs:141`. | Delete rollup maintenance and the unused reader/model helpers after checking their remaining test-only dependencies. Retain shipped migrations and historical tables initially. Do not build a new cache to replace an unused one. |
| 10 | Medium | S | Yes | Formatting gate fails in 30 Rust files. | Separate mechanical formatting commit, then recheck. Do not mix it with correctness changes. Full inventory below. |
| 11 | Medium | S | Yes for product | Clickable-path test assumes exactly six microtask turns: `apps/desktop/src/components/TerminalPane.test.tsx:103`, `:406`. | Wait for observable provider readiness with controlled font/renderer setup. Keep path verification and click behavior assertions. Reproduce on the failing CI platform; do not merely extend the flush count. |
| 12 | Low | M | Yes if legacy migration coverage retained | Legacy SQL playbook mutation/search API has no runtime callers: `crates/repomon-core/src/store/mod.rs:1336`, `:1378`, `:1406`, `:1428`. | Remove old mutation/search paths and their obsolete CRUD tests; seed legacy rows directly in migration fixtures. Keep `list_playbooks` and migration behavior in `crates/repomon-daemon/src/repomind.rs:390`. |
| 13 | Low | S | Yes within repository | Uncalled Rust utilities and redundant public exports: `crates/repomon-core/src/agent/tmux.rs:1136`, `usage_ledger/scan.rs:938`; Knip export inventory. | Delete `attach_args` and `require_readable` after a final caller check. Reduce export visibility only where useful; do not delete locally used functions or generated bindings based on Knip. |
| 14 | Low | S | Yes | Repeated chord lookup and byte formatting: `App.tsx:106`, `ControlCenter.tsx:67`, `Onboarding.tsx:668`; four viewer helpers listed below. | Put chord display lookup beside the registry and share the identical byte formatter. Keep platform-specific key event normalization and genuinely different rate formatting contracts. |
| 15 | Low | S | Yes, except user-visible punctuation | Stale or overlong explanations and banned literal inventory below. | Correct current-contract comments first; shorten redundant historical narration. Preserve protocol examples and parser fixtures that must match external output. Make presentation literal cleanup a separate scoped change. |
| 16 | Low | S | Yes for product | `lineDiff.test.ts:238` uses a wall-clock deadline. | Keep deterministic result/cap coverage; move the speed assertion into a measured benchmark if CI evidence shows it is flaky. Current local tests pass. |
| 17 | Low | M | Yes, with query equivalence tests | Lane-filtered usage sessions query uses the time index instead of the available lane/time index: `crates/repomon-core/src/store/mod.rs:2355`. | Only after the deletions above, measure separate filtered/unfiltered SQL forms. The optional `OR` condition prevents the selective plan in this fixture. No new index is justified yet. |
| 18 | Low | L | Intended yes; substantial review surface | Large modules and eager editor dependencies. | Split by existing domain boundaries only when touching the area. Consider lazy PDF/editor loading after an actual startup trace. Line count and chunk size alone do not justify a rewrite. |

## Confirmed behavior and persistent-state risks

### Fleet polling

A harness imported the actual `createFleetStore`, injected a source that takes 1,300 ms per load, and observed it for 4,500 ms. Four loads started, three completed, peak concurrent loads was two, `synced` stayed false, and `loading` stayed true. All responses succeeded. Every completion was superseded by the next 1,200 ms heartbeat. This is a deterministic source-level reproduction, not a claim that every production request takes that long.

The source makes six RPC calls per load, including `usage.summary` for today's sidebar cost. A slow dependency delays the entire `Promise.all`. Event refreshes can also supersede a pending heartbeat load. Fix the scheduling before tuning individual RPCs. Preserve the keyed `reconcile` used for repo and lane identity.

### Recount and attribution

The exact `fail_usage_recount` SQL was executed three times with the same timestamp in a copied synthetic database. In 0.298 ms the cursor advanced to the target version and reset its failures to zero. `recount_failures_survive_reopen_and_success_resets_the_streak` currently verifies persistence and reset but also accepts immediate strikes. When a backlog fills the 25-attempt recount batch, `next_ingest_delay` can schedule another pass after one second. Three fast failures can therefore retire a source without the intended recovery interval.

Successful recount currently commits source deletion, then inserts events in another store call, then updates session metadata, then advances the cursor. A failure between these operations leaves a partial visible ledger until retry. The old cursor normally makes recovery possible; permanent loss is not established by this audit. Atomic replacement would remove that recovery gap.

The ingest fleet index uses `unwrap_or_default` on both repo and lane reads. Unlike missing display labels, an empty attribution index can persist external/unassigned attribution and then advance an otherwise successful cursor. Database failure must not mean “the user has no repos.” This risk is source-confirmed; no fault was injected into a production store.

### Prices and redundant state

The old `refresh_price_cache` invokes curl with a 20-second timeout, accepts any nonempty successful response, writes directly to the cache, ignores write errors, and runs while holding the ingest pass lock. `usage_rates::run_refresh` uses ureq with a five-second timeout, validates the snapshot, and maintains ETag/status metadata. Both paths can be enabled and target `prices/litellm.json`. The old path bypasses the new metadata and validation contract. Both direct publication and concurrent refreshes need explicit treatment when consolidating. No network fetch or cache mutation was performed by this audit.

Runtime summary, timeline, findings, and export queries read `usage_events`. References to `usage_daily_between` occur in store/ingest tests; the daily table adds rollup work to event insertion, monotone count increases, and source deletion. Removing this ongoing work is preferable to optimizing its unused reader. Event identity, monotone token updates, and transactions should remain covered. Session metadata is different: headlines, tool counts, retries, and source provenance have actual readers and cannot simply be deleted as a duplicate of events.

File playbooks are the current runtime source of truth. Legacy SQL rows remain an input to `migrate_records`; remove obsolete SQL mutation/search surfaces without deleting that migration input. The migration explicitly preserves existing files, so it is not a second active synchronization loop.

### Fixture isolation

`Ctx::new_with_backend` calls `backend.configure()` when the selected session already exists. A test that only redirects `repomind.home` still uses the default tmux namespace and constructor defaults for other paths. `integration.rs` has an `isolated_config` helper, while the boot/export suites repeat equivalent partial initialization inline. The brief's “two helpers” is better understood here as duplicated setup, not two functions with that exact name. This audit's daemon harness supplied explicit DB/config/notes/home/basic-memory paths and a unique tmux namespace. It started no agent, bound no daemon socket, and left no runtime process running.

## Measurements

Host: macOS 27.0 (26A5425a), arm64. Measurements are local samples, not Windows or CI performance claims. The standalone Rust harness uses this checkout's source in an unoptimized development build. SQLite measurements use Python SQLite 3.53.4, not the daemon's bundled rusqlite build. Some checks ran concurrently, so these are baselines rather than controlled performance regressions.

### Daemon and locks

| Probe | Result | Interpretation |
|---|---|---|
| `lane.list`, 8 synthetic repos/lanes, 0 agents, compact JSON result | 5,997 bytes | Result body only, excludes protocol envelope/framing. No live-agent extrapolation. |
| First `lane.list` | 1,655.334 ms | Cold discovery path. |
| Next four `lane.list` calls | 0.080 to 0.089 ms each | Immediate cache hits; these do not measure cache expiry or ordinary heartbeat cost. |
| `ensure_home`, cold / warm | 42.180 / 0.821 ms | Whole-call upper bounds on uncontended `repomind_lock` occupancy; the guard covers almost the entire body. |
| Reconcile 8 cold watchers / warm watchers | 31.243 / 0.019 ms | Whole-call upper bounds, including viewport snapshot work before taking `lane_watchers`. Not exact lock-only timings. |
| Empty `rate_limits` mutex, 1,000 uncontended acquisitions | maximum 1,084 ns | Acquisition overhead only. Not populated-map retention time or production contention. |

`reconcile_lane_watchers` (`lib.rs:571`) nests viewport waits under the sessions lock and holds `lane_watchers` across `lanes.get` plus watcher creation. This deserves a contended trace before changing lock ownership. `repomind_lock` intentionally prevents concurrent initialization of the same Git home; retain that invariant. `rate_limits` is cloned or changed in short sections; its auto-continue retain uses a linear window search per entry, but this audit did not measure a populated-map bottleneck. No lock optimization is ranked as a proven performance fix.

Configured cadence, verified from this base rather than copied from old comments:

- Notify and supervision ticks: 2 seconds (`notify_watch.rs:32`, `supervision.rs:21`). Supervision exits early when disabled.
- Agent overlay cache: 500 ms (`rpc.rs:5856`). General pane sniff TTL: 4 seconds; running sniff TTL: 1,500 ms (`rpc.rs:6399`). Process lookup cache: 10 seconds (`rpc.rs:7980`).
- Ordinary attention captures use 45 lines; stamped-transcript confirmation can capture 500 (`rpc.rs:6889`). Controller notification capture uses 45 (`notify_watch.rs:445`).
- Auto-continue: 20-second tick, 120-line capture, 10-second await bound. These are configured limits, not measured per-agent capture latency. Do not infer that a Tokio await timeout kills its blocking subprocess.
- Ingest: configurable discovery budget, at least 30 seconds between ordinary passes; stale recount and headline redigest each have a 25-item batch. Backlog recount may run after one second. Full source reread size/time is not bounded merely by a file-count limit.

No populated agent capture, sustained idle CPU, network rate refresh, or Windows host timing was measured. Do not use the idle fixture to claim those paths are fast.

### SQLite plans on a copied fixture

Created a synthetic fixture using migrations 0023 through 0029, containing 100,000 events across 100 days, 1,000 sessions, and 1,000 cursors. Closed it and copied it before executing queries. Five fetch-all runs per query; medians below. Session SQL is copied directly from the store; event SQL selects all columns with the same filter/order.

| Query | Rows returned | Median | Plan |
|---|---:|---:|---|
| Events over 30 days | 30,000 | 61.635 ms | `idx_usage_events_at` range search |
| Sessions over 30 days, all lanes, limit 200 | 200 | 98.615 ms | Time index, session PK join, temporary grouping/order trees; correlated model lookup uses `idx_usage_events_session` |
| Same sessions query, one lane | 125 | 18.482 ms | Still the time index because of the optional lane predicate |
| Stale cursor batch, limit 25 | 25 | 0.247 ms | Cursor scan and temporary sort |
| Events for one source | 100 | 0.183 ms | `idx_usage_events_source` |

The session limit bounds returned rows, not the grouping or correlated-model work. A new cursor index is not justified by a 0.247 ms sample. Preserve the current time/session/source indexes. Compare a direct `lane_id = ?` query before adding another index or restructuring aggregation. Python row conversion is included; these timings exclude Rust pricing and JSON serialization.

### Desktop work and bundle

| Probe | Result |
|---|---|
| Continuous axis, folding, and stacking 50 series x 720 hourly buckets, 25 runs | median 6.040 ms; maximum 10.670 ms |
| Actual line diff, 3,000 lines with every other line changed, 25 runs | median 54.898 ms; maximum 75.578 ms |
| Frontend production build | 15.17 seconds, success |
| Main JS | 1,816.78 kB; gzip 565.30 kB |
| TerminalPane JS | 525.80 kB; gzip 140.80 kB |
| PDF worker asset | 1,375.84 kB, expected separate worker |
| Main CSS | 98.40 kB; gzip 16.04 kB |

The frontend helper probes ran under Bun, without DOM/layout. They are not browser frame-time measurements. `UsageView` memoizes the continuous axis and column plan; `UsageChart` memoizes folding/bars. `TerminalWorkspace` already derives visible targets and preserves warm panes with memoized/keyed state. Its eight targeted multitasking tests pass. No evidence here justifies replacing those structures or adding indiscriminate memoization.

`CodeEditor.tsx:947` debounces gutter calculation by 300 ms. The diff caps total lines at 20,000 and edit distance at 4,000. A 55 ms measured calculation still blocks its executing thread, but the caps and debounce are useful safeguards. Keep them; only consider a worker after a browser responsiveness trace on representative editing workloads.

`RightPanelHost` statically imports the file editor, which imports PDF support; `PdfViewer.tsx:13` statically imports pdf.js. Lazy boundaries are a plausible later startup improvement, not a measured startup regression. The worker is expected and should not be removed to make the chunk report smaller.

## Dead-code and duplication review

Clippy cannot prove exported APIs are used. Whole-tree symbol searches and manual call-site checks were used after the heuristic scan. The preliminary public-item candidate scan truncates files at the first test configuration marker and has false positives; it is not a deletion list. In particular, `upsert_worktree`, `prune_worktrees`, `get_or_create_lane`, and `worktree_template_for` have real callers in `lane.rs` and must stay.

Confirmed local deletion candidates are `TmuxRuntime::attach_args` and `usage_ledger::scan::require_readable`. The latter is also misnamed: `is_file()` does not establish readability. `pipe_pane_named` has a backend test caller; do not silently delete backend coverage merely because production uses the newer byte-stream abstraction. `list_worktrees` is currently exercised by store tests; decide whether to move its inspection into fixtures when deleting other unused store API.

Knip reported 6 files, 61 exports, 72 types, and one development dependency. It reported no unused production dependency, unresolved import, or duplicate export. These are analysis candidates:

- `e2e/run.ts` is invoked by `e2e/isolated.sh:71`; `webdriverio` is used there. Both are false positives due to entry discovery.
- `qa/contrast.mjs` is a manual QA utility, not an application entry.
- The other four file reports are generated bindings (`LaneMeta`, `RemoteDevice`, `SyncReport`, `TimeRange`). Keep generated outputs consistent with Rust export generation.
- Many reported icon functions are locally referenced by the icon catalog/render switch. For example, `IconAgentClaudeCode` is used at `icons.tsx:1346` and `:1423`.
- `isUrgentState` and `laneStateCount` are used inside `fleet.ts`; `subscribeConnection` similarly participates in the local connection source. An unnecessary `export` does not mean an unnecessary function.

Identical helpers worth consolidating when those files are next touched:

| Helper | Locations | Decision |
|---|---|---|
| Chord lookup/display | `App.tsx:106`, `ControlCenter.tsx:67`, `Onboarding.tsx:668` | One lookup beside `BINDINGS`; `chordOf` is event normalization and is a different operation. |
| Byte formatting | `ImageViewer.tsx:48`, `PdfViewer.tsx:102`, `EditorWorkspace.tsx:77`, `BinaryViewer.tsx:8` | Same precision/units and empty handling; share one helper accepting null/undefined. |
| Isolated daemon setup and framed RPC helpers | `tests/integration.rs:63`, boot/export fixture setup and `connect_retry`/`call` | Consolidate isolation guarantees first. A large test framework is unnecessary. |
| Rust/TS rates footnote formatting | `core/src/pricing.rs`, `components/usageMetrics.ts` | Separate language/runtime consumers; retain unless a shared data contract can remove a real mismatch. |

## Comments and current-contract documentation

| Location | Verdict |
|---|---|
| `apps/desktop/src/stores/fleet.ts:467` | “2s heartbeat” is stale; the poll is 1.2 seconds. Keep the explanation of keyed reconciliation and hover stability. |
| `apps/desktop/src/components/Onboarding.tsx:665` | “Mirrors” three copies is maintenance narration, not a guarantee against drift; remove with shared chord lookup. |
| `apps/desktop/src/components/RightPanelHost.tsx:14`, `:39` | Future C1/D4/pre-C1 narration is stale now that panels are implemented. Describe the actual registry and optional test injection. |
| `crates/repomon-core/src/agent/tmux.rs:1134` | Claims TUI uses `attach_args`, but no caller remains; delete with dead method. |
| `crates/repomon-core/src/usage_ledger/scan.rs:936` | Claims a readability check while only checking file type; delete with dead helper. |
| `crates/repomon-core/src/store/mod.rs:1392` | SQL playbook list is described as the approval surface; its runtime role is now legacy migration input. |
| `crates/repomon-core/src/lane.rs:96` | Says prune issues a DELETE even when nothing is removed; current `prune_worktrees` selects paths and only deletes absent entries. Keep the cache rationale without this false claim. |
| `crates/repomon-daemon/src/usage_ingest.rs:467` | Says curl avoids adding an HTTP stack; daemon already depends on ureq and has the newer rates fetcher. Remove with obsolete fetch path. |
| `crates/repomon-daemon/src/usage_ingest.rs:714` | “Cost per session is small” is an unmeasured assumption; a count-bounded batch can contain large rereads. Keep the batching rationale. |
| `docs/protocol.md:151` | Image preview wording is incomplete: asset protocol is primary, raw read is fallback and still used by Markdown. Do not remove this live RPC. |
| `crates/repomon-daemon/src/rpc.rs:7047` | Sixteen-line private helper comment: shorten historical narrative, retain the identity/headcount failure case and test reference. |
| `crates/repomon-daemon/src/rpc.rs:7067` | Thirteen-line pairing comment: retain sticky identity and ordering contract; move repeated incident history into its test. |
| `crates/repomon-daemon/src/rpc.rs:7317` | Eight-line resize arbitration rules explain a real precedence contract; keep. |
| `crates/repomon-core/src/agent/tmux.rs:180` | Long locale explanation documents external tmux behavior and parser protection; keep the non-obvious rationale. |
| `crates/repomon-core/src/store/mod.rs:20` | Migration numbering history explains why versions must not be inferred from array position; keep that invariant. |

`orchestrator.*` RPC names and adoption compatibility still exist. `docs/architecture.md:111` correctly says the legacy window is an adoption artifact. Historical plans describing an older concrete tmux architecture are historical, not current implementation guidance. Do not mechanically delete every occurrence of “orchestrator.” No current “Automation settings” replacement error was confirmed by the targeted search.

## Tests, platform coverage, and gates

| Check | Result |
|---|---|
| `cargo clippy --workspace --all-targets -- -W dead_code -W unused` (JSON output added for counting) | Exit 101, 162.064 s. Two E0063 fixture errors in MCP. 20 emitted warning diagnostics. |
| Same Clippy command with `--exclude repomon-mcp` | Exit 0, 11.499 s. 20 warning diagnostics; MCP library remains a dependency, but its test targets are excluded. This does not make the workspace gate green. |
| `cargo fmt --all --check` | Exit 1, 2.866 s; 30 files differ. |
| `bun run check` | Pass. `strict`, unused locals/parameters, and switch fallthrough checks are enabled. Root include is `src`; this alone does not validate shell entry discovery or all e2e/scripts. |
| `bun run build` | Pass, 15.17 s. Frontend only; no Tauri bundle. |
| Targeted frontend: TerminalPane, lineDiff, TerminalWorkspace.multitasking | 3 files, 29 tests pass, 2.30 s. |
| TerminalPane suite repeated ten times | 10/10 runs pass, 9 tests per run. Clickable-path CI flake not reproduced on this macOS host. |
| `cargo test -p repomon-core transport::tests -- --test-threads=1` | 6 tests pass after allowing temporary socket binds. Initial sandbox run denied binds with EPERM; that was an environment failure, not the stale-socket race. |
| Knip via `bunx knip --reporter json --no-progress` | Completed analysis with the candidates described above. |
| Nightly dependency audit | Not run: nightly/udeps was not installed. Stable symbol/dependency inspection used instead. |

Warning categories, counted as emitted diagnostics per Clippy invocation, not added across reruns: 7 `cloned_ref_to_slice_refs`, 4 `io_other_error`, 2 `useless_conversion`, 2 `unnecessary_lazy_evaluations`, 2 `map_entry`, and one each of `type_complexity`, `manual_split_once`, `field_reassign_with_default`. No dead-code/unused warning was emitted in these successful targets. Public dead APIs still require caller review. Fix the build errors before treating these as a complete all-target workspace inventory.

The stale-socket test uses tag plus PID in its temporary endpoint, and immediate listener drop/rebind passed here. Do not insert sleeps without reproducing a listener-release problem. The lineDiff deadline remains machine-load dependent even though its deterministic cap tests pass. `TerminalWorkspace.multitasking` uses bounded `waitFor` on visible behavior; the existence of a 5-second timeout is not itself a reason to rewrite working tests.

Windows pure naming, framing, ownership, and launch-decision tests exist; they do not execute macOS-inactive `cfg(windows)` host/process paths. `windows.rs:1046` explicitly leaves ordered external ConPTY Grid events as a parity TODO. Keep this as native validation work rather than declaring it covered by the macOS gate. The Windows smoke jobs are correctly prerequisites of publishing in both desktop workflows. Native title-bar controls, snap/DPI behavior, packaged CLI install/reinstall, host adoption, stream resizing, and real Git discovery still require Windows CI or the VM. This Phase 1 did not run the full Rust/frontend/bindings fix-round gates, modify their tests, or build/install a bundle.

## Hygiene inventory

Whole tracked-tree inventory: 629 paths, 607 readable text files. Automated coverage included the tracked text inventory, compiler checks, Knip, and targeted caller/comment searches. Manual review concentrated on daemon RPC/state, usage storage/ingest/query, backend/test isolation, fleet/editor/panel state, and the known flaky tests. This is not a claim that every line of every file received equal manual scrutiny.

- Em dash: 1,558 occurrences on 1,479 lines. Largest line counts: `rpc.rs` 179, TUI `app.rs` 173, TUI `view.rs` 60, `tmux.rs` 52, daemon `lib.rs` 47.
- Emoji/symbol candidate scan: 178 code points on 135 lines using broad pictograph/dingbat ranges. This deliberately overcounts stars, checkmarks, and prompt cursors; it is not a count of 178 emoji violations. Parser fixtures matching third-party agent output must remain accurate. Review presentation/docs separately from protocol examples.
- Frontend hex candidate scan: 12 literals on 8 lines. `src/index.css:6` has three comment references; `shortcutsPrint.ts:63,65,67` has five actual print stylesheet literals; `theme.test.ts:94,95` and `controls/ColorField.test.tsx:38,39` contain four input/assertion literals. Convert presentation colors to equivalent supported color syntax/tokens. Keep coverage for user-supplied color strings while expressing fixtures consistently with the no-literal rule.
- Four TODO/FIXME lines: `daemon/src/ext.rs:1759` and its historical extension-plan copy are a generated skill-description placeholder; `SettingsModal.tsx:753` has a connection-phase assumption; `core/src/agent/windows.rs:1046` records the ConPTY Grid gap. They are not four unfinished features.

Formatting drift files:

- `crates/repomon-core/src/agent/tmux.rs`
- `crates/repomon-core/src/agent/windows.rs`
- `crates/repomon-core/src/config.rs`
- `crates/repomon-core/src/git/mod.rs`
- `crates/repomon-core/src/pricing.rs`
- `crates/repomon-core/src/store/mod.rs`
- `crates/repomon-core/src/usage_ledger/mod.rs`
- `crates/repomon-core/src/usage_ledger/scan.rs`
- `crates/repomon-daemon/src/files.rs`
- `crates/repomon-daemon/src/remote.rs`
- `crates/repomon-daemon/src/repomind.rs`
- `crates/repomon-daemon/src/repomind/basic_memory.rs`
- `crates/repomon-daemon/src/repomind/boot.rs`
- `crates/repomon-daemon/src/repomind/export.rs`
- `crates/repomon-daemon/src/repomind/md.rs`
- `crates/repomon-daemon/src/repomind/playbooks.rs`
- `crates/repomon-daemon/src/rpc.rs`
- `crates/repomon-daemon/src/usage_ingest.rs`
- `crates/repomon-daemon/src/usage_query.rs`
- `crates/repomon-daemon/src/usage_rates.rs`
- `crates/repomon-daemon/src/worktree_watch.rs`
- `crates/repomon-daemon/tests/file_rpcs.rs`
- `crates/repomon-daemon/tests/integration.rs`
- `crates/repomon-daemon/tests/repomind_boot.rs`
- `crates/repomon-daemon/tests/repomind_export.rs`
- `crates/repomon-daemon/tests/repomind_primary.rs`
- `crates/repomon-daemon/tests/usage_pricing.rs`
- `crates/repomon-mcp/src/lib.rs`
- `crates/repomon-tui/src/cli.rs`
- `crates/repomon-tui/tests/repomind_cli.rs`

## Modules over 1,500 lines

Counts include inline tests. Split suggestions are boundaries for future work, not independent rewrite requests. Lockfiles and historical plans are excluded from the module table.

| Module | Lines | Useful boundary if the area is changed |
|---|---:|---|
| `crates/repomon-daemon/src/rpc.rs` | 13,227 | Dispatch/DTOs, agent identity overlay, terminal control, file/config handlers; preserve shared authorization. |
| `crates/repomon-tui/src/app.rs` | 6,835 | TUI state transitions and domain actions; keep input precedence explicit. |
| `crates/repomon-core/src/store/mod.rs` | 5,460 | Domain store methods and tests; keep the single connection/transaction owner. |
| `crates/repomon-tui/src/view.rs` | 3,231 | TUI pane/render sections. |
| `crates/repomon-tui/src/cli.rs` | 2,861 | Command families and argument parsing. |
| `crates/repomon-daemon/src/ext.rs` | 2,719 | Provider adapters, discovery, installation; delete duplicate adapters first. |
| `crates/repomon-core/src/agent/tmux.rs` | 2,351 | Command rendering, transport, streaming, and backend tests. |
| `crates/repomon-daemon/tests/integration.rs` | 2,247 | Integration suites by contract, with one isolated fixture helper. |
| `crates/repomon-mcp/src/server.rs` | 2,133 | MCP catalog, dispatch, and policy tests. |
| `crates/repomon-core/src/agent/supervision.rs` | 2,059 | Policy model versus evaluation, or daemon tick versus intervention phases. |
| `apps/desktop/src/components/icons.tsx` | 1,949 | Curated icon assets/catalog; no split justified solely by count. |
| `crates/repomon-daemon/src/usage_ingest.rs` | 1,844 | Discovery versus ingest/recount; first remove the legacy price fetcher. |
| `apps/desktop/src/components/SettingsModal.tsx` | 1,838 | Settings sections with shared save/error state. |
| `crates/repomon-core/src/agent/prompt.rs` | 1,706 | Dialog classifiers and corpus tests, preserving external text fixtures. |
| `crates/repomon-daemon/src/supervision.rs` | 1,543 | Policy model versus evaluation, or daemon tick versus intervention phases. |

At this base, `fleet.ts`, `EditorWorkspace.tsx`, and `UsageView.tsx` are below 1,500 lines despite the older brief examples.

## Handoff and proposed rounds

1. Agree the correctness list: MCP fixtures, fleet scheduling, recount interval, test isolation, editor restoration, price refresh ownership, atomic source replacement, and attribution failure handling. These require explicit regression cases, not blanket refactoring.
2. Delete obsolete work: daily rollups, old SQL playbook mutation/search, uncalled helpers. Preserve migration inputs and event invariants.
3. Mechanical formatting and focused test stabilization; then shared helpers/comments/presentation hygiene.
4. Only after new measurements, consider filtered session SQL, module extraction, and lazy editor loading. Leave currently sound caching, reconciliation, identity, and diff limits alone.

Raw logs, JSON measurements, the synthetic database/copy, and standalone QA probes remain under this worktree's `qa/` directory. They contain synthetic fixture data. The committed report contains the measurements and failure details needed for review; no production database was copied. No Phase 2 fix has begun. Operator agreement on the ranked list is the next required handoff.
