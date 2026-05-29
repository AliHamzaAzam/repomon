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
| `agent.detect` | — | `[AgentChoice]` (built-ins on PATH + config customs; `default` flags the configured default) |
| `agent.add` | `{ name, command }` | `null` (upsert a custom agent; rejects built-in names; persists to config.toml) |
| `agent.remove` | `{ name }` | `null` (drop a custom agent; clears it as default; rejects built-ins) |
| `agent.set_default` | `{ name? }` | `null` (set/clear the New Lane default; `name` may be a built-in or custom) |
| `agent.spawn` | `{ lane_id, agent, task? }` | `{ lane_id, window, agent }` |
| `agent.capture` | `{ lane_id, lines? }` | `{ content }` (ANSI-colored) |
| `agent.send_input` | `{ lane_id, text }` | `null` (types text + Enter) |
| `agent.key` | `{ lane_id, key, literal=false }` | `null` (one keystroke: literal char or key name) |
| `agent.signal` | `{ lane_id, key }` | `null` |
| `agent.stop` | `{ lane_id }` | `null` |
| `agent.pin` | `{ lane_id, pinned }` | `null` |
| `agent.target` | `{ lane_id }` | `{ target, available }` |
| `fs.browse` | `{ path? }` | `BrowseResult` (subdirs, repos, added flags) |
| `viewport.set` | `{ lane_ids }` | `null` |
| `subscribe` | `{ topics? }` | `null` |
| `daemon.status` | — | `{ uptime_secs, repos, lanes, db_size_bytes, version }` |
| `daemon.shutdown` | — | `null` |

`CreateLaneParams`: `{ repo_id, branch, source_branch?, path?, copy_files? }`.

## Events

| Topic | Params |
|---|---|
| `event.repo.added` | `{ repo }` |
| `event.repo.removed` | `{ repo_id }` |
| `event.repo.changed` | `{ path, kind? }` |
| `event.lane.created` | `{ lane }` |
| `event.lane.deleted` | `{ lane_id }` |
| `event.agent.status` | `{ lane_id, status }` |
| `event.agent.output` | `{ lane_id, content }` |
| `event.agent.changed` | `{ name }` or `{ default }` (a custom agent was added/removed, or the default changed) |

Object ids travel as lowercase hex strings; timestamps as RFC3339 UTC.
