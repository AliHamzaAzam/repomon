# C1 comment pass

Base: `35d003a3039ade243a2600ec82fa3c7fe834c54c`.
Branch: `chore/comment-pass`.
Worktree: `/private/tmp/repomon-comment-pass`.

The pass reduces 15,923 standalone comment lines to 10,600, removing 5,323 (33.4%). Runtime sources, migrations, installers, workflows, and configuration retain their code and literal contents. No bundle, merge, push, production socket access, or production database access was performed.

## Counts and commits

Counts cover tracked in-scope sources against the base commit, including unchanged files and generated bindings. The lexical inventory counts standalone `//` (including `///` and `//!`), `/*`, `{/*`, and block-body `*` lines; it excludes inline suffix comments and text inside literals. SQL `--` and script/configuration `#` lines are included in their areas. Shebangs and PowerShell executable help blocks are excluded. The per-file inventory and area membership are attached locally in `qa/evidence/comment-counts.json`.

| Area | Before | After | Removed | Commit |
|---|---:|---:|---:|---|
| repomon-core | 3,931 | 2,628 | 1,303 | `75fa7e9` |
| repomon-daemon | 5,034 | 3,071 | 1,963 | `ab0faf4` |
| repomon-tui | 1,883 | 1,549 | 334 | `f526007` |
| repomon-host | 249 | 186 | 63 | `6d5f75b` |
| repomon-mcp | 341 | 254 | 87 | `e8f6b8c` |
| Desktop: Tauri | 267 | 180 | 87 | `3831ef0` |
| Desktop: shell and settings | 1,412 | 707 | 705 | `89d8386` |
| Desktop: work views | 510 | 292 | 218 | `afa69a2` |
| Desktop: agent surfaces | 245 | 153 | 92 | `9437050` |
| Desktop: shared state, IPC, styles and tests | 876 | 565 | 311 | `a213baf` |
| Desktop: generated bindings | 955 | 920 | 35 | `e99409f` |
| Desktop: tooling | 54 | 31 | 23 | `4444b54` |
| Scripts, workflows and configuration | 166 | 64 | 102 | `8ed658a` |
| **Total** | **15,923** | **10,600** | **5,323** | |

## Wrong or misleading comments corrected

References below point to the corrected comment or its current API item after the pass.

