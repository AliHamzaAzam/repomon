# Fleet messaging

Fleet messaging provides durable local communication between managed agents, Repomind and the operator. SQLite stores
the messages; terminal injection is a delivery aid. Inbox access remains available when injection is disabled or cannot
run.

## Contents

| Topic | Sections |
|---|---|
| Addressing | [Addresses](#addresses), [threads](#persistence-and-threads) |
| Delivery | [Limits](#validation-and-rate-limits), [delivery](#delivery) |
| Interfaces | [RPC](#local-rpc), [MCP](#mcp-attachment-and-capability-boundaries), [CLI/TUI](#cli-and-terminal-ui), [desktop](#desktop-behavior) |
| Backends | [Degradation](#capability-degradation), [matrix](#observed-backend-matrix) |

## Addresses

The canonical address forms are:

| Address | Meaning |
|---------|---------|
| `lane-<id>` | The first agent slot in the lane. A UI may expand this shorthand to its currently selected slot before sending. |
| `lane-<id>/<slot>` | A specific 1-based agent slot in the lane. |
| `@<label>` | The agent session whose persisted label exactly matches `<label>`. Duplicate exact labels are ambiguous and are rejected. |
| `repomind` | The active orchestrator identity. |
| `operator` | The human identity used by the local CLI and desktop or TUI actions. Agents cannot claim this address. |

Send-time resolution stores the requested address and resolved sender/recipient identities, so slot addresses keep
identifying the original recipient session after lane reordering. Missing lanes, slots or labels, ambiguous labels and
unavailable Repomind identities reject the send without creating a message.

### Multi-recipient and wildcard `to`

`message.send`'s `to` also accepts a JSON array of any of the address forms above, or a wildcard:

| `to` | Meaning |
|------|---------|
| `"lane-2/1"` | A single address - unchanged pre-existing behavior. Returns a bare `FleetMessage`. |
| `["lane-2/1", "lane-3/1"]` | Fan out one message to each address, deduplicated. Returns a per-recipient summary (below). |
| `"lane-2/*"` | Every active agent session in lane 2. |
| `"*"` | Every active agent session in the fleet. |

Wildcards exclude the sender’s session; explicit self-addresses, either alone or in an array, deliver normally. Only a
single plain address returns a bare `FleetMessage`. Lists and wildcards return this summary, even when they resolve to
one recipient:

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

`status` is `sent`, `no_such_session` (no live recipient), or `delivery_error` (the store rejected a resolved send,
usually for rate limiting or a `reply_to` that does not reverse that recipient’s thread). Each recipient uses the
single-delivery validation, threading and rate limits independently; one rejection cannot block others.

## Persistence and threads

The daemon stores messages in its existing SQLite database. A message records its ID, requested and resolved addresses,
sender and recipient lane, window, slot and session identity where those fields apply, full body, thread ID, optional
reply ID, remaining thread hops, creation time, delivery time, read time, and the last delivery error.

Threads start with six hops. Ordinary agent replies inherit and decrement the budget; exhausted replies are refused. The
reserved `operator` identity refreshes it to six. A designated human-supervised coordinator may also refresh it when
configured by exact address or lane wildcard:

```toml
message_hop_refresh_senders = ["lane-81/3", "lane-92/*"]
```

No lane sender is trusted by default. Exact entries affect only that slot, while `lane-<id>/*` affects every slot in the
named lane. When a sender writes to a recent inbound peer without `reply_to`, the daemon automatically links the send to
that recent thread. This prevents an ordinary agent pair from evading the hop limit by repeatedly starting roots.

Messages and MCP identities are separate tables. Every spawned managed agent gets a random MCP identity token while the
daemon holds the spawn lock. Only a cryptographic hash of that token is stored. The plaintext token exists only in the
spawned process environment and is never returned by RPC, written to logs, or placed in a repository.

## Validation and rate limits

Message bodies must be valid UTF-8 after transport decoding, contain at least one non-whitespace character, and be at
most 8 KiB. Bodies are retained verbatim in SQLite.

Each resolved sender may create ten messages in a rolling minute, with no more than three in the initial burst. Rate
limiting applies before insertion. The six-hop thread limit is independent of the sender rate limit.

Agent-to-agent terminal injection is disabled by default. Injection from `repomind` and `operator` is enabled by
default. These policies affect only terminal injection. Every accepted message is stored and visible in the recipient
inbox regardless of policy.

## Delivery

A daemon worker retries queued messages. Terminal injection is eligible only when the recipient is a live managed window
and its current overlay is Waiting, Idle, or at an ended turn. Injection is blocked while the recipient is working, has
a pending permission or decision dialog, is rate limited, or is stalled. Recipient-state blocks remain queued without
losing their place; a sender policy block records a durable `delivery_error` so it is distinguishable from retryable
pending mail.

The injected text is one compact line. Control characters are removed and whitespace is collapsed for this line, while
the full original body remains in SQLite:

```text
[REPOMAIL id=<id> from=<address> reply_to=<id>] <body> [END REPOMAIL]
```

`reply_to` is `none` for a root message. Successful injection sets `delivered_at`. Polling an inbox also marks returned
queued messages delivered because the recipient has obtained their contents through the durable channel. Reading and
delivery are separate transitions. Delivery failures are recorded and retried when safe; they do not delete the message.

## Local RPC

Messaging methods are available only on the local daemon socket. They are not added to the remote bridge allowlist.

| Method | Parameters | Result |
|--------|------------|--------|
| `message.send` | `{ to: string \| string[], body, reply_to? }` | `FleetMessage` for a single plain address; a per-recipient fan-out summary for a list or wildcard `to` (see [Multi-recipient and wildcard `to`](#multi-recipient-and-wildcard-to)) |
| `message.inbox` | `{ unread_only?, limit?, before? }` | `MessagePage` |
| `message.mark_read` | `{ id }` | `FleetMessage` |
| `message.list` | `{ lane_id?, unread_only?, limit?, before? }` | `MessagePage` |

Agent MCP calls use their connection identity; local operator clients use `operator`, and orchestrator MCP calls use
`repomind`. Pagination is newest first, with an opaque `before` message-ID cursor and a daemon-defined maximum `limit`.

## MCP attachment and capability boundaries

Managed agents receive a restricted agent-mode `repomond mcp` server with these tools:

| Purpose | Tools |
|---|---|
| Fleet messaging | `fleet_status`, `message_send`, `message_inbox`, `message_mark_read` |
| Read-only supervision for the caller’s lane | `supervision_status`, `supervision_audit` |

Agent mode cannot call Repomind's mutating fleet tools. Each request uses the inherited identity token; the daemon resolves its stored hash to the spawned session.

| Backend | MCP registration and limits |
|---|---|
| Claude and Codex | Launch builders add the server without replacing user MCP configuration. |
| OpenCode | Runtime-only `OPENCODE_CONFIG_CONTENT` merge preserves higher-precedence managed settings. |
| Antigravity | Merges only the token-free `mcpServers.repomon` entry into its global registry. Identity stays in inherited environment; no repository MCP file is created. |
| Cursor | Same token-free merge into global `~/.cursor/mcp.json`; `--approve-mcps` is available for headless/non-interactive spawns. |
| Aider | No native MCP client, so fleet mail tools cannot reach the server. Token and socket still pass through the environment for a future MCP-capable version. |
| Custom | Binary-name inspection gives known wrappers (such as `claude` or `agy`) that backend's wiring. Unknown binaries get no MCP registration; identity environment variables are always set. |

Repomind retains its orchestrator tools and the same messaging tools under the `repomind` identity. Missing, revoked or
mismatched agent identities may list the restricted catalog but cannot read or send messages.

## CLI and terminal UI

The human CLI uses the reserved operator identity:

1. Find the recipient’s lane and slot.

   ```sh
   repomon lane list
   ```

2. Replace `lane-2/1` with its actual address, send a message, then inspect delivery and read state.

   ```sh
   repomon msg send lane-2/1 "Please review the navigation diff."
   repomon msg list
   ```

`msg list` shows durable messages and their delivery and read state. The TUI extends Notifications with mail rows.
Opening a mail row marks it read; a jump action focuses the resolved recipient lane and slot when that target still
exists. Mail remains readable if the target session has ended.

## Desktop behavior

The Repomail panel (`mod+8`) manages fleet mail in the right rail; Control Center also shows the message feed beside
notifications. Repomail supports composing mail, per-recipient delivery results, force-send, and deletion. It shows
unread counts per recipient lane, delivery and read state, and click-to-jump behavior. A newly stored message uses its
message ID as the sole deduplication key, produces one native notification, and schedules the incoming-message cue from
the sound service when sound policy permits it. Reconnects and repeated daemon events do not notify twice.

## Capability degradation

Durability does not depend on terminal injection or a specific agent backend. If a backend cannot load the restricted
MCP server, its sessions can still receive operator and repomind messages in the fleet inbox, but they cannot call
messaging tools themselves. If a backend has no trustworthy idle or attention signal, automatic injection remains
queued. If a session is external or no longer has a managed window, messages stay stored and can be read through the
CLI, TUI, or desktop.

Backend documentation and the capability matrix must distinguish MCP access, safe injection, inbox-only delivery, and
unsupported behavior based on live verification.

## Observed backend matrix

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
