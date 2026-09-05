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
default; `repomon remote enable` generates the legacy shared bearer token, detects the
Tailscale address, and writes `[remote] enabled/bind/token` to the config (apply with a daemon
restart).

- **Auth: per-device tokens.** `repomon remote pair --name <device>` mints that device its own
  named, individually revocable token (re-running it for the same name re-shows the same
  token rather than minting a second one; capped at 16 paired devices) and renders a
  `repomon://<host:port>#<token>&name=<device>` QR. `repomon remote devices` lists paired
  devices (name, role, created/last-seen — never the token); `repomon remote revoke <name>`
  revokes one, and a live connection authenticated with that token is kicked on its very next
  request with `-32000` `"device revoked"`. The legacy shared `[remote] token` from config
  still authenticates exactly as before (it isn't a row in `remote.devices`'s list — the CLI
  calls it out separately as the shared config token). `remote.pair`, `remote.devices`, and
  `remote.revoke` are themselves local-socket only: no client can mint or enumerate tokens
  over the same network it authenticates onto.
- **Checked before the WS upgrade completes** — `Authorization: Bearer <token>` header or a
  `?token=<token>` query parameter; anything else gets a 401 and no connection.
- **Bind it privately** (the Tailscale IP). Any paired device's token exercises the full
  allowlist below.
- `ping` → `"pong"` serves as an application-level keep-alive; events flow after `subscribe`
  exactly as on the Unix socket.
- **Method allowlist (default-deny), full fleet control:** paired devices can read the fleet,
  drive existing agents, AND spawn/stop/adopt agents and create/delete/merge lanes
  (`agent.spawn`, `agent.stop`, `agent.adopt`, `agent.detect`, `lane.create`, `lane.delete`,
  `lane.merge`, `lane.focus`, `lane.diff`), on top of everything previously allowed: reads,
  `agent.prompt`/`agent.answer` (verified dialog steering), `agent.watch_bytes`,
  `terminal.list_all`, `agent.fit`, and the orchestrator's status/transcript/send_input/key.
  Still blocked: daemon lifecycle (`daemon.shutdown`), machine diagnostics (`system.doctor`),
  config/secrets (`config.get`, `config.set`), host terminal + filesystem access
  (`terminal.open/close/target`, `fs.browse`), the worktree file-editor RPCs
  (`file.list`/`file.read`/`file.read_raw`/`file.write`, doubly so for `file.write` since it overwrites host
  files), `commit.show` (a caller-chosen oid can walk the entire repo history, unlike the
  already-allowed `lane.diff`), the blind `agent.resize` (only the arbitrated `agent.fit` is
  reachable remotely, since an unconditional remote resize is exactly what squeezed the TUI's
  mediated view), the orchestrator's `start`/`stop`/`watch`/`resize` (spawning/killing repomind
  and its pane geometry stay local), and credential minting (`remote.*`). Anything else answers
  `-32601` `"not permitted over remote bridge"`.
- **Per-connection viewports and byte watches.** Each connection (Unix socket or WebSocket)
  owns its own `viewport.set` state and its own `agent.watch_bytes` windows — an iPhone, an
  iPad, and the Mac TUI can each hold a different view (or watch the same window) at once
  without clobbering one another. The daemon polls and streams the *union* of every live
  connection's viewport. `event.agent.bytes` is delivered only to the connections watching
  that window; `event.agent.output` only to connections whose viewport covers the event's
  lane, or names its terminal window — a connection that never calls `viewport.set` receives
  no `event.agent.output` at all.
- **`agent.watch_bytes` is per-window and refcounted, not single-watch.** `on:true` starts (or
  joins) the shared backend stream for that window and always acks with `{ cols, rows }`, the pane's
  current grid. A connection may hold several windows at once; the same window watched by
  several connections shares one underlying stream, and that stream stops only once its last
  watcher leaves. `on:false` with a `window` releases
  just that window for this connection; `on:false` without a `window` releases every window
  this connection watches for the lane.
