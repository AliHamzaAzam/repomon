# Audit Round 3: formatting and focused hygiene

Date: 2026-09-06. Branch: `chore/audit-r3-hygiene`.
Base: `4ed58ab74a6ce07cc68e7bb437abb10004c196c7`, accepted Round 2 on main.
Worktree: `/private/tmp/repomon-audit-r3`.
Scope: accepted items 10, 14, 15, and 16.

## Changes and commits

| Item | Commits | Result |
|---|---|---|
| 10 | `d35746f` | First commit, exclusively mechanical `cargo fmt --all` output across 20 Rust files. Reformatting each base file independently reproduces its committed content exactly. The full workspace formatting check now passes. |
| 14 | `726cd92` | One `chordFor` beside the keymap registry replaces three lookup copies; one `formatBytes` replaces four viewer copies. The first registered alias, platform-specific display, absent-size handling, units, and precision stay the same. Key-event normalization and rate formatting remain separate. |
| 15 | `470916f`, `5769b95` | Corrected the fleet heartbeat, panel registry, worktree prune, digest batching, and image-read documentation. Shortened identity-binding comments while retaining the partial-snapshot risk, evidence requirement, and regression reference. Cleaned owned desktop and CLI/TUI presentation strings; updated error-copy test expectations. |
| 16 | `ba20466` | Removed the line-diff unit test's two-second deadline. The sweeping-edit case now asserts exactly 1,500 modified hunks for 3,000 lines; existing edit-distance and total-line cap coverage remains. |

Item 15 converts the standalone shortcut printout's fixed colors to equivalent RGB values and removes redundant color literals from the brand comment. Desktop notices use sentence punctuation. CLI/TUI display strings use colons; the TUI Repomind marker is `R`, and notification footers use `Notice:`. No protocol name, serialized field, terminal input mapping, or external agent parser changed.

Presentation cleanup is scoped, not a blanket text replacement. External agent-output fixtures, protocol examples, and historical documents remain intact. Existing theme and ColorField hex-input tests remain because they exercise the accepted custom-color syntax; replacing their values with RGB would stop testing that parser contract. No new hex color, emoji, or em dash literals were introduced after the mechanical formatting commit. Existing punctuation moved by rustfmt is unchanged in content.

No performance claim is made from the diff test. Its deterministic algorithm caps still provide bounded-work coverage; a separate benchmark was not added without CI timing evidence or a performance change.

## Verification

- `binding display lookup > uses the first registered alias and formats it for the platform`: verifies the primary Control Center alias on macOS and Windows, plus an unknown id.
- `preserves viewer byte units, precision, and absent sizes`: checks null, undefined, zero, byte/KB/MB boundaries, and rounding.
- `returns each modified hunk for a 3000-line document with sweeping changes`: verifies exact diff results without a wall-clock dependency. All 12 lineDiff tests pass, including cap cases.
- The full frontend suite covers the shared helper consumers, RightPanelHost, SystemHealthView, printable shortcuts, friendly errors, and both modal error views.
- The full Rust suite covers TUI rendering/help, backend behavior, migration, usage, MCP, and the persistent-state regressions from earlier rounds.

| Check | Result |
|---|---|
| `cargo test --workspace` | Exit 0; 1,331 passed, 2 ignored, 0 failed across 40 result groups; 130.961 s including compilation. Final doc-test tail: `test result: ok. 0 passed; 0 failed; 0 ignored`. |
| `cargo clippy --workspace --all-targets` | Exit 0; existing warnings only; 18.338 s. Tail: `Finished dev profile [unoptimized + debuginfo] target(s) in 18.22s`. |
| `cargo fmt --all --check` | Exit 0; no output; 0.518 s. Covers the whole workspace, including all touched files. |
| `bun run check` in `apps/desktop` | Exit 0; `tsc --noEmit`; 3.097 s. |
| `bun run test --maxWorkers=2` in `apps/desktop` | Exit 0; `Test Files 109 passed (109)`, `Tests 1158 passed (1158)`; 38.573 s wall time. |
| `bun run bindings:check` in `apps/desktop` | Exit 0; 107 binding exports passed, no tracked drift; 28.768 s. |
| Mechanical-format reproduction | All 20 committed files exactly match rustfmt output from their base versions. |
| Migration and external agent-fixture comparison | No changes against the base. |
| `git diff --check` and post-format added-line literal review | Pass; no new hex color, emoji, or em dash candidates. |

The initial frontend run found two modal expectations still using the previous error punctuation. Those assertions were updated, and the complete frontend gate was rerun. Earlier targeted checks also caught two retained `formatChord` imports and a test using browser platform labels instead of keymap platform ids; both were corrected before the helper commit. No unresolved gate failure remains.

Evidence and gate scripts are retained locally under `qa/evidence/` and `qa/`. Tests use worktree-local config, data, source fixtures, daemon socket, and tmux directory. Tests did not access production state. Runtime cleanup uses exact socket paths below this worktree only, with no process-name matching. Cleanup found 27 owned sockets: one remaining server was stopped and 26 had already stopped.

## Handoff and remaining validation

Native Windows CI or the VM should run the workspace gate and check the TUI labels and marker on its terminal/font. The macOS tests do not execute Windows-only ConPTY host paths. The print colors retain their numeric RGB values, but no native print dialog or Windows bundle was exercised. No bundle, merge, or push was performed.

Round 3 is ready for operator review and merge. Round 4 remains contingent on that merge and on new measurements for item 17. Item 18 remains deferred unless a trivial change is justified while touching the measured area.
