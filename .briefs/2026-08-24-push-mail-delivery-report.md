# Push mail delivery report

Branch: `feat/push-mail-delivery`, based on `chore/dead-code-cleanup`.

## Root cause

Fleet mail had two independent delivery paths. The ordinary worker polled once per
second and deliberately skipped supervised lanes. Supervision then polled the whole
queued-message table every two seconds, grouped only by lane, and treated any queued
row as outstanding work. That path did not apply `message_inject_agents` /
`message_inject_operator`, did not require the queued recipient's exact window, and
sent the lane policy's generic nudge. Agent-to-agent mail blocked by policy therefore
stayed queued forever and caused unrelated windows in the same lane to receive
"Check your repomon mail...". The supervision snapshot gate also relied on the
previous snapshot; the step now refreshes before checking the master switch.

## Implementation

- `message.send` broadcasts `event.message.stored` and wakes a durable delivery
  worker immediately. The worker retains a one-second fallback sweep.
- The worker queries only sender classes currently allowed for pane injection, resolves
  each message to its exact recipient window/session, checks `injection_eligible`, and
  uses the verified injector. Busy panes, dialogs, stale panes, unresolved sessions,
  policy-blocked mail, and send failures remain queued.
- Overlay refreshes track observed pane eligibility and wake delivery on a busy-to-idle
  transition. At most one message is attempted per pane per overlay snapshot.
- Real send failures persist `delivery_error`; the second repeated failure emits one
  `needs_you` attention notification, while the durable message continues retrying.
- The old lane-wide supervision mail phase and its generic mail nudge are gone. Stall
  supervision now considers only explicit `expect_work`, never queued mail.
- The old 1,000-character body cap is gone. The complete sanitized body and closing
  `[END REPOMON MAIL]` marker are injected. No schema or RPC surface changed.
- Removed the now-unused store polling helpers.

## Verification

In an isolated config/data/socket/tmux instance (tmux server `rmv-push-yQK6I3`):

- Five immediate sends reached a raw-mode fake TUI in 154–215 ms end-to-end,
  including CLI startup and the injector's intentional 80 ms paste-settle delay.
- An 8,192-byte message arrived as an exact 8,290-byte frame with both markers intact.
- With supervision disabled, the isolated DB recorded zero `stall` audit rows.

Passed: focused mail tests (7), supervision tests (18), store policy-filter test (1),
workspace Clippy with `-D warnings`, desktop Vitest (46 files / 403 tests), and desktop
TypeScript check.

`cargo test --workspace --locked` compiled successfully and reached the core suite:
337 passed, 15 failed. Those 15 failures were environment-only socket/tmux permission
denials from the sandbox; the elevated rerun was rejected by the environment approval
quota. The affected feature tests and isolated runtime verification passed.
