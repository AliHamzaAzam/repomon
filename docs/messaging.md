# Fleet messaging

Send one message to an active agent with `repomon msg send`, then inspect it with `repomon msg list`. Fleet mail is durable local communication between managed agents, Repomind and the operator.

**You are here:** addressing and delivery rules. A terminal injection is a delivery aid; the inbox is the durable record.

## Find your next task

| Task | Go to |
|---|---|
| Addresses | [Open section](#addresses) |
| Persistence and threads | [Open section](#persistence-and-threads) |
| Validation and rate limits | [Open section](#validation-and-rate-limits) |
| Delivery | [Open section](#delivery) |
| Local RPC | [Open section](#local-rpc) |
| MCP attachment and capability boundaries | [Open section](#mcp-attachment-and-capability-boundaries) |
| CLI and terminal UI | [Open section](#cli-and-terminal-ui) |
| Desktop behavior | [Open section](#desktop-behavior) |
| Capability degradation | [Open section](#capability-degradation) |
| Observed backend matrix | [Open section](#observed-backend-matrix) |

<details>
<summary>Why this guide exists</summary>

Fleet messaging is a durable, local communication channel between managed agents, repomind, and
the human operator.

SQLite is the source of truth.

Terminal injection is only a delivery aid, not the message store, and a message remains
available in an inbox even when injection is disabled or cannot run.

</details>

## Addresses

Choose a canonical recipient from the table.

| Address | Meaning |
|---------|---------|
| `lane-<id>` | The first agent slot in the lane. A UI may expand this shorthand to its currently selected slot before sending. |
| `lane-<id>/<slot>` | A specific 1-based agent slot in the lane. |
| `@<label>` | The agent session whose persisted label exactly matches `<label>`. Duplicate exact labels are ambiguous and are rejected. |
| `repomind` | The active orchestrator identity. |
| `operator` | The human identity used by the local CLI and desktop or TUI actions. Agents cannot claim this address. |

<details>
<summary>Details</summary>

The canonical address forms are:

Addresses are resolved at send time.

Each stored message keeps both the requested address and the resolved sender and recipient
identity.

Slot addresses therefore continue to identify the session that received the message even if lane
ordering later changes.

A missing lane, missing slot, missing label, ambiguous label, or unavailable repomind identity
is a send error and does not create a message.

</details>

### Multi-recipient and wildcard `to`

Use a list or wildcard only when you intend to mail several recipients.

| `to` | Meaning |
|------|---------|
| `"lane-2/1"` | A single address - unchanged pre-existing behavior. Returns a bare `FleetMessage`. |
| `["lane-2/1", "lane-3/1"]` | Fan out one message to each address, deduplicated. Returns a per-recipient summary (below). |
| `"lane-2/*"` | Every active agent session in lane 2. |
| `"*"` | Every active agent session in the fleet. |

```json
{
  "recipient_count": 2,
  "sent_count": 1,
  "results": [
    { "to": "lane-2/1", "status": "sent", "message_id": "…", "thread_id": "…" },
    { "to": "lane-3/1", "status": "no_such_session", "error": "…" }
  ]
}
```

<details>
<summary>Details</summary>

`message.send`'s `to` also accepts a JSON array of any of the address forms above, or a
wildcard:

Wildcard expansion always excludes the sender's own session - a broadcast never mails itself -
but an *explicit* self-address (in a plain single `to`, or listed by name inside an array) still
delivers normally.

A single plain address is the only shape that returns a bare `FleetMessage`; every list or
wildcard `to` - even one that expands to a single recipient - returns:

`status` is one of `sent`, `no_such_session` (the address didn't resolve to a live session), or
`delivery_error` (the address resolved, but the store rejected the send - most commonly the
existing sender rate limit, or an explicit `reply_to` that doesn't reverse that particular
recipient's thread).

Each recipient reuses the single-delivery path - the same validation, threading, and rate
limiting described below - as if it had been sent to individually; one recipient's rejection
never blocks the others.

</details>

## Persistence and threads

Use `reply_to` for a reply; let the daemon enforce the thread’s hop budget.

```toml
message_hop_refresh_senders = ["lane-81/3", "lane-92/*"]
```

<details>
<summary>Details</summary>

The daemon stores messages in its existing SQLite database.

A message records its ID, requested and resolved addresses, sender and recipient lane, window,
slot and session identity where those fields apply, full body, thread ID, optional reply ID,
remaining thread hops, creation time, delivery time, read time, and the last delivery error.

The first message in a thread starts with six remaining hops.

Ordinary agent replies inherit the thread and decrement that budget; a reply is refused once it
is exhausted.

Replies authored by the reserved `operator` identity refresh the budget to six.

An explicitly designated, human-supervised coordinator can do the same when its canonical
address (or its lane wildcard) is configured:

No lane sender is trusted by default.

Exact entries affect only that slot, while `lane-<id>/*` affects every slot in the named lane.

When a sender writes to a recent inbound peer without `reply_to`, the daemon automatically links
the send to that recent thread.

This prevents an ordinary agent pair from evading the hop limit by repeatedly starting roots.

Messages and MCP identities are separate tables.

Every spawned managed agent gets a random MCP identity token while the daemon holds the spawn
lock.

Only a cryptographic hash of that token is stored.

The plaintext token exists only in the spawned process environment and is never returned by RPC,
written to logs, or placed in a repository.

</details>

## Validation and rate limits

Keep each message nonempty, at most 8 KiB, and within the sender limit.

<details>
<summary>Details</summary>

Message bodies must be valid UTF-8 after transport decoding, contain at least one non-whitespace
character, and be at most 8 KiB.

Bodies are retained verbatim in SQLite.

Each resolved sender may create ten messages in a rolling minute, with no more than three in the
initial burst.

Rate limiting applies before insertion.

The six-hop thread limit is independent of the sender rate limit.

Agent-to-agent terminal injection is disabled by default.

Injection from `repomind` and `operator` is enabled by default.

These policies affect only terminal injection.

Every accepted message is stored and visible in the recipient inbox regardless of policy.

</details>

## Delivery

Check delivery state before retrying; queued mail remains in the inbox.

```text
[REPOMAIL id=<id> from=<address> reply_to=<id>] <body> [END REPOMAIL]
```

<details>
<summary>Details</summary>

A daemon worker retries queued messages.

Terminal injection is eligible only when the recipient is a live managed window and its current
overlay is Waiting, Idle, or at an ended turn.

Injection is blocked while the recipient is working, has a pending permission or decision
dialog, is rate limited, or is stalled.

Recipient-state blocks remain queued without losing their place; a sender policy block records a
durable `delivery_error` so it is distinguishable from retryable pending mail.

The injected text is one compact line.

Control characters are removed and whitespace is collapsed for this line, while the full
original body remains in SQLite:

`reply_to` is `none` for a root message.

Successful injection sets `delivered_at`.

Polling an inbox also marks returned queued messages delivered because the recipient has
obtained their contents through the durable channel.

Reading and delivery are separate transitions.

Delivery failures are recorded and retried when safe; they do not delete the message.

</details>

## Local RPC

Call these methods over the local daemon transport.

| Method | Parameters | Result |
|--------|------------|--------|
| `message.send` | `{ to: string \| string[], body, reply_to? }` | `FleetMessage` for a single plain address; a per-recipient fan-out summary for a list or wildcard `to` (see [Multi-recipient and wildcard `to`](#multi-recipient-and-wildcard-to)) |
| `message.inbox` | `{ unread_only?, limit?, before? }` | `MessagePage` |
| `message.mark_read` | `{ id }` | `FleetMessage` |
| `message.list` | `{ lane_id?, unread_only?, limit?, before? }` | `MessagePage` |

<details>
<summary>Details</summary>

Messaging methods are available only on the local daemon socket.

They are not added to the remote bridge allowlist.

The connection identity supplies the sender for agent MCP calls.

Local operator clients use `operator`.

Orchestrator MCP calls use `repomind`.

Pagination is newest first, uses the opaque `before` message ID cursor, and caps `limit` to a
daemon-defined safe maximum.

</details>

## MCP attachment and capability boundaries

Use the managed agent’s inherited identity; never copy an identity token into a repository.

<details>
<summary>Details</summary>

Managed agents receive a restricted agent-mode `repomond mcp` server.

Its messaging surface provides these four tools:

- `fleet_status`
- `message_send`
- `message_inbox`
- `message_mark_read`

It also exposes the read-only `supervision_status` and `supervision_audit` tools for the caller’s own lane.

Agent mode cannot call repomind's mutating fleet tools.

The server authenticates each request with the inherited identity token and the daemon resolves
its stored hash to the spawned agent session.

Claude and Codex launch builders add the server without replacing user MCP configuration.

OpenCode merges a runtime-only `OPENCODE_CONFIG_CONTENT` object and preserves higher-precedence
managed settings.

Antigravity surgically merges the token-free `mcpServers.repomon` entry into its global MCP
registry; identity remains in inherited environment and no repository MCP file is created.

Cursor is wired the same way, merging only `mcpServers.repomon` into its global
`~/.cursor/mcp.json`; its `--approve-mcps` flag is available for headless/non-interactive
spawns.

Aider has no native MCP client support as of its current release: the identity token and socket
are still passed via the process environment (so a future Aider version that adds support would
work without a repomon change), but its fleet mail tool calls cannot reach the MCP server today.

Custom agents are inspected by binary name and receive the matching known backend's MCP wiring
when they wrap one (e.g. a custom command wrapping `claude` or `agy` gets that backend's
registration); a completely unknown binary gets no MCP registration, though the identity
environment variables are always set.

Repomind keeps its orchestrator tool surface and adds the same messaging tools under the
`repomind` sender identity.

An agent MCP process with a missing, revoked, or mismatched identity can list only the
restricted catalog and cannot read or send messages.

</details>

## CLI and terminal UI

Replace the recipient below with an active lane address before sending.

**Time: 1 minute.**

1. Find the recipient with `repomon lane list`.
2. Send with `repomon msg send lane-2/1 "Please review the navigation diff."`, replacing `lane-2/1` with the actual address.
3. Run `repomon msg list` to inspect delivery and read state.

**You know it worked when:** the message appears in the durable list, even if terminal injection is disabled.

```text
repomon msg send <address> <body>
repomon msg list
```

<details>
<summary>Details</summary>

The human CLI uses the reserved operator identity:

`msg list` shows durable messages and their delivery and read state.

The TUI extends Notifications with mail rows.

Opening a mail row marks it read; a jump action focuses the resolved recipient lane and slot
when that target still exists.

Mail remains readable if the target session has ended.

</details>

## Desktop behavior

Press `mod+8` to open Repomail and inspect delivery results.

<details>
<summary>Details</summary>

The Repomail panel (`mod+8`) manages fleet mail in the right rail; Control Center also shows the
message feed beside notifications.

Repomail supports composing mail, per-recipient delivery results, force-send, and deletion.

It shows unread counts per recipient lane, delivery and read state, and click-to-jump behavior.

A newly stored message uses its message ID as the sole deduplication key, produces one native
notification, and schedules the incoming-message cue from the sound service when sound policy
permits it.

Reconnects and repeated daemon events do not notify twice.

</details>

## Capability degradation

Check the backend matrix before depending on automatic injection or MCP mail.

<details>
<summary>Details</summary>

Durability does not depend on terminal injection or a specific agent backend.

If a backend cannot load the restricted MCP server, its sessions can still receive operator and
repomind messages in the fleet inbox, but they cannot call messaging tools themselves.

If a backend has no trustworthy idle or attention signal, automatic injection remains queued.

If a session is external or no longer has a managed window, messages stay stored and can be read
through the CLI, TUI, or desktop.

Backend documentation and the capability matrix must distinguish MCP access, safe injection,
inbox-only delivery, and unsupported behavior based on live verification.

</details>

## Observed backend matrix

Choose a backend based on the capabilities verified for this integration.

| Backend | Managed spawn | Durable MCP mail | Idle or attention | Exact resume | Repomind |
|---------|---------------|------------------|-------------------|--------------|----------|
| Claude Code | Yes | Yes | Transcript and pane | `--resume` | Yes |
| Hermes Agent | Yes | Yes, runtime MCP configuration | Pane fallback | Session resume | Yes, pane-only |
| Codex | Yes | Yes | Pane fallback | CLI session behavior only | Yes, pane-only |
| OpenCode 1.15.5 | Yes | Yes, verified without approval | SQLite finish and tool state | `--session` | Yes, pane-only |
| Antigravity 1.1.12 | Yes | Yes, global `~/.gemini/config/mcp_config.json` registration | Pane dialogs and cache identity | `--conversation` | Yes, pane-only |
| Cursor | Yes when installed | Yes, global `~/.cursor/mcp.json` registration | Pane fallback | Unsupported (relaunches fresh) | No |
| Aider | Yes when installed | Inbox only (no MCP client support) | Coarse (chat-history mtime) | Unsupported (relaunches fresh) | No |
| Custom agent | Yes | Matches the known backend it wraps, or inbox only if unknown | Pane fallback | Unsupported (relaunches fresh) | No |
