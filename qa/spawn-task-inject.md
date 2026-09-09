# F1 and F1a: complete task and mail delivery

Branch: `fix/spawn-task-inject`. No production socket or database was accessed. All runtime checks used unique test tmux servers and temporary directories. Real composer probes did not submit tasks to a model.

## Findings

1. **Spawn could consume the task as an option value.** `attach_agent_mcp` ends Claude's launch command with `--allowedTools mcp__repomon`. The installed Claude help declares `--allowedTools <tools...>`. The previous spawn code appended the positional task without `--`, allowing that variadic option to swallow it. This explains a successful launch with an empty composer even for native `high` effort. The separate slash-effort branch also typed the task after fixed 2,000 ms and 600 ms sleeps. Tasks now use supported launch arguments regardless of slash effort, with `--` separating Claude/Codex prompts from options.

2. **The shared typed path loses chunks inside the ready Claude composer.** The first incident transcript was read without modification: its first user message begins `and 1040x680 and state their paths (keep them uncommitted).`. The controller clarified that this copy arrived through `send_to_agent`, not spawn. A raw PTY byte receiver preserved all 2,571 bytes through both `send-keys -l` and argv; the real Claude composer did not. Its external-editor shortcut exported the complete input, including hidden paste placeholders, without submitting a task. Observed results:

| Input bytes | Old send-keys export | Head intact | Bracketed paste export |
| ---: | ---: | :---: | ---: |
| 1,001 | 1,001 | yes | 1,001 exact |
| 1,501 | 479 | no | 1,501 exact |
| 1,801 | 1,801 | yes | 1,801 exact |
| 2,001 | 2,001 | yes | 2,001 exact |
| 2,101 | 57 | no | 2,101 exact |
| 2,501 | 2,501 | yes | 2,501 exact |
| 4,097 | 3,075 | yes, middle missing | 4,097 exact |
| 8,193 | 5,127 | yes, middle missing | 8,193 exact |

The missing byte counts are multiples of 1,022 in these samples. Failures are non-monotonic: a larger input can succeed when a smaller one fails. A binary search cannot identify a stable safe cutoff. **There is no safe payload-size rule to infer from this failure. Keeping prompts short or replacing them with file pointers was a workaround, not a convention callers should retain after this fix.** The exact internal Claude implementation cause was not inspected; the measured failure is in consumption by its TUI, not tmux's raw-byte transport.

3. **Uncertain mail submission could be replayed.** Previously `mail::try_deliver` persisted delivery only after `inject::send_verified_line` confirmed the closing marker had left the composer. A cursor/verification miss left the message queued, and a subsequent sweep could type the full frame again. The marker was also shared by every message. There was no durable claim covering the side effect. This code path explains repeated injection after uncertain verification; the production database was not inspected to attribute each reported copy individually.

4. **Inbox recovery is present in this checkout, but not confirmed in the deployed daemon.** `message.inbox` passes `unread_only` through to `Store::list_messages`; the default is false in both RPC and MCP. The query filters by `read_at` only when true and does not exclude delivered messages. New tests cover full-body recovery after push, newest-first bounded pagination, and recovery after an uncertain attempt. MCP tool wording now explicitly documents false as the recovery option. The controller's production result of an empty inbox remains a deployment/identity discrepancy to verify after integration; no claim is made that production was changed.

## Implementation