- **Fit arbitration.** A local (TUI) session's fresh viewport focus always owns a window's
  size. Among remote sessions actively focused on the same window, the one that most recently
  drove the agent (`agent.send_input`/`signal`/`key`/`scroll`/`answer`, or an applied
  `agent.fit`) wins.
  Both kinds of ownership decay: the TUI heartbeats its viewport every ~5s and a focus lapses
  15s after the last beat, so a crashed or closed client's hold releases within seconds. The
  unconditional `agent.resize` stays local-only regardless of who's connected.

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
| `lane.list` | — | `[Lane]` (each lane carries `role`: `null` for an ordinary work lane, `"controller"` for the repomind home lane. Agent sessions overlaid; each session may carry `pending_dialog`, `stale`/`stalled_since`, and `gate` — the worktree's latest dxkit stop-gate verdict `{ allowed, net_new_findings, at, session_id? }`, tailed from `.dxkit/loop/ledger.jsonl` when the lane runs dxkit's loop pack. A fresh `allowed` grants the done-candidate attention; a fresh block vetoes it) |
| `lane.get` | `{ lane_id }` | `Lane` |
| `lane.create` | `CreateLaneParams` | `Lane` |
| `lane.delete` | `{ lane_id, also_delete_branch=false }` | `null` |
| `lane.focus` | `{ lane_id }` | `{ path }` |
| `lane.merge` | `{ lane_id, into? }` | `{ message }` |
| `lane.diff` | `{ lane_id, include_patch=false, max_patch_chars=8000 }` | `LaneDiff` — commits ahead of the repo's base branch (with diffstat) plus uncommitted state; see below |
| `message.send` | `{ to: string \| string[], body, reply_to? }` — `to` also accepts `"lane-N/*"` or `"*"` | `FleetMessage` for a single plain address; a per-recipient fan-out summary (`{ recipient_count, sent_count, results: [{ to, status, message_id?, thread_id?, error? }] }`) for a list or wildcard `to` (local socket only; see `docs/messaging.md`) |
| `message.inbox` | `{ unread_only=false, limit=50, before? }` | `MessagePage`; returned queued rows become delivered (local socket only) |
| `message.mark_read` | `{ id }` | `FleetMessage` (local socket only) |
| `message.list` | `{ lane_id?, unread_only=false, limit=50, before? }` | `MessagePage`, newest first (local socket only) |
| `commit.today` | — | `[Commit]` (live, all repos) |
| `commit.range` | `{ from_iso, to_iso, repo_ids? }` | `[Commit]` |
| `commit.search` | `{ query, limit=50 }` | `[Commit]` (indexed) |
| `commit.recent` | `{ lane_id? \| repo_id?, limit=8 }` | `[Commit]` (latest on the worktree/repo HEAD, any date) |
| `commit.show` | `{ lane_id, oid, max_patch_chars=8000 }` | `CommitShow`, one commit's full metadata (`oid` resolved to the full 40-char hash, `author_name`, `author_email`, `time`, `summary`, `body`) plus `patch` (capped, char-boundary safe, `max_patch_chars` server-clamped to 20000) and `stat`; `patch_truncated` set when cut. Backs the git explorer's commit-detail view. Local socket only (a caller-chosen `oid` can walk the entire repo history, a broader read surface than `lane.diff`). |
| `timeline` | `{ from_iso, to_iso, bucket_secs=3600 }` | `TimelineData` |
| `sessions` | `{ from_iso, to_iso }` | `[WorkSession]` |
| `agent.detect` | none | `[AgentChoice]` (one Claude entry per config dir + codex/hermes/opencode/antigravity/aider/cursor + config customs; `default` flags the configured default) |
| `agent.adopt` | `{ lane_id, session_id?, agent? }` | `{ lane_id, window }` (take over an external session; every agent kind is adoptable. Claude, Hermes, OpenCode, and Antigravity resume that exact identity/session with their resume flag; Codex, Cursor, Aider, and custom agents have no session-resume flag, so adopting relaunches the same command fresh in the worktree instead. Omitted `agent` retains account-aware Claude behavior) |
| `agent.add` | `{ name, command }` | `null` (upsert a custom agent; rejects built-in names; persists to config.toml) |
| `agent.remove` | `{ name }` | `null` (drop a custom agent; clears it as default; rejects built-ins) |
| `agent.set_default` | `{ name? }` | `null` (set/clear the New Lane default; `name` may be a built-in or custom) |
| `agent.spawn` | `{ lane_id, agent, task?, effort?, mode?, model?, identity_token? }` | `{ lane_id, window, agent, role? }`. `role` is `"controller"` when `lane_id` is the repomind controller lane, and absent otherwise. A controller is launched with `REPOMON_MCP_MODE=orchestrator` (the full fleet catalog) and counted against `[repomind] max_controllers`; `identity_token` is the caller's own MCP identity, and a caller whose identity belongs to another lane is refused with `invalid_params` when it targets the controller lane. |
| `agent.capture` | `{ lane_id, lines?, window?, include_state=false }` | `{ content }` normally. With `include_state=true`, also returns `{ alternate, cols, rows, cursor?, generation?, sequence?, stable }` as a terminal checkpoint. A sequenced client must only resume with stream chunks after the checkpoint cursor and recapture when `stable=false`. |
| `agent.transcript` | `{ lane_id, session_id?, limit=50 }` | `[TranscriptItem]` — `{ role, text, at? }` with role `user`/`assistant`/`tools`; full unwrapped message text for clients that lay text out themselves (the mobile chat view). Claude sessions only (empty otherwise). |
| `agent.transcript_page` | `{ lane_id, session_id?, before? }` | `{ items: [TranscriptItem], next_before: number? }` - reads one bounded Claude transcript page backwards on JSONL boundaries. Omit `before` for the newest page, then pass `next_before` to load older pages without skipping messages. A known `session_id` is resolved directly regardless of age. |
| `agent.send_input` | `{ lane_id, text, enter=true, window? }` | `null` (types text, then Enter unless `enter=false`; `window` targets one agent in a multi-agent lane) |
| `agent.key` | `{ lane_id, key, literal=false, window? }` | `null` (one keystroke: literal char or key name; `window` targets one agent in a multi-agent lane) |
| `agent.signal` | `{ lane_id, key, window? }` | `null` |
| `agent.watch_bytes` | `{ lane_id, window?, on }` | on `on: true`, `{ cols, rows, generation, sequence }`; `null` on `off`. Streams the pane's raw PTY bytes as `event.agent.bytes`, delivered only to connections watching that window. Every chunk carries the same generation and a contiguous sequence. A gap or generation change invalidates cursor-relative emulator state and requires a fresh sequenced `agent.capture` checkpoint. Per-connection and refcounted, not single-watch: a window has one shared backend stream no matter how many connections watch it; this connection may watch several windows at once. `on:false` with a `window` releases just that one; `on:false` without a `window` releases every window this connection watches for the lane. A matching `event.agent.stream_closed` means the target window died and the client must release its local watch. Render at the authoritative grid from the ack or `agent.fit`. |
| `agent.prompt` | `{ lane_id, window? }` | `{ dialog: PendingDialog\|null }` — fresh pane capture parsed for the interactive dialog actually on screen right now (never the sniff cache). `PendingDialog` = `{ title?, question, body?: [String], options: [{ number?, text }], selected? }`; `lane.list` carries the same object on `AgentSession.pending_dialog` alongside the `pending_prompt` summary. |
| `agent.answer` | `{ lane_id, choice, window?, expect_summary? }` | `{ answered, sent }` — re-captures the pane, verifies a dialog is still up (and, when `expect_summary` is set, that it still summarizes to that string), then steers to `choice` (0-based) and confirms. On a stale view it does NOT send anything: error `-32010` (`no pending dialog` / `dialog changed`) with `error.data.dialog` carrying what's actually on screen (possibly `null`) so the client re-renders instead of re-fetching. Any input path (`send_input`/`key`/`signal`/`answer`) drops the window's sniff-cache entry, so an answered dialog can't be re-advertised for the rest of its TTL. |
| `agent.stop` | `{ lane_id, window? }` | `null` (stops one specific agent window; `None` = the lane's first slot) |
| `agent.pin` | `{ lane_id, pinned }` | `null` |
| `session.rename` | `{ session_id, label? }` | `null` (set/clear a user label for a session, keyed by its durable transcript id; empty/absent `label` clears it; overlaid onto `AgentSession.custom_label`) |
| `agent.target` | `{ lane_id, window? }` | `{ target, available, attach? }` (also resets the window to follow the attaching client's size; `attach` — optional, additive — is `{ program, args }`, the exact command to run in a real terminal to attach; clients without it keep deriving the tmux invocation from `target`) |
| `agent.resize` | `{ lane_id, cols, rows, window? }` | `null` (resize the agent's pane so the mediated view reflows to fit; clamped to a floor) |
| `agent.fit` | `{ lane_id, cols, rows, window? }` | `{ applied, cols, rows }` — the arbitrated resize for remote viewers: reflows the shared pane to the caller's grid ONLY while no live local viewport focus owns the window (the TUI heartbeats its viewport every ~5s; ownership lapses 15s after the last beat, and a clean TUI quit releases it immediately) AND no other remote session that's also focused on this window right now drove the agent more recently than the caller (last-interaction-wins among remotes; an applied fit counts as an interaction). Refused (`applied: false`) it answers with the pane's current grid so the caller renders pinned at the shared size instead of fighting. Poll it (~10s) to adapt when the TUI starts or stops viewing. |
| `agent.scroll` | `{ lane_id, up, ticks=1, window? }` | `{ forwarded }` (forward `ticks` wheel events to a full-screen agent so it scrolls its own history; `forwarded:false` when the pane isn't on the alternate screen — the client then scrolls the captured buffer itself) |
| `terminal.open` | `{ lane_id }` | `{ id, target, attach? }` (a new plain shell window in the worktree; `attach` as in `agent.target`) |
| `terminal.list` | `{ lane_id }` | `[String]` (open terminal window names for the lane) |
| `terminal.list_all` | — | `[{ lane_id, id }]` (every lane's open terminals, sorted — what the Grid tiles) |
| `terminal.close` | `{ id }` | `null` |
| `terminal.target` | `{ id }` | `{ target, available, attach? }` (`attach` as in `agent.target`) |
| `fs.browse` | `{ path? }` | `BrowseResult` (subdirs, repos, added flags) |
| `file.list` | `{ lane_id, path? }` | `FileListResult`: `{ entries: [FileEntry], truncated }`, one directory level of the lane's worktree (`path` relative to the worktree root, omitted lists the root); `FileEntry` = `{ name, path, is_dir, size, ignored }` (`ignored` from a batched `git check-ignore`; `.git` itself is never listed). Backs the in-app editor's lazy file tree. Local socket only, like `fs.browse`. |
| `file.read` | `{ lane_id, path }` | `FileReadResult`: `{ content, mtime_ms, size, truncated, kind, large }`; `kind` is "text", "binary", "image", or "pdf" (SVG is treated as text for CodeMirror editing; raster images are "image"; `.pdf` is classified before the binary sniff, with empty `content` - the editor streams the file itself via the Tauri asset protocol instead of reading it through this RPC); `large` is true when file size exceeds 2 MiB (opens read-only in editor); rejects (does not truncate) a file over the 8 MiB read cap so the editor never silently saves back a partial copy. `mtime_ms` round-trips into `file.write`'s `expected_mtime_ms`. Local socket only. |
| `file.read_raw` | `{ lane_id, path }` | `FileReadRawResult`: `{ base64, mime, size }`; returns base64-encoded file payload and MIME type under the same 8 MiB read cap. Used for image previews. Local socket only. |
| `file.write` | `{ lane_id, path, content, expected_mtime_ms? }` | `FileWriteResult`: `{ mtime_ms, size }`; given `expected_mtime_ms`, rejected with a conflict error unless the on-disk mtime still matches (omitted = last-write-wins). Broadcasts `event.file.changed`. Local socket only, doubly so since it overwrites host files. |
| `file.index` | `{ lane_id }` | `FileIndexResult`: `{ paths: [string], truncated: bool, generation: number }`; recursive list of non-ignored worktree relative file paths, files only, capped at 50,000 entries. Cached in daemon memory per lane with generation bumped on watcher events and writes. Local socket only. |
| `file.create` | `{ lane_id, path, is_dir? }` | `FileCreateResult`: `{ path, is_dir }`; creates an empty file or directory. Parent directories created automatically. Rejects with `-32009` if target already exists. Broadcasts `event.file.changed` with `op: "created"`. Local socket only. |
| `file.rename` | `{ lane_id, from, to }` | `FileRenameResult`: `{ from, to }`; renames or moves a path within worktree. Rejects with `-32009` if destination exists. Broadcasts `event.file.changed` with `op: "renamed"`. Local socket only. |
| `file.delete` | `{ lane_id, path, recursive? }` | `FileDeleteResult`: `{ path }`; deletes file or directory. Refuses root and `.git`. Non-empty directory requires `recursive: true` or rejects with `-32008`. Broadcasts `event.file.changed` with `op: "removed"`. Local socket only. |
| `file.search` | `{ lane_id, query, regex?, case_sensitive?, glob?, max_results? }` | `FileSearchResult`: `{ query, hits: [{ path, line, column, preview }], truncated }`; search file contents across non-ignored worktree files, skipping binaries and files over 2 MiB. Cap at `max_results` (default 200, hard cap 2000). Local socket only. |
| `file.diff_base` | `{ lane_id, path }` | `FileDiffBaseResult`: `{ content: string | null, kind: "text" \| "binary" \| "missing" }`; returns the HEAD version of the path via `git show HEAD:<path>` in the lane worktree (`missing` for untracked or newly added files, `binary` by the same null-byte sniff as `file.read`, same 8 MiB cap). Local socket only. |
| `viewport.set` | `{ lane_ids, focus_lane?, focus_window?, windows? }` | `null` (`focus_lane`/`focus_window` pick which agent window the focused lane streams; others stream their first slot. `windows` names plain-terminal windows — `term-{lane}-{n}` — to stream as extra panes alongside the lanes, e.g. the Grid's shell tiles; non-terminal names are ignored. Per-connection: each connection owns its own viewport and focus; the capture loop streams the union across every live connection, and `event.agent.output` is filtered to the connections whose viewport actually covers it) |
| `subscribe` | `{ topics? }` | `null` |
| `ping` | — | `"pong"` (remote keep-alive / connectivity probe) |
| `remote.pair` | `{ name }` | `{ name, token, url }` — mints (or, for a name already paired, re-shows) that device's own revocable token and its `repomon://` pairing URL. Capped at 16 paired devices. Local socket only. |
| `remote.devices` | — | `[{ name, role, created_at, last_seen_at? }]` — never includes the token. Local socket only. |
| `remote.revoke` | `{ name }` | `{ revoked }` — `true` if a device by that name existed; drops it from the auth cache so live connections holding its token are kicked on their next request. Local socket only. |
| `push.register` | `{ device_token }` | `null` (register an APNs device for push; idempotent) |
| `push.unregister` | `{ device_token }` | `null` |
| `daemon.status` | - | `{ uptime_secs, repos, lanes, db_size_bytes, version, protocol_revision, capabilities }`; terminal checkpoint and stream-sequence support is advertised in `capabilities`. |
| `daemon.shutdown` | — | `null` |
| `system.doctor` | - | `SystemDoctorResult`: `{ tmux: TmuxDoctorInfo, git: GitDoctorInfo, agents: [AgentDoctorInfo] }`, a machine-health snapshot backing Settings > System. `TmuxDoctorInfo` = `{ available, version?, source?, path? }` with `source` one of `"system"`/`"bundled"` (the portable tmux sidecar shipped with the app); `GitDoctorInfo` = `{ available, version?, path? }`; `AgentDoctorInfo` = `{ kind, name, command, detected }` per configured/built-in agent CLI. Local socket only. |
| `usage.get` | — | `[AccountUsage]` (per agent account, scraped from Claude `/usage` and Codex `/status`; empty unless `usage_probe` is enabled and a TUI is attached) |
| `orchestrator.status` | — | `{ running, agent?, model?, backend?, window?, autonomy?, session_id?, attention, headline? }` (the daemon-owned repomind orchestrator; reconciles against tmux, so a window killed externally reports `running:false`) |
| `orchestrator.transcript` | `{ limit? }` | `[TranscriptItem]` (repomind's conversation, same `{ role, text, at? }` shape as `agent.transcript`, so a client can render it as a chat instead of mirroring the pane; pinned to the orchestrator's own `session_id` when known, else falls back to the newest `$HOME` Claude transcript with real content across accounts. Always `[]` while `backend` is `"codex"`, `"antigravity"`, or `"opencode"` (none of their on-disk session formats are parsed); treat it as "no chat view for this backend" and render the `event.orchestrator.output` pane stream instead, never as an error/loading state) |
| `orchestrator.start` | `{ agent?, model?, autonomy?, max_agents?, prompt? }` | `{ running, agent?, model?, backend?, window?, autonomy?, session_id?, attention, headline? }` (ensure the repomind home repo and its controller lane, then spawn or adopt the primary controller in that lane's window, wired to the repomon MCP server with `REPOMON_MCP_MODE=orchestrator`; idempotent; re-spawns if the prior window died. `agent` picks the backend: a Claude account / custom agent name, `codex`, `antigravity`/`agy`, or `opencode`/`open-code`, defaulting to `[repomind] primary_agent` then `orchestrator_agent`; an agent with no MCP client for orchestration (e.g. `aider` or `cursor`) is rejected with `invalid_params` instead of spawning a broken window) |
| `orchestrator.stop` | — | `{ running:false, attention:"none", headline:null, … }` (**deprecated** alias: kill the controller lane's window) |
| `orchestrator.target` | — | `{ target, available, attach? }` (**deprecated** alias: attach target for the controller window; resets it to follow the attaching client's size; `attach` as in `agent.target`) |
| `orchestrator.send_input` | `{ text, enter=true }` | `null` (**deprecated** alias for `agent.send_input` on the controller window: type an instruction to repomind, then Enter unless `enter=false`) |
| `orchestrator.key` | `{ key, literal=false }` | `null` (**deprecated** alias for `agent.key`: one keystroke to repomind, literal char or key name) |
| `orchestrator.watch` | `{ on }` | `null` (**deprecated** alias: gate the pane stream; the TUI sets it `true` while the command-center view is open and `false` on leaving) |
| `orchestrator.resize` | `{ cols, rows }` | `null` (**deprecated** alias: size the controller window to the viewer's pane so its capture reflows to fit; clamped to a floor) |
| `repomind.status` | — | [`RepomindStatus`](../apps/desktop/src/bindings/RepomindStatus.ts): `{ home, exists, repo_id?, lane_id?, window?, max_controllers, export, counts, boot }` (read-only: where the repomind home repo lives, whether it is on disk yet, which repo/lane represent it, the controller lane's last recorded tmux window, and the controller cap. `export` is `{ last_run?, pending, last_error? }`: when the one-way export last ran, whether a write is waiting for its 5 s debounce, and the last failure. `counts` is `{ active_plans, standing, playbooks, drafts }`, read from `plans/active/`, `plans/standing/`, `playbooks/`, and `playbooks/drafts/` (each directory's own `README.md` never counts). `boot` is `{ generated_at?, tokens_estimate, trimmed }`: when `.repomind/boot.md` was last assembled, its size in the budget's four-characters-per-token units, and the home-relative paths the budget forced out of it (all zero/empty before the first regeneration). Never creates anything) |
| `repomind.boot` | — | `{ path, bytes, tokens_estimate, trimmed }` (**local-only**: reassemble the daemon-owned boot context at `<home>/.repomind/boot.md` and report what it produced. `path` is absolute, `bytes` the document's size on disk, `tokens_estimate` its size in the budget's units, and `trimmed` the home-relative paths the budget forced out, least important first. Every spawn into the controller lane regenerates it too, so this is for seeing what a controller would be handed right now) |
| `repomind.export` | — | `{ files, kinds }` (**local-only**: run the one-way export now instead of waiting out the debounce. `files` are the home-relative paths written or removed, `kinds` the record kinds they belong to (`journal`, `schedules`, `approvals`, `notes`, `playbooks`), which also form the commit subject `chore(repomind): export <kinds>`. Empty when nothing changed. Silently a no-op when the home does not exist) |
| `repomind.instruct` | `{ text }` | `{ outcome, window, entry_id?, reason? }` (**local-only**: type one instruction into the primary controller's composer, using the same verified injection as fleet mail and the boot line. The text is squashed onto one line and framed as `[REPOMIND] <text> [END REPOMIND]`, whose closing sentinel is the delivery marker. `outcome` is `"sent"`, `"skipped"` (the composer was busy or a dialog was up, with `reason` naming which), or `"failed"`. Refuses with `invalid_params` when the text is empty, when the home has no controller lane, or when no controller is running: an instruction that went nowhere is reported, never dropped quietly) |
| `playbook.reject` | `{ name }` | `{ name, path, status }` (**local-only**: the other half of the human approval gate. Moves `playbooks/drafts/<name>.md` to `playbooks/rejected/<name>.md` with `status: rejected` and a `rejected` stamp in its frontmatter. It never deletes: the text stays on disk and in the home's git history, and rejecting a revision leaves the approved playbook standing. `path` is home-relative. `invalid_params` when no draft by that name exists) |

The daemon exports its own records into the home one way and commits each batch there as
`Repomind <repomind@local>`: journal rows become `journal/YYYY-MM-DD.md` sections keyed by row id
in an HTML comment, schedules become `plans/standing/<slug>.md`, and the approval rules become
`profile/approvals.md`. Exports are debounced 5 s behind a journal, schedule, or approval-rule
write, and commits are batched at most once a minute; nothing outside those targets is written,
and a home that is not a git repo simply skips the commit. Repo notes (`fleet/<repo>/notes.md`)
and playbooks (`playbooks/<name>.md`, drafts under `playbooks/drafts/`) are file-first: the
`repo.notes.*` and `playbook.*` RPCs read and write those files directly, approving a
playbook moves its file up out of `drafts/`, and rejecting one moves it sideways into
`playbooks/rejected/` rather than deleting it. A daemon start migrates anything that still only
exists in SQLite or the app-support `repo-notes/` directory into the home, and never deletes the
original.

At spawn the daemon assembles `<home>/.repomind/boot.md` and hands it to the controller: the
operator's `REPOMIND.md` overlay whole, `profile/*.md` bodies with their frontmatter stripped,
one status line per `plans/active/*.md`, yesterday's and today's journal day files, and a fleet
snapshot of one line per lane (repo, lane, branch, agent count, most urgent state). The whole
document is bounded by `[repomind] boot_budget_tokens` (12k by default, estimated at four
characters to a token); over budget it drops the journal first, then profile notes, then plans,
and ends with a `Trimmed: ...` line naming exactly what went. Claude receives it as
`--append-system-prompt-file <path>` on top of the shipped persona (which stays the system
prompt); Codex, Antigravity, and OpenCode get a first typed line pointing at the file, delivered
through the same verified-composer injection fleet mail uses so it never types over a busy
composer. The file is daemon-owned and gitignored: never hand-edit it.

Repomind runs in the **controller lane**, the main worktree of the repomind home repo
(`~/repomind` by default, `[repomind] home`). It is registered like any other repo, and its lane
carries `role: "controller"` in `lane.list`. Its window is therefore an ordinary `lane-*` window and does
appear in `lane.list`, the lane overlay, and the reaper. The old daemon-owned `orchestrator`
window is kept only as an adoption fallback, so a window left behind by a pre-R1 daemon is
adopted rather than duplicated.

Every `orchestrator.*` RPC above except `.status`/`.transcript`/`.start` is a **deprecated** thin
alias onto the controller lane's window:

| Deprecated alias | Replacement | Removal target |
|---|---|---|
| `orchestrator.stop` | `agent.stop` | the release after next |
| `orchestrator.target` | `agent.target` | the release after next |
| `orchestrator.send_input` | `agent.send_input` | the release after next |
| `orchestrator.key` | `agent.key` | the release after next |
| `orchestrator.watch` | `viewport.set` | the release after next |
| `orchestrator.resize` | `agent.resize` | the release after next |

Prefer `lane.list` plus the `agent.*`/`viewport.set` RPCs, which work on the controller lane's
window like any other agent's; find the window itself from `lane.list`'s `role: "controller"`
row or from `repomind.status`'s `window`. Each deprecated alias's handler logs one
`tracing::warn!` naming its replacement the first time it is called in the daemon process's
lifetime (not on every call, since some of these sit on a hot path), so an operator or client
author watching the daemon log gets one nudge to move off it before it is removed.

`attention` on the orchestrator payloads above is always present: one of `"none"`,
`"permission"`, `"decision"`, `"end_of_turn"`. `headline` is non-null only alongside
`permission`/`decision` (the open dialog's question) or `end_of_turn` (a tail of repomind's
last message, when cheaply available); always `null` when `attention` is `"none"`.

`backend` is the normalized agent CLI the session runs on: `"claude"`, `"codex"`,
`"antigravity"`, or `"opencode"` (`null` when not running), and is what clients should switch
rendering on (`agent` is the raw launch name, e.g. `claude-work`). Claude is the only backend
with a parseable on-disk transcript; a `"codex"`, `"antigravity"`, or `"opencode"` session is
monitored best-effort from its pane only: `orchestrator.transcript` is always `[]`, `attention`
never reports `"end_of_turn"` (pane dialogs may still surface `permission`/`decision`), and
`session_id` is always `null`.

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
newest-with-content heuristic. Always `null` for a `"codex"`, `"antigravity"`, or `"opencode"`
backend (none has an equivalent of `--session-id`, and none of their session files are parsed;
the fallback heuristic is deliberately NOT applied there, since it would misattribute an
unrelated Claude session's transcript).

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
`untracked` is the lane's untracked-file *count* only, computed live alongside the stats (one
snapshot is self-consistent) — untracked file **contents** never appear in `patch` or either
`*_stat` field. `patch` (`git diff HEAD` text, capped at
`max_patch_chars`, char-boundary safe) and `patch_truncated: true` are present only when
`include_patch: true` and the patch was actually cut; `max_patch_chars` is server-clamped to a
ceiling of 20000. Errors (`-32000`) when the base branch shares no common history with `HEAD`,
or the repo's main checkout has no current branch (detached HEAD). Allowed over the remote
bridge — it joined the allowlist with the rest of fleet control (see "Remote transport" above).

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
| `event.agent.bytes` | `{ lane_id, window, data, generation, sequence }` - raw PTY bytes (base64) from the byte-watched pane. Chunks are arbitrary byte boundaries, so feed them to a terminal emulator rather than parsing them as individual UTF-8 strings. Sequence values are contiguous within one generation. |
| `event.agent.stream_closed` | `{ lane_id, window, generation }` - the backend stream ended because the watched target disappeared. Release local watch state only when `generation` matches the active stream; delayed notifications from an older generation are stale. |
| `event.agent.changed` | `{ name }` or `{ default }` (a custom agent was added/removed, or the default changed) |
| `event.file.changed` | `{ lane_id, path }`, a worktree file was saved through `file.write` (so other viewers of the same file can reload). |
| `event.notification` | `{ lane_id, session_id?, kind, title, body, prompt?, attention, dialog? }` — daemon-side agent alert (kinds: `needs_you`, `rate_limited`, `resumed`, `idle`, `stalled`; `prompt` is the agent's pending question verbatim). `attention` refines `needs_you`: `permission` (routine tool-call ask) / `decision` (a real question) / `done_candidate` (turn finished on a clean lane with a this-turn commit — ready to review) / `end_of_turn` (turn finished, no dialog) / `none`; `dialog` is the full `PendingDialog` when one is on screen, so an actionable client can offer its real options. `stalled` fires once when a managed agent's pane and transcript both freeze mid-work for ~5 min while its process lives (see `AgentSession.stale`/`stalled_since` on `lane.list` — additive overlay fields, with `stalled_since` marking the pane's last change). Emitted to every subscribed client. When `[remote]` is enabled, the same alert also goes to APNs devices with category `AGENT_PROMPT` (actionable) or `AGENT_ALERT`. |
| `event.message.stored` | `{ id, lane_id?, from, body, message }` for one newly accepted durable fleet message. Clients deduplicate only by `id`. |
| `event.orchestrator.output` | `{ content, cursor? }` — the repomind pane's text (and `[col, row]` cursor) streamed while watched; same shape as `event.agent.output` without `lane_id`. |
| `event.orchestrator.status` | `{ running, agent?, model?, backend?, window?, autonomy?, session_id?, attention, headline? }` — broadcast when the orchestrator starts, stops, is reconciled to stopped after its window died, or its `attention` changes. |

Object ids travel as lowercase hex strings; timestamps as RFC3339 UTC.

### Usage model rates

`usage.models` takes no params and returns `ModelRateRow[]`: one row per model in
`usage_events`, plus every configured override, including unseen model ids or family prefixes.
It is readable over the remote bridge. Each row contains:

- `model`: model id or family prefix.
- `input_per_mtok`, `output_per_mtok`, `cache_read_per_mtok`, `cache_write_per_mtok`:
  resolved USD rates per million tokens.
- `source`: `builtin`, `litellm`, `override`, or `unpriced`. Unmatched models have zero
  rates and are explicitly `unpriced`; known free-tier models are `builtin` at zero.
- `override`: the raw `PriceOverride` or `null`. Its four rate fields and `effective_from`
  are nullable; absent corrections are never filled with resolved values here.
- `last_seen`: latest `usage_events.at` timestamp, or `null` for an unseen override.
- `tokens_30d`: sum of input, output, cache-read, and cache-write tokens in the last 30 days.
  Thinking tokens already billed as output are not counted twice.

`usage.rates` remains the small provenance response (`RatesStatus`): source counts, refresh
state, fetch timestamp, next refresh timestamp, ETag, and last error. `usage.refresh_rates`
returns the same shape after an immediate refresh and remains local-only.

`config.get` includes `usage_enabled` and `usage_refresh_prices`. Local-only `config.set`
accepts either toggle and these atomic per-model patches:

```json
{"usage_price_override_upsert":{"model":"claude-sonnet-5","output_per_mtok":9}}
```

```json
{"usage_price_override_reset":"claude-sonnet-5"}
```

Upsert accepts `input_per_mtok`, `output_per_mtok`, `cache_read_per_mtok`, and
`cache_write_per_mtok`. At least one finite non-negative rate is required. Zero is a valid
rate. Omitted fields preserve existing overrides, otherwise inheriting the resolved snapshot
or built-in value. The model must be non-empty with no surrounding whitespace. Reset removes
that model's entire override; resetting a missing key is a no-op. Both operations return the
updated config view, persist `[usage.price_overrides]`, and update the daemon's live config.
The next usage query re-prices history without a restart. Rate edits emit `event.usage.changed`
as well as `event.config.changed` so open views reload their costs.

### Manual usage refresh

`usage.refresh` (no params, local only) reads cached gates and immediately returns
`{ refreshed: false, reason, detail, snapshot, request_id? }`. The reason is
`pending`, `probe_disabled`, `no_active_kind`, or `cooldown` (a manual round is
already queued/running). An unknown fleet cache accepts a pending request; a
known cache with no supported active kind skips it. Manual requests bypass the
five-minute freshness cooldown and wake ledger ingest. No probe IO or completion
wait happens in the connection's serial RPC dispatcher.

Subscribe before requesting refresh. `event.usage.refreshed` carries
`{ request_id, reason: "ok" | "timeout" | "error", detail, snapshot }`.
The watcher sends completion, or a `timeout` notification after 15 seconds while
it continues probing. That notification does not release the round's single-flight
guard; final completion can arrive later on the same ticket. A hard ceiling of
75 seconds per installed account (at least one account) releases the guard even
if the watcher stops. Old timers cannot release a newer ticket. `snapshot` matches
`usage.get`. Only a matching ticket settles a client's wait, even if its event
arrives before the RPC response. Desktop waits for the event locally, with a
20-second ceiling, then re-snapshots quota and today's cost and shows an inline
notice when appropriate. Late completion still refreshes the displayed numbers.

A deadline notification means work is continuing, including later accounts in a
round. Desktop renders `timeout` as "Still probing, this can take a moment",
including its own 20-second ceiling. Only `error` uses failure wording. Fleet
polling continues during the round, so accounts already probed can update before
the final completion.
