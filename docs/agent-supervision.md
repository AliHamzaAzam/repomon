# Agent supervision

Agent supervision is a bounded, audited proxy for permission dialogs and stalls. Its conservative class-based policy can
answer routine prompts and nudge or escalate stalled agents through a single re-verified injection path. It is off by
default globally and per lane; mail delivery operates independently.

## Contents

| Topic | Sections |
|---|---|
| Policy | [Overview](#overview), [taxonomy](#the-dialog-taxonomy), [opt-in](#the-master-switch-and-per-lane-opt-in) |
| Automation | [Mail](#wake-on-mail), [stalls](#stall-nudges-and-escalation) |
| Boundaries | [Safety](#safety-guarantees), [learned rules](#learned-approval-rules-under-supervision) |
| Interfaces | [Audit](#the-supervision_log-audit-trail), [RPC](#the-rpc-surface), [MCP](#the-read-only-mcp-worker-tools) |
| Procedures | [Recipes](#live-recipes) |

## Overview

Supervision has three moving parts:

- **Classification** (`repomon-core`): every pending dialog is classified into one of nine semantic
  categories and evaluated against a policy to produce a decision: auto-approve, auto-deny, or hold.
- **The daemon watch loop** (`repomon-daemon`): a 2-second tick answers classified dialogs, and
  nudges/escalates stalled ones, all through one verified-send module that re-checks pane state
  immediately before typing anything.
- **Surfaces**: a JSON-RPC `supervision.*` namespace, a read-only MCP tool pair for the agent itself, and
  desktop UI for both global defaults and per-lane overrides.

Every attempted action, sent, skipped, failed, or held, writes exactly one row to a durable audit log. Nothing
supervision does is silent.

## The dialog taxonomy

`classify_dialog` (`crates/repomon-core/src/agent/supervision.rs`) reads a pending dialog's title, question, body, and
context and matches it, in a fixed priority order, into one of nine `DialogClass` values. Classification also determines
whether the action is **repo-scoped**: provably confined to the lane's worktree/repo root, with no `..` traversal, no
`~`-prefixed path, and (for commands) every segment of a compound command matching an allowlisted binary whose path
arguments stay in scope.

Priority order (a dialog is classified as the first class whose markers match; later classes never re-match text already
claimed by an earlier one):

| Priority | Class and matching markers |
|---|---|
| 1 | `credential_access`: `token`, `api key`/`api_key`, `secret`, `.env`, `~/.aws`, `credential`, `keychain`, `password`, `ssh key`, `id_rsa`, `.pem`. |
| 2 | `push_remote`: `git push`, `gh pr`, `gh release`, `git remote`, or `git fetch`/`git pull` when a URL is present. |
| 3 | `install`: `npm install`/`npm i`, `pnpm add`, `bun add`, `cargo install`, `brew install`, `pip install`, `apt install`/`apt-get install`, `gem install`. |
| 4 | `device_access`: `osascript`, `camera`, `microphone`, `screen recording`, `screencapture`, `system_profiler`, `defaults write`. |
| 5 | `network_access`: `curl`, `wget`, `nc`, `ssh`, `http://`/`https://`. |
| 6 | `deletion`: `rm`, `unlink`, `git clean`, or a title/question containing "delete file". |
| 7 | `file_write`: a title/question containing "edit file", "create file", "apply patch", "multiedit", "write", "make this edit", or "do you want to create". |
| 8 | `command_exec`: a "Bash command" title, or text containing "run command", "run tool", "requesting permission", or "wants to run". |
| 9 | `unknown`: anything that matches none of the above. |

Scoping rules per class:

- **`command_exec`** is scoped only if the command splits (on `&&`, `||`, `;`, `|`) into segments that
  are each an allowlisted binary (`cargo`, `bun`, `npm`, `pnpm`, `yarn`, `go`, `make`, `pytest`, `rg`,
  `grep`, `ls`, `cat`, `tsc`, `vitest`, `eslint`, `python[3] -m pytest`, `sed -n`, or `git
  status`/`diff`/`log`/`add`/`commit`/`show`) whose path-like arguments all resolve inside the worktree
  or repo root.
- **`file_write`** and **`deletion`** are scoped if the extracted target path(s) resolve inside the
  worktree or repo root.
- **`network_access`** is scoped only if every URL's host is `localhost`, `127.0.0.1`,
  `registry.npmjs.org`, `crates.io`, `static.crates.io`, `pypi.org`, `files.pythonhosted.org`, or
  `https://github.com` (`nc`/`ssh` evidence is never scoped).
- **`credential_access`**, **`push_remote`**, **`install`**, **`device_access`**, and **`unknown`** are
  never repo-scoped.

### Shipping defaults

`SupervisionConfig::default()`, the conservative baseline shipped in `config.toml` and used by every lane that has not
set its own class override:

| Class | Default action |
|---|---|
| `command_exec` | Auto-approve, **repo-scoped only** |
| `file_write` | Auto-approve, **repo-scoped only** |
| `network_access` | Hold |
| `credential_access` | Hold |
| `deletion` | Hold |
| `push_remote` | Hold |
| `install` | Hold |
| `device_access` | Hold |
| `unknown` | Hold |

Nothing auto-denies out of the box. `command_exec` and `file_write` are the only classes that auto-approve by default,
and only when the action is provably repo-scoped: an out-of-scope command or edit falls back to a hold even if its class
says auto-approve (see [Safety guarantees](#safety-guarantees)).

## The master switch and per-lane opt-in

Supervision requires both `[supervision] enabled = true` in `config.toml` (Settings > Policies > Supervision) and an
enabled `lane_policies` row (the lane’s Supervision panel). The master switch overrides every lane; a missing lane
policy means disabled. `resolve()` enforces `enabled = defaults.enabled && lane.is_some_and(|l| l.enabled)`, and
`supervise_dialog` checks again before routing.

`config.toml` layout:

```toml
[supervision]
enabled = false
nudge_text = "Check your repomail and act on it."
stall_mins = 20
nudge_retries = 2

[supervision.classes]
command_exec = "auto_approve"
file_write = "auto_approve"
network_access = "hold"
```

A lane override only needs to list the fields it changes: `resolve()` merges lane
`classes`/`nudge_text`/`stall_mins`/`nudge_retries` on top of the global defaults field by field, and a class the lane
doesn't mention keeps the global default's action for that class. Overrides are stored in SQLite (the `lane_policies`
table), not in `config.toml`.

The two GUI paths:

- **Global defaults**: Settings > Policies > Supervision sub-tab
  (`apps/desktop/src/components/PolicySettings.tsx`). The master switch, the same nine-row class grid,
  default nudge text, default stall minutes, and default nudge retries. Writes go through `config.set {
  supervision: <full object> }` (read-modify-write on the whole nested struct). A footnote points at the
  per-lane panel for overrides.
- **Per-lane overrides**: the lane's own Supervision panel
  (`apps/desktop/src/components/SupervisionPanel.tsx`). A per-lane enable switch (disabled while the
  master switch is off, with a banner linking back to Settings), the same class grid scoped to this lane
  (each overridden row shows a dot and a "Reset" link back to the global default), nudge text, stall
  minutes, nudge retries, an "expect this lane to act on mail" switch, and a live activity log fed by
  `event.supervision.acted`. Writes go through `supervision.set { lane_id, ... }`, sending only the
  changed field(s).

## Wake-on-mail

1. Find the agent's address in Repomail or the fleet list.

   ```sh
   repomon lane list
   ```

2. Replace `lane-2/1` with that address and send a message.

   ```sh
   repomon msg send lane-2/1 "Please check your assigned task."
   ```

3. Check the durable message record.

   ```sh
   repomon msg list
   ```

The dedicated mail worker delivers full message bodies to supervised and unsupervised lanes alike.

It wakes on eligibility changes and has a one-second fallback sweep.

It resolves the exact recipient window, requires an eligible idle pane without a dialog or rate-limit screen, and
re-verifies the pane before submission.

A confirmed send marks the message delivered; a busy or unresolved recipient stays queued.

Backend failures record a delivery error and raise attention after two failures, once per tracked failure episode.

Only one message is attempted per window per sweep, so a second message cannot interrupt generation started by the
first.

The former supervision `mail_mode` setting, lane-wide inbox nudge and 30-second retry-once policy are obsolete. Mail
policy now uses `message_inject_agents` and `message_inject_operator`; see [Delivery](messaging.md#delivery).

The supervision audit still records mail sends with `trigger: "mail"` and `decision: "full_body"`.

## Stall nudges and escalation

Independently of mail, the watch loop's stall phase watches every eligible session (non-external, windowed, no dialog on
screen, and either `Waiting`/`Idle` or `Running` with `ended_turn`) in a supervised lane. A session counts as having
**outstanding work** only when the lane’s `expect_work` flag is set. Queued mail is handled separately and never becomes
a lane-wide stall nudge. Once a session has outstanding work, its pane has independently sat unchanged for at least
`stall_mins` (evidence-of-freeze, not just "assigned work minus activity time": a `None` pane-seen record never counts
as quiet), and its own idle time has crossed `stall_mins`, the pure `decide_stall` function drives:

1. **First nudge**: sent immediately once the threshold is crossed.
2. **Further nudges**: spaced at least 5 minutes apart, up to `nudge_retries` nudges total.
3. **Escalation**: once `nudge_retries` nudges have gone out with no activity past the last one, the
   episode is marked escalated (latched, no further nudges), a `hold` is journaled, and a `needs_you`
   notification fires once.

An episode's bookkeeping resets, so a future stall on the same window starts fresh, the moment the session's activity
moves past the last nudge, or outstanding work clears.

## Safety guarantees

Supervision is built to fail toward asking you, never toward acting past what it can verify:

- **Pre-send re-verification.** All supervision pane interaction goes through one module,
  `inject::verified_send` (`crates/repomon-daemon/src/inject.rs`). It re-captures the pane immediately
  before sending and compares it against the expectation the decision was made against: either the exact
  dialog summary that was classified, or (for mail/nudge/stall, which expect an idle pane) that no dialog
  and no usage-limit menu is currently on screen. Any mismatch, whether the dialog changed, a dialog
  appeared where none was expected, the usage-limit menu is up, the capture timed out, or the capture
  failed, skips the send instead of typing blind.
- **Never types over live generation.** The idle-pane check (`Expectation::IdleNoDialog`) is exactly what
  prevents a mail nudge or stall nudge from landing keystrokes while the agent is still generating or has
  something else on screen.
- **Always-escalate veto.** A hardcoded pattern check (`approval::is_always_escalate`: force pushes with
  `--force`/`-f`, `rm -rf`-shaped deletions, `git reset --hard`, `git clean -f`, `sudo rm`) wins over
  *any* policy, including an explicit `auto_approve` class mapping and any learned approval rule. It
  always produces a `Hold` with `PolicySource::AlwaysEscalate`.
- **Decision-class prompts are never auto-answered.** A dialog `detect_dialog` classifies as
  `PromptClass::Decision` (a genuine multiple-choice question, not a yes/no permission) always holds,
  regardless of its `DialogClass` or the configured policy.
- **Never selects a standing-permission option.** `approve_option`/`deny_option` blacklist any option
  whose text contains "always allow", "always", "for this session", "don't ask again", "persist",
  "remember this choice", "in settings", "settings.json", or "in this conversation": supervision can only
  ever pick a single-shot approve/deny, never a "remember this" grant. If the surviving candidates are
  still ambiguous (e.g. two equally plain "Yes, deploy to X" rows, or two identical "Yes" rows), no
  option is selected and the dialog holds instead of guessing.
- **Ambiguity resolves to hold, not to a guess.** Both class-level ambiguity (an
  `auto_approve`/`auto_deny` policy that can't find an unambiguous single-shot option) and scope
  ambiguity (an `auto_approve`-mapped `command_exec`/`file_write`/`deletion` that isn't provably
  repo-scoped) fall back to `Hold`.
- **Supervision-off equals today's behavior, with one documented deviation.** With supervision disabled
  (globally or per-lane), the legacy learned-rule auto-approve path
  (`notify_watch.rs::legacy_rule_auto_approve`, routine Bash commands matching a confirmed per-repo
  `ApprovalRule`) still runs exactly as it did before this feature, and it now shares the same
  `verified_send` path supervision uses. That's the one behavior change: previously this path sent a raw
  `Enter` unconditionally once a matching rule was found; it now re-captures the pane first, and if the
  dialog has since vanished (already answered, changed, or the agent moved on), it **skips instead of
  sending a stray Enter**, and the alert is not suppressed, so the human still hears about it. A lane
  under active supervision opts out of this legacy path entirely (the supervision loop's own
  `extra_allow` input covers the same learned rule instead), so the two paths never race.

## Learned approval rules under supervision

The pre-existing per-repo "learned rule" mechanism (a confirmed `(repo, command_pattern)` pair) still applies once a
lane is supervised, but narrowly: it can only lift a `Hold` to `AutoApprove`, and only for `command_exec` dialogs that
are already repo-scoped and did not already trip the always-escalate veto. It never touches `file_write`, `deletion`, or
any other class, and it never overrides an explicit `auto_deny` mapping.

## The supervision_log audit trail

Every action `verified_send` (or the `record_hold` helper for a pure hold) takes, and every action it *declines* to
take, writes exactly one row to `supervision_log` (`crates/repomon-core/migrations/0018_supervision.sql`):

```sql
CREATE TABLE supervision_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at TEXT NOT NULL,
    lane_id INTEGER NOT NULL,
    window TEXT NOT NULL,
    session_id TEXT,
    agent_kind TEXT,
    trigger TEXT NOT NULL,       -- "dialog" | "mail" | "stall" | "manual_nudge" | "legacy_rule"
    dialog_class TEXT,
    repo_scoped INTEGER,
    decision TEXT NOT NULL,      -- "approve" | "deny" | "nudge" | "full_body" | "hold" | ...
    policy_source TEXT,
    keys TEXT,                   -- JSON array of keys actually sent, when sent
    outcome TEXT NOT NULL,       -- "sent" | "skipped" | "failed" | "held"
    reason TEXT,
    subject TEXT,
    pane_excerpt TEXT
);
```

The "every action and skip is journaled" guarantee is structural, not incidental: `verified_send`'s internal `finish()`
helper is the single exit point for every branch (latch-held, capture-timeout, capture-failed, state-changed,
dialog-present, usage-limit-menu, a successful send, or a backend send failure), and it always writes the row and
broadcasts `event.supervision.acted` before returning. `record_hold` does the same for the "policy says Hold, nothing
was attempted" case. There is no supervision code path, including the legacy learned-rule fallback, that reaches a pane
without going through this module.

## The RPC surface

All methods live under `supervision.*` on the local daemon socket (`crates/repomon-daemon/src/rpc.rs`):

| Method | Parameters | Result |
|---|---|---|
| `supervision.get` | `{ lane_id? }` | `{ defaults, lane, effective }`: global `SupervisionConfig`, the lane's raw `SupervisionOverrides` row (or `null`), and the fully resolved `SupervisionPolicy` for that lane (all `null`/absent without `lane_id`) |
| `supervision.set` | `{ lane_id, enabled?, classes?, nudge_text?, stall_mins?, nudge_retries?, expect_work? }` | `{ effective }`: read-modify-write on the lane's stored row; refreshes the in-memory policy snapshot and broadcasts `event.supervision.changed` |
| `supervision.audit` | `{ lane_id?, limit?, before_id?, identity_token? }` | `{ entries }`: recent `supervision_log` rows, newest first, capped at 200 |
| `supervision.status` | `{ identity_token? }` | `{ master, lanes: [{ lane_id, enabled, last }] }`: the master switch and, per actively-supervised lane, its last logged entry |
| `supervision.nudge` | `{ lane_id, window?, text? }` | `{ outcome, entry_id, keys? }`: a manual, audited nudge through the same `verified_send` path (defaults to the lane's configured nudge text) |

An `identity_token` on `supervision.audit`/`supervision.status` (used by the restricted worker MCP server) forces the
lane filter to that identity's own lane and rejects a mismatched explicit `lane_id`: a worker can never read another
lane's supervision state through these calls.

**`supervision.set` is deliberately not remote-reachable.** The remote WebSocket bridge's method allowlist
(`crates/repomon-daemon/src/remote.rs`) includes `supervision.get`, `supervision.audit`, `supervision.status`, and
`supervision.nudge` (all either read-only or no stronger than the already-allowed `agent.send_input`), but omits
`supervision.set`, the same posture `config.set` gets: granting standing auto-approval authority is a local-only
decision, never one a paired phone can make remotely.

## The read-only MCP worker tools

The restricted `repomond mcp` server given to managed agents (`crates/repomon-mcp/src/agent.rs`) adds two supervision
tools alongside its existing `fleet_status`/`message_*` tools:

- **`supervision_status`**: this agent's own supervision status, whether the fleet master switch is on,
  and, if its lane is actively supervised, the last logged decision. Calls `supervision.status` with the
  agent's own `identity_token`; the daemon resolves the token to the agent's lane and returns only that
  lane's row.
- **`supervision_audit`**: this agent's own recent supervision decisions (auto-approve, auto-deny,
  nudge). Always scoped to the caller's own lane; there is no `lane_id` argument to request another
  lane's log.

There is **no approval power over MCP**: the catalog has no `supervision_set` or `supervision_nudge` tool, and
`AgentServer::call`'s dispatch has no arm for either name (or any spelling of them); an attempt falls through to
"unknown tool". An agent can observe its own supervision state; it cannot grant itself standing permission or nudge
itself (or anyone else) through this surface.

## Live recipes

### 1. Turn on supervision and watch a repo-scoped command auto-approve

1. Settings > Policies > Supervision → **Enable supervision**.
2. Open a lane's own Supervision panel and toggle **Supervise this lane**. Leave `command_exec` and
   `file_write` on their default `Auto-approve`.
3. Ask the agent to run something in-worktree, e.g. `cargo test -p repomon-core`. The dialog is answered
   without you touching it, and a `sent` row (`trigger: "dialog"`, `decision: "approve"`, `dialog_class:
   "command_exec"`, `repo_scoped: true`) appears in the panel's activity log within the 2-second tick.
4. Ask it to run something outside the worktree (e.g. `cd /etc && cat passwd`) or something
   force-flavored (`git push --force`). Both hold: the first because it is not repo-scoped, the second
   because it trips the always-escalate veto regardless of policy, and you still see the normal
   permission dialog in the terminal.

### 2. Wake a supervised lane on incoming mail

1. Leave the lane supervised and send a message to its exact agent address.
2. Wait for the recipient to become idle without a dialog.
3. Read the injected body or use `message_inbox`; inspect the mail audit row.

### 3. Stall past the threshold and see it escalate

1. Set the lane's `stall_mins` low (e.g. 1) and turn on **Expect this lane to act on mail**
   (`expect_work`). Despite the label, unread mail alone does not trigger stall nudges.
2. Let the agent sit idle past the threshold with its pane genuinely unchanged. A `trigger: "stall"`,
   `decision: "nudge"` row appears once the threshold and the independent pane-quiet check both agree.
3. Keep it idle through `nudge_retries` total nudges, then another 5 minutes. The episode escalates: a
   `held` row is journaled with a reason like "escalated after 2 nudges", and a `needs_you` notification
   fires once, not on every subsequent tick.
4. Send the agent any input (attach and type, or resolve the mail); the next tick's activity check clears
   the episode, so a future stall starts fresh.

### 4. Read the audit trail two ways, and confirm what each surface can and can't do

1. From the desktop app, open the lane's Supervision panel's activity log and expand a row to see its
   `pane_excerpt` and the exact `keys` sent.
2. From inside the agent itself, call the `supervision_audit` MCP tool: it returns the same log, scoped
   to this agent's own lane only, with no way to name another lane or to act on what it sees.
3. Try `supervision.set` over the remote WebSocket bridge (a paired phone/companion connection): it is
   rejected, since only `supervision.get`/`audit`/`status`/`nudge` are on the remote allowlist, so
   changing policy stays a local, in-front-of-the-machine action.
4. Turn the master switch off. Every lane's policy resolves to `enabled: false` immediately
   (`supervision.get` reflects it fleet-wide), and the watch loop's next tick stops acting on any lane,
   regardless of that lane's own `enabled` flag.
