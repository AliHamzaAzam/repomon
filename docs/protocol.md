# Daemon protocol

JSON-RPC 2.0 over a Unix domain socket. Each message is length-prefixed: a **4-byte
little-endian `u32`** length, then that many bytes of UTF-8 JSON.

- **Socket:** `/tmp/repomon-$USER.sock` (macOS) or `$XDG_RUNTIME_DIR/repomon.sock` (Linux),
  overridable via config or `--socket`.
- **Requests** carry an integer `id`; the daemon replies with a matching `Response`.
- **Events** are notifications (no `id`) with a method of the form `event.<topic>`. A client
  must send `subscribe` once to start receiving them.

Test it by hand: `nc -U /tmp/repomon-$USER.sock` and send framed JSON, or use the `repomon`
CLI which speaks this protocol.

## Remote transport (WebSocket)

Companion apps (the iOS client) reach the daemon over a **WebSocket bridge** speaking the
exact same JSON-RPC envelopes — one WS *text frame* per message, no length prefix. Disabled by
default; `repomon remote enable` generates a bearer token, detects the Tailscale address, and
writes `[remote] enabled/bind/token` to the config (apply with a daemon restart; pair a phone
with `repomon remote pair`, which renders a `repomon://<host:port>#<token>` QR).

- **Auth:** checked before the upgrade completes — `Authorization: Bearer <token>` header or a
  `?token=<token>` query parameter; anything else gets a 401 and no connection.
- **Bind it privately** (the Tailscale IP). The bridge is full-control: anyone holding the
  token can read panes and type into agents.
- `ping` → `"pong"` serves as an application-level keep-alive; events flow after `subscribe`
  exactly as on the Unix socket.

## Envelope

```jsonc
// request
{ "jsonrpc": "2.0", "id": 1, "method": "lane.list", "params": null }
// response (ok)
{ "jsonrpc": "2.0", "id": 1, "result": [ /* … */ ] }
// response (error)
{ "jsonrpc": "2.0", "id": 1, "error": { "code": -32601, "message": "method not found: x" } }
// event notification
{ "jsonrpc": "2.0", "method": "event.agent.output", "params": { "lane_id": 7, "content": "…" } }
```

Error codes: `-32700` parse error, `-32601` method not found, `-32602` invalid params,
`-32000` internal.

## Methods