| Location | Correction |
|---|---|
| `crates/repomon-core/src/agent/backend.rs:126` | The backend boundary is implemented by tmux and Windows ConPTY, rather than being a future Windows extension. |
| `crates/repomon-core/src/agent/mod.rs:1` | Monitoring combines transcript evidence with platform window liveness; the runtime is not tmux-only. |
| `crates/repomon-core/src/agent/tmux.rs:1` | A lane can own multiple durable agent windows; the header no longer implies one lane-<id> window per lane. |
| `crates/repomon-core/src/config.rs:148` | Orchestrator selection supports the implemented backends, rather than only Claude variants. |
| `crates/repomon-core/src/config.rs:1` | Endpoint documentation includes Windows named pipes alongside Unix sockets. |
| `crates/repomon-core/src/git/reader.rs:1` | Removed the blanket claim that Git reads never use subprocesses; the header now describes this module’s gix-backed reads. |
| `crates/repomon-core/src/local_llm/mod.rs:73` | Dropping model ownership does not guarantee zero resident process memory; documentation now states the ownership guarantee. |
| `crates/repomon-core/src/notes.rs:1` | Flat data-directory notes are migration input; live home notes are handled by the daemon’s repomind notes module. |
| `crates/repomon-core/src/notify.rs:73` | The fallback notification key does not imply that only one windowless session can exist in a lane. |
| `crates/repomon-core/src/pricing.rs:275` | Source counts use each model’s newest stored row, rather than an implied as-of-now filter. |
| `crates/repomon-core/src/store/mod.rs:19` | Migration versions are explicit stable identifiers; array position cannot determine their meaning. |
| `crates/repomon-core/src/store/mod.rs:1901` | Session metadata upserts merge incrementally; source replacement is a separate transactional operation. |
| `crates/repomon-daemon/src/conn.rs:79` | Drop schedules asynchronous session cleanup; it cannot promise synchronous removal before the connection disappears. |
| `crates/repomon-daemon/src/files.rs:1` | File APIs are implemented current functionality, rather than upcoming editor work. |
| `crates/repomon-daemon/src/lib.rs:101` | Codex orchestration uses pane attention and has no orchestrator transcript lookup; this does not claim the core Codex monitor reads nothing. |
| `crates/repomon-daemon/src/lib.rs:136` | Sessions track controller-lane or adopted fallback windows, rather than a necessarily separate daemon-owned orchestrator window. |
| `crates/repomon-daemon/src/main.rs:1` | The daemon serves platform IPC, not exclusively Unix sockets. |
| `crates/repomon-daemon/src/notify_watch.rs:43` | Repeat suppression uses message timestamps, not transcript modification times. |
| `crates/repomon-daemon/src/pubsub.rs:38` | Manual quota-refresh completion and ledger/pricing invalidation have distinct event contracts. |
| `crates/repomon-daemon/src/push.rs:79` | APNs collapse IDs are capped at a UTF-8 boundary; safety does not depend on all future IDs remaining ASCII. |
| `crates/repomon-daemon/src/remote.rs:22` | The connection cap includes pending unauthenticated handshakes, rather than only upgraded authenticated peers. |
| `crates/repomon-daemon/src/rpc.rs:67` | The device name is the query value and the bare token is the fragment; the percent-encoding helper applies to the name. |
| `crates/repomon-daemon/src/rpc.rs:769` | Binary and oversized reads are parameter errors, while filesystem I/O failures are internal errors. |
| `crates/repomon-daemon/src/rpc.rs:5704` | The constant describes session retention, not the adjacent overlay operation. |
| `crates/repomon-daemon/src/rpc.rs:5720` | The overlay TTL is documented independently of stale watcher timing claims. |
| `crates/repomon-daemon/src/rpc.rs:6586` | Every unclaimed live window can produce a placeholder; pairing is backed by stamps or pane evidence, not assumed transcript age order. |
| `crates/repomon-daemon/src/rpc.rs:6981` | Resize ownership includes freshness and newer viewport claims, rather than an unconditional local-wins shortcut. |
| `crates/repomon-daemon/src/rpc.rs:7900` | The resolver accepts the implemented Claude, Codex, Antigravity, and OpenCode choices. |
| `crates/repomon-daemon/src/rpc.rs:8290` | The helper attaches boot context through launch flags; adjacent MCP-registration prose described another operation. |
| `crates/repomon-daemon/src/socket.rs:54` | Cancellation may interrupt a partial frame, so timeout/shutdown abandons the reader rather than promising a cancellation-safe resumable read. |
| `crates/repomon-daemon/src/usage_ingest.rs:720` | An absent headline result is distinct from retrying a source; callers control retirement of the extraction version. |
| `crates/repomon-tui/src/app.rs:323` | Notification latches follow message activity rather than file modification times. |
| `crates/repomon-tui/src/app.rs:781` | Lane ordering uses pin, attention, and stable lane ID within daemon repository order, rather than most recent activity. |
| `crates/repomon-tui/src/emu.rs:1` | The terminal emulator consumes backend output, rather than depending on the removed pipe-pane transport. |
| `apps/desktop/src-tauri/src/cli.rs:155` | The timeout constant describes the bounded PATH probe, instead of carrying the following function’s PATH-resolution documentation. |
| `apps/desktop/src/components/SystemHealthView.tsx:193` | Removed the claim that CLI installation always copies a binary; Unix installation can use a symlink. |
| `apps/desktop/src/index.css:543` | Removed the claim that no other responsive media queries exist. |
| `.github/workflows/desktop-preview.yml:77` | Windows packaging includes the CLI and requires the ConPTY host, rather than describing an obsolete two-sidecar set. |
| `.github/workflows/release.yml:77` | Removed the obsolete suggestion that the Windows host might not exist yet and retained the actual target-specific packaging requirement. |

