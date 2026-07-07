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
- **Method allowlist (default-deny):** the bridge only dispatches reads and
  interaction-with-existing-agents — anything else (host management, spawning, config,
  terminal open/close, filesystem) answers `-32601` `"not permitted over remote bridge"`.
  Since v0.4.1 that includes `agent.prompt`, `agent.answer` (verified dialog steering,
  strictly safer than the blind `agent.key` it complements), `agent.watch_bytes`, and
  `terminal.list_all`; since v0.4.2 `agent.fit` replaces `agent.resize` over the bridge —
  the unconditional resize is local-only (a blind remote resize is exactly what squeezed
  the TUI's mediated view), and `agent.fit` reflows the shared pane only while no live
  TUI viewport owns it. Events themselves are forwarded unfiltered after `subscribe`. Note
  `agent.watch_bytes` is single-watch daemon-wide: a remote client and a local TUI Focus view
  contend for the one byte stream, last writer wins — the loser should fall back to
  capture-based polling.

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
| `lane.list` | — | `[Lane]` (agent sessions overlaid; each session may carry `pending_dialog`, `stale`/`stalled_since`, and `gate` — the worktree's latest dxkit stop-gate verdict `{ allowed, net_new_findings, at, session_id? }`, tailed from `.dxkit/loop/ledger.jsonl` when the lane runs dxkit's loop pack. A fresh `allowed` grants the done-candidate attention; a fresh block vetoes it) |
| `lane.get` | `{ lane_id }` | `Lane` |
| `lane.create` | `CreateLaneParams` | `Lane` |
| `lane.delete` | `{ lane_id, also_delete_branch=false }` | `null` |
| `lane.focus` | `{ lane_id }` | `{ path }` |
| `lane.merge` | `{ lane_id, into? }` | `{ message }` |
| `lane.diff` | `{ lane_id, include_patch=false, max_patch_chars=8000 }` | `LaneDiff` — commits ahead of the repo's base branch (with diffstat) plus uncommitted state; see below |
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
| `agent.watch_bytes` | `{ lane_id, window?, on }` | on `on: true`, `{ cols, rows }` — the pane's current grid (`null`s when the window is gone); `null` on `off`. Streams the pane's raw PTY bytes (tmux `pipe-pane`) as `event.agent.bytes`. Single-watch: a new `on` replaces the previous watch. Render your emulator at the authoritative grid — the ack's, or the one `agent.fit` answers with: the pane is shared, and only `agent.fit` may reflow it remotely (a local TUI re-asserts its own size within seconds). The capture-based `viewport.set` streaming is unaffected. |
| `agent.prompt` | `{ lane_id, window? }` | `{ dialog: PendingDialog\|null }` — fresh pane capture parsed for the interactive dialog actually on screen right now (never the sniff cache). `PendingDialog` = `{ title?, question, body?: [String], options: [{ number?, text }], selected? }`; `lane.list` carries the same object on `AgentSession.pending_dialog` alongside the `pending_prompt` summary. |
| `agent.answer` | `{ lane_id, choice, window?, expect_summary? }` | `{ answered, sent }` — re-captures the pane, verifies a dialog is still up (and, when `expect_summary` is set, that it still summarizes to that string), then steers to `choice` (0-based) and confirms. On a stale view it does NOT send anything: error `-32010` (`no pending dialog` / `dialog changed`) with `error.data.dialog` carrying what's actually on screen (possibly `null`) so the client re-renders instead of re-fetching. Any input path (`send_input`/`key`/`signal`/`answer`) drops the window's sniff-cache entry, so an answered dialog can't be re-advertised for the rest of its TTL. |
| `agent.stop` | `{ lane_id, window? }` | `null` (stops one specific agent window; `None` = the lane's first slot) |
| `agent.pin` | `{ lane_id, pinned }` | `null` |
| `session.rename` | `{ session_id, label? }` | `null` (set/clear a user label for a session, keyed by its durable transcript id; empty/absent `label` clears it; overlaid onto `AgentSession.custom_label`) |
| `agent.target` | `{ lane_id, window? }` | `{ target, available }` (also resets the window to follow the attaching client's size) |
| `agent.resize` | `{ lane_id, cols, rows, window? }` | `null` (resize the agent's pane so the mediated view reflows to fit; clamped to a floor) |
| `agent.fit` | `{ lane_id, cols, rows, window? }` | `{ applied, cols, rows }` — the arbitrated resize for remote viewers: reflows the shared pane to the caller's grid ONLY while no live local viewport focus owns the window (the TUI heartbeats its viewport every ~5s; ownership lapses 15s after the last beat, and a clean TUI quit releases it immediately). Refused (`applied: false`) it answers with the pane's current grid so the caller renders pinned at the shared size instead of fighting. Poll it (~10s) to adapt when the TUI starts or stops viewing. |
| `agent.scroll` | `{ lane_id, up, ticks=1, window? }` | `{ forwarded }` (forward `ticks` wheel events to a full-screen agent so it scrolls its own history; `forwarded:false` when the pane isn't on the alternate screen — the client then scrolls the captured buffer itself) |
| `terminal.open` | `{ lane_id }` | `{ id, target }` (a new plain shell window in the worktree) |
| `terminal.list` | `{ lane_id }` | `[String]` (open terminal window names for the lane) |
| `terminal.list_all` | — | `[{ lane_id, id }]` (every lane's open terminals, sorted — what the Grid tiles) |
| `terminal.close` | `{ id }` | `null` |
| `terminal.target` | `{ id }` | `{ target, available }` |
| `fs.browse` | `{ path? }` | `BrowseResult` (subdirs, repos, added flags) |
| `viewport.set` | `{ lane_ids, focus_lane?, focus_window?, windows? }` | `null` (`focus_lane`/`focus_window` pick which agent window the focused lane streams; others stream their first slot. `windows` names plain-terminal windows — `term-{lane}-{n}` — to stream as extra panes alongside the lanes, e.g. the Grid's shell tiles; non-terminal names are ignored) |
| `subscribe` | `{ topics? }` | `null` |
| `ping` | — | `"pong"` (remote keep-alive / connectivity probe) |
| `push.register` | `{ device_token }` | `null` (register an APNs device for push; idempotent) |
| `push.unregister` | `{ device_token }` | `null` |
| `daemon.status` | — | `{ uptime_secs, repos, lanes, db_size_bytes, version }` |
| `daemon.shutdown` | — | `null` |
| `usage.get` | — | `[AccountUsage]` (per agent account, scraped from Claude `/usage` and Codex `/status`; empty unless `usage_probe` is enabled and a TUI is attached) |
| `orchestrator.status` | — | `{ running, agent?, model?, backend?, window?, autonomy?, session_id?, attention, headline? }` (the daemon-owned repomind orchestrator; reconciles against tmux, so a window killed externally reports `running:false`) |
| `orchestrator.transcript` | `{ limit? }` | `[TranscriptItem]` (repomind's conversation, same `{ role, text, at? }` shape as `agent.transcript`, so a client can render it as a chat instead of mirroring the pane; pinned to the orchestrator's own `session_id` when known, else falls back to the newest `$HOME` Claude transcript with real content across accounts. Always `[]` while `backend` is `"codex"` — codex's on-disk session format is not parsed; treat it as "no chat view for this backend" and render the `event.orchestrator.output` pane stream instead, never as an error/loading state) |
| `orchestrator.start` | `{ agent?, model?, autonomy?, max_agents?, prompt? }` | `{ running, agent?, model?, backend?, window?, autonomy?, session_id?, attention, headline? }` (spawn or adopt the singleton `orchestrator` window wired to the repomon MCP server; idempotent; re-spawns if the prior window died. `agent` picks the backend: a Claude account / custom agent name / `codex`; an agent with no MCP client — e.g. `aider` — is rejected with `invalid_params` instead of spawning a broken window) |
| `orchestrator.stop` | — | `{ running:false, attention:"none", headline:null, … }` (kill the orchestrator window) |
| `orchestrator.target` | — | `{ target, available }` (attach target for the orchestrator window; resets it to follow the attaching client's size) |
| `orchestrator.send_input` | `{ text, enter=true }` | `null` (type an instruction to repomind, then Enter unless `enter=false`) |
| `orchestrator.key` | `{ key, literal=false }` | `null` (one keystroke to repomind: literal char or key name) |
| `orchestrator.watch` | `{ on }` | `null` (gate the pane stream; the TUI sets it `true` while the command-center view is open and `false` on leaving) |
| `orchestrator.resize` | `{ cols, rows }` | `null` (size the orchestrator window to the viewer's pane so its capture reflows to fit; clamped to a floor) |

The `orchestrator` window is deliberately not a `lane-*` name, so it never appears in `lane.list` / the lane overlay / the reaper. It is the in-daemon backing for `repomon orchestrate` and the TUI's command-center view.

`attention` on the orchestrator payloads above is always present: one of `"none"`,
`"permission"`, `"decision"`, `"end_of_turn"`. `headline` is non-null only alongside
`permission`/`decision` (the open dialog's question) or `end_of_turn` (a tail of repomind's
last message, when cheaply available); always `null` when `attention` is `"none"`.

`backend` is the normalized agent CLI the session runs on — `"claude"` or `"codex"` (`null`
when not running) — and is what clients should switch rendering on (`agent` is the raw
launch name, e.g. `claude-work`). A `"codex"` session is monitored best-effort from its pane
only: `orchestrator.transcript` is always `[]`, `attention` never reports `"end_of_turn"`
(pane dialogs may still surface `permission`/`decision`), and `session_id` is always `null`.

`autonomy` is the level the running session was actually started with (the value passed to, or
defaulted by, `orchestrator.start`). It is `null` when the daemon *adopted* a window that
survived a restart of a previous daemon process — the adopting process has no record of what
autonomy that window was originally launched with, so it reports unknown rather than guessing.

`session_id` is the UUID the running session's `claude` was launched with (`--session-id`,
minted fresh by the daemon at spawn time), which pins `orchestrator.transcript` and the
end-of-turn attention check to *this* session's own transcript file — instead of guessing "the
newest `$HOME` transcript", which misattributes any other active Claude session on the machine as
repomind's. Like `autonomy`, it is `null` when the daemon *adopted* a surviving window: the prior
process's session id lived only in its own memory, so an adopted session falls back to the old
newest-with-content heuristic. Always `null` for a `"codex"` backend (codex has no equivalent of
`--session-id`, and its session files aren't parsed — the fallback heuristic is deliberately NOT
applied there, since it would misattribute an unrelated Claude session's transcript).

**Remote bridge:** of the orchestrator methods above, `status`/`transcript`/`send_input`/`key`
are allowed over the WebSocket bridge (read + interact, like their `agent.*` equivalents);
`start`/`stop`/`watch`/`resize` are Unix-socket only — spawning or killing repomind, and its
pane-geometry plumbing, stay local.

`CreateLaneParams`: `{ repo_id, branch, source_branch?, path?, copy_files? }`.

`LaneDiff` (the result of `lane.diff`): `{ base, merge_base, commits, commits_truncated?,
committed_stat, uncommitted_stat, untracked, patch?, patch_truncated? }`. `base` is the repo's
main checkout's current branch name; `merge_base` a short hash of `git merge-base HEAD <base>`;
`commits` is `git log --oneline <merge_base>..HEAD`, newline-joined and capped at 20 lines
(`commits_truncated: true` present only when there were more). `committed_stat` is `git diff
--stat <merge_base>..HEAD`; `uncommitted_stat` is `git diff HEAD --stat` (staged + unstaged).
`untracked` is the lane's untracked-file *count* only — untracked file **contents** never
appear in `patch` or either `*_stat` field. `patch` (`git diff HEAD` text, capped at
`max_patch_chars`, char-boundary safe) and `patch_truncated: true` are present only when
`include_patch: true` and the patch was actually cut; `max_patch_chars` is server-clamped to a
ceiling of 20000. Errors (`-32000`) when the base branch shares no common history with `HEAD`,
or the repo's main checkout has no current branch (detached HEAD). Local Unix socket only — not
on the remote bridge allowlist.

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
| `event.agent.output` | `{ lane_id, window, content, cursor? }` (`window` names the tmux window the capture came from — a `lane-*` agent pane or a `term-*` plain terminal, so one lane can stream both without colliding; `cursor` is `[col, row]` — the pane's text-cursor position, 0-based from the pane's top-left — sent only for the focused pane when its cursor is visible; `null`/absent otherwise) |
| `event.agent.bytes` | `{ lane_id, window, data }` — raw PTY bytes (base64) from the byte-watched pane, in emission order. Chunks are arbitrary byte boundaries (not UTF-8 safe individually); feed them to a terminal emulator, don't string-parse them. |
| `event.agent.changed` | `{ name }` or `{ default }` (a custom agent was added/removed, or the default changed) |
| `event.notification` | `{ lane_id, session_id?, kind, title, body, prompt?, attention, dialog? }` — daemon-side agent alert (kinds: `needs_you`, `rate_limited`, `resumed`, `idle`, `stalled`; `prompt` is the agent's pending question verbatim). `attention` refines `needs_you`: `permission` (routine tool-call ask) / `decision` (a real question) / `done_candidate` (turn finished on a clean lane with a this-turn commit — ready to review) / `end_of_turn` (turn finished, no dialog) / `none`; `dialog` is the full `PendingDialog` when one is on screen, so an actionable client can offer its real options. `stalled` fires once when a managed agent's pane and transcript both freeze mid-work for ~5 min while its process lives (see `AgentSession.stale`/`stalled_since` on `lane.list` — additive overlay fields, with `stalled_since` marking the pane's last change). Emitted only while `[remote]` is enabled; the same alert goes to APNs devices with category `AGENT_PROMPT` (actionable) or `AGENT_ALERT`. |
| `event.orchestrator.output` | `{ content, cursor? }` — the repomind pane's text (and `[col, row]` cursor) streamed while watched; same shape as `event.agent.output` without `lane_id`. |
| `event.orchestrator.status` | `{ running, agent?, model?, backend?, window?, autonomy?, session_id?, attention, headline? }` — broadcast when the orchestrator starts, stops, is reconciled to stopped after its window died, or its `attention` changes. |

Object ids travel as lowercase hex strings; timestamps as RFC3339 UTC.