- `TmuxRuntime::paste_text_named` streams bytes to a uniquely named buffer with `load-buffer` stdin, then uses `paste-buffer -p -d -r`. No task text is interpolated into shell code. `-p` respects the application's advertised paste mode. Failed sends remove their buffer.
- All submitted tmux text, including `send_to_agent` and verified mail injection, uses this primitive. Multi-line or large unsubmitted literal input also uses it. Individual keyboard events retain their literal behavior. Generic non-tmux backends do not invent escape framing.
- Spawn waits for a bounded empty, idle composer before typed recovery or effort input. It checks the first 40 characters with ANSI/whitespace normalization, retries only at an empty composer, and returns `spawn_warnings` on uncertainty. Argument recovery is attempted once. A failed/partial paste is not blindly submitted or duplicated. Startup polling has an eight-second budget below the normal 15-second RPC timeout; underlying backend I/O retains its existing synchronous behavior.
- The desktop keeps warnings visible and disables repeat spawning after success. MCP already serializes the complete RPC result as tool text; its spawn description now directs callers to inspect `spawn_warnings`.
- Migration 0030 adds durable claims keyed by `(message_id, window)`, inserted atomically before terminal I/O. A verified skip releases the claim because no input was written. A sent or uncertain attempt retains it, preventing concurrent callers, later sweeps, and restarts from replaying the body. Exact recipient-window matching remains required. Failed verification raises attention immediately.
- Mail frames carry a message-specific closing receipt. Before recording delivery, the daemon verifies the opening frame identifier in the pane. Bodies above 512 collapsed bytes become an explicit `[BODY OMITTED]` notice that asks the recipient to read `message_inbox(unread_only:false)`. The original body remains complete in storage. This bounded notice is a mail transport choice, not a limit on task prompts or stored reports.

## Regression and runtime evidence

- Scripted delayed startup reproduces a missing head and proves readiness-gated delivery preserves the full task; timeout sends nothing; retry/verification failure paths are covered.
- Launch argument tests preserve quotes, dollar signs, backticks, newlines, and Unicode. Claude/Codex task specs require the `--` boundary. Installed CLI help confirms both accept positional prompts.
- Real tmux fixtures exercise **2,101 bytes** and a multi-paragraph Unicode payload above 20 KB for Claude, Codex, and Antigravity mode profiles, with paste mode both enabled and disabled. Enabled clients receive one exact framed paste; disabled clients receive exact unframed bytes, without literal escape wrappers.
- Real Codex advertised bracketed paste and displayed no literal wrapper text. Its external-editor export could not be obtained, so full-body correctness there is covered by the protocol fixtures, not claimed from the live pane. Real Antigravity advertised bracketed paste but did not reach a ready composer within the probe budget.
- A concurrent automatic/forced **2,101-byte mail** test injects once, verifies its notice, and recovers the exact complete report from the delivered inbox. An uncertain head-check test never replays and leaves the full body recoverable. Store tests prove claims survive reopening the database and reject a different recipient window.
- An isolated spawn beside an existing idle test window allocates another slot and preserves the original process. The controller withdrew the adoption hypothesis after the variadic-option finding.
- Desktop tests cover warning visibility and prevention of duplicate spawn. The scoped UI detector reported no findings.

Reproduction tools: `qa/measure-spawn-input.py`, `qa/measure-composer.py`, and `qa/measure-other-composers.py`. Raw real-client measurements are in the adjacent JSONL files. Run the probes with permission to create isolated PTYs; no production daemon is required.

## Validation

All required gates passed on the final source state:

| Command | Exit | Result |
| --- | ---: | --- |
| `cargo test --workspace` | 0 | 1351 tests passed, 2 ignored across 41 test groups |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 | No warnings |
| `cargo fmt --all --check` | 0 | Clean |
| `bun run check` in `apps/desktop` | 0 | TypeScript clean |
| `bun run test` in `apps/desktop` | 0 | 109 files, 1,161 tests passed |
| `bun run bindings:check` in `apps/desktop` | 0 | No generated binding diff |

The initial worktree lacked desktop dependencies; `bun install --frozen-lockfile` with temporary cache paths resolved that without lockfile changes. Two new test-fixture failures were corrected before the final gates: a report exceeded the existing 8 KiB storage cap, and a pagination test incorrectly assumed subsecond stored timestamps. The fixtures now use valid report sizes and deterministic timestamps. Existing ts-rs attribute parsing notices occurred during binding generation; binding verification exited successfully.

## Limits and rollout

No merge, push, bundle build, installation, or production restart was performed. Slash-only effort settings apply when the launched session becomes idle; if it is still busy within the startup budget, the caller receives a warning and the first task uses its launch effort. A collapsed/hidden task that cannot be confirmed can produce a conservative warning. Durable at-most-once push attempts intentionally prefer inbox recovery to automatic replay after an ambiguous write or a crash between claim and injection. Post-integration checks should confirm the controller's deployed inbox identity/routing and absence of duplicate mail.