## Preserved contracts and behavior checks

- Private function comments are at most three lines and module headers at most eight. The public unsafe PATH repair retains its `# Safety` section.
- Public API and generated DTO docs retain concise contracts. The ts-rs export suite regenerated the bindings; its 107 exports passed.
- Three Clap help comments retain their original punctuation because derive macros turn them into executable user-facing help: `crates/repomon-host/src/cli.rs` (program arguments), and `crates/repomon-tui/src/cli.rs` (orchestrate and Windows attach). PowerShell’s `.SYNOPSIS`/`.DESCRIPTION` block is preserved for the same reason. These are the deliberate exceptions to prose cleanup and punctuation rules under the behavior-preserving requirement.
- Source-equivalence checks strip only lexically identified comments and compare nonblank source lines. A separate exact-literal check covers Rust strings/characters and TypeScript literals, templates, and meaningful JSX text. Python ASTs also match. JSX comment expressions are treated as one removable comment construct.
- Compiler/tool directives, safety arguments, migration versions and SQL, protocol examples, fixture output, command text, and user-facing strings remain intact.
- The audit inventory’s removed APIs remain removed; current migration inputs and live compatibility paths remain present. Markdown documentation outside executable sources was outside this pass.

Local evidence: `qa/evidence/rust-css-comment-equivalence.json`, `ts-comment-equivalence.json`, `config-comment-equivalence.json`, `rust-literal-equivalence.json`, and `ts-literal-equivalence.json`.

## Gate

The gate uses worktree-owned config, data, source-fixture directories, IPC endpoint, repomind home, and tmux namespace. It does not override HOME or CODEX_HOME.

- `cargo test --workspace`: exit 0, 125.74 seconds.
- `cargo clippy --workspace --all-targets`: exit 0, 11.06 seconds.
- `cargo fmt --all --check`: exit 0, 0.59 seconds.
- `bun run check`: exit 0, 3.69 seconds.
- `bun run test --maxWorkers=2`: exit 0, 33.53 seconds.
- `bun run bindings:check`: exit 0; 107 export tests pass and generated bindings have no diff.

Cargo aggregate: 1,332 passed, 0 failed, 2 ignored across 40 test/doc-test summaries. Frontend: 1,158 tests in 109 files passed. Clippy exits successfully with existing warnings; this pass does not change code to address them.

Relevant coverage includes frame and remote handshake handling, window death while siblings survive, orphan confirmation/reset, transcript/window pairing, source replay/replacement, pricing, migration compatibility, editor self-echo and persistence, terminal readiness, and platform health rendering. Test bodies and assertions are unchanged.

Gate logs and timing records are in `qa/evidence/`. No Windows VM or bundle validation was run. Windows-only comments were reviewed against their source; no runtime behavior changed that would require a new VM smoke test from this pass alone.

### Gate tails

`cargo-test.log`:

```text
   Doc-tests repomon_tui

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

```

`cargo-clippy.log`:

```text
    |                                    ^^^^^^^^^^^^^^^^^^ help: try: `std::slice::from_ref(&rel_str)`
    |
    = help: for further information visit https://rust-lang.github.io/rust-clippy/rust-1.95.0/index.html#cloned_ref_to_slice_refs

warning: `repomon-daemon` (lib test) generated 14 warnings (5 duplicates) (run `cargo clippy --fix --lib -p repomon-daemon --tests -- ` to apply 1 suggestion)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.92s
```

`cargo-fmt.log`:

```text
(empty output; exit 0)
```

`bun-check.log`:

```text
$ tsc --noEmit
```

`bun-test.log`:

```text

 Test Files  109 passed (109)
      Tests  1158 passed (1158)
   Start at  19:51:39
   Duration  32.65s (transform 4.72s, setup 3.71s, collect 10.27s, tests 14.72s, environment 21.40s, prepare 3.81s)

```

`bindings-check.log`:

```text
test result: ok. 107 passed; 0 failed; 0 ignored; 0 measured; 503 filtered out; finished in 0.37s

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.00s

```