| Method | Params | Result |
|---|---|---|
| `repo.list` | — | `[Repo]` |
| `repo.add` | `{ path }` | `Repo` |
| `repo.remove` | `{ repo_id }` | `null` |
| `repo.discover` | `{ root, max_depth=4 }` | `[String]` (repo paths) |
| `lane.list` | — | `[Lane]` (agent sessions overlaid) |
| `lane.get` | `{ lane_id }` | `Lane` |
| `lane.create` | `CreateLaneParams` | `Lane` |
| `lane.delete` | `{ lane_id, also_delete_branch=false }` | `null` |
| `lane.focus` | `{ lane_id }` | `{ path }` |
| `lane.merge` | `{ lane_id, into? }` | `{ message }` |
| `commit.today` | — | `[Commit]` (live, all repos) |
| `commit.range` | `{ from_iso, to_iso, repo_ids? }` | `[Commit]` |
| `commit.search` | `{ query, limit=50 }` | `[Commit]` (indexed) |
| `commit.recent` | `{ lane_id? \| repo_id?, limit=8 }` | `[Commit]` (latest on the worktree/repo HEAD, any date) |
| `timeline` | `{ from_iso, to_iso, bucket_secs=3600 }` | `TimelineData` |
| `sessions` | `{ from_iso, to_iso }` | `[WorkSession]` |
| `agent.detect` | — | `[AgentChoice]` (one Claude entry per config dir + codex/aider + config customs; `default` flags the configured default) |
| `agent.adopt` | `{ lane_id }` | `{ lane_id, window }` (take over an external session: resume it in a managed lane, account-aware) |
| `agent.add` | `{ name, command }` | `null` (upsert a custom agent; rejects built-in names; persists to config.toml) |
| `agent.remove` | `{ name }` | `null` (drop a custom agent; clears it as default; rejects built-ins) |
| `agent.set_default` | `{ name? }` | `null` (set/clear the New Lane default; `name` may be a built-in or custom) |
| `agent.spawn` | `{ lane_id, agent, task? }` | `{ lane_id, window, agent }` |
| `agent.capture` | `{ lane_id, lines?, window? }` | `{ content }` (ANSI-colored; `window` captures one agent in a multi-agent lane) |
| `agent.transcript` | `{ lane_id, session_id?, limit=50 }` | `[TranscriptItem]` — `{ role, text, at? }` with role `user`/`assistant`/`tools`; full unwrapped message text for clients that lay text out themselves (the mobile chat view). Claude sessions only (empty otherwise). |
| `agent.send_input` | `{ lane_id, text, enter=true, window? }` | `null` (types text, then Enter unless `enter=false`; `window` targets one agent in a multi-agent lane) |
| `agent.key` | `{ lane_id, key, literal=false, window? }` | `null` (one keystroke: literal char or key name; `window` targets one agent in a multi-agent lane) |
| `agent.signal` | `{ lane_id, key, window? }` | `null` |
| `agent.stop` | `{ lane_id, window? }` | `null` (stops one specific agent window; `None` = the lane's first slot) |
| `agent.pin` | `{ lane_id, pinned }` | `null` |
| `session.rename` | `{ session_id, label? }` | `null` (set/clear a user label for a session, keyed by its durable transcript id; empty/absent `label` clears it; overlaid onto `AgentSession.custom_label`) |
| `agent.target` | `{ lane_id, window? }` | `{ target, available }` (also resets the window to follow the attaching client's size) |
| `agent.resize` | `{ lane_id, cols, rows, window? }` | `null` (resize the agent's pane so the mediated view reflows to fit; clamped to a floor) |
| `agent.scroll` | `{ lane_id, up, ticks=1, window? }` | `{ forwarded }` (forward `ticks` wheel events to a full-screen agent so it scrolls its own history; `forwarded:false` when the pane isn't on the alternate screen — the client then scrolls the captured buffer itself) |
| `terminal.open` | `{ lane_id }` | `{ id, target }` (a new plain shell window in the worktree) |
| `terminal.list` | `{ lane_id }` | `[String]` (open terminal window names for the lane) |
| `terminal.close` | `{ id }` | `null` |
| `terminal.target` | `{ id }` | `{ target, available }` |
| `fs.browse` | `{ path? }` | `BrowseResult` (subdirs, repos, added flags) |
| `viewport.set` | `{ lane_ids, focus_lane?, focus_window? }` | `null` (`focus_lane`/`focus_window` pick which agent window the focused lane streams; others stream their first slot) |
| `subscribe` | `{ topics? }` | `null` |
| `ping` | — | `"pong"` (remote keep-alive / connectivity probe) |
| `push.register` | `{ device_token }` | `null` (register an APNs device for push; idempotent) |
| `push.unregister` | `{ device_token }` | `null` |
| `daemon.status` | — | `{ uptime_secs, repos, lanes, db_size_bytes, version }` |
| `daemon.shutdown` | — | `null` |
| `usage.get` | — | `[AccountUsage]` (per agent account, scraped from Claude `/usage` and Codex `/status`; empty unless `usage_probe` is enabled and a TUI is attached) |

`CreateLaneParams`: `{ repo_id, branch, source_branch?, path?, copy_files? }`.

`AccountUsage`: `{ key, label, report: UsageReport, age_secs }` — `key` is how a client attributes
usage to the focused agent: a Claude agent's config dir (`"default"` for `~/.claude`), or `"codex"`.
`UsageReport`: `{ windows: [UsageWindow] }`. `UsageWindow`: `{ label, pct_used, reset_at? }` — one
limit window, normalized to **% used** across agents (Codex's "% left" is converted). `label` is a
short tag (`5h`, `wk`, `mo`, or a model name); windows are ordered shortest-first and only present
when readable (a partial parse still returns what it could).

## Events

| Topic | Params |
|---|---|
| `event.repo.added` | `{ repo }` |
| `event.repo.removed` | `{ repo_id }` |
| `event.repo.changed` | `{ path, kind? }` |
| `event.lane.created` | `{ lane }` |
| `event.lane.deleted` | `{ lane_id }` |
| `event.agent.status` | `{ lane_id, status }` |
| `event.agent.output` | `{ lane_id, content, cursor? }` (`cursor` is `[col, row]` — the pane's text-cursor position, 0-based from the pane's top-left — sent only for the focused lane when its cursor is visible; `null`/absent otherwise) |
| `event.agent.changed` | `{ name }` or `{ default }` (a custom agent was added/removed, or the default changed) |
| `event.notification` | `{ lane_id, session_id?, kind, title, body, prompt? }` — daemon-side agent alert (kinds: `needs_you`, `rate_limited`, `resumed`, `idle`; `prompt` is the agent's pending question verbatim). Emitted only while `[remote]` is enabled; the same alert goes to APNs devices with category `AGENT_PROMPT` (actionable) or `AGENT_ALERT`. |

Object ids travel as lowercase hex strings; timestamps as RFC3339 UTC.
