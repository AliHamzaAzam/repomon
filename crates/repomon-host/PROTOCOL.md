# repomon-agent-host control protocol (v1) — FROZEN

This document is the inter-track contract for repomon's native Windows session model.
`repomon-agent-host.exe` (Track C) implements it; the daemon's `WindowsBackend` (Track I) and
the attach client (Track F) build against **this document**, not against the host's code.

**Freeze rule:** from the commit that introduces this file, the protocol is frozen for the
implementation wave. Any change is a mini-RFC that the integrator must approve, and it must
touch Tracks C, I, and F together. Extensions are additive only: new optional request fields,
new response fields, and new ops. Unknown fields MUST be ignored by both sides. Unknown ops
MUST produce an `err` response, never a disconnect.

## 1. Roles and lifecycle

One host process per agent window — the Windows equivalent of one tmux window on the tmux
server. The host:

1. Spawns the agent child on a ConPTY (via `portable-pty`), with a structured
   `program + args + cwd + env` (never a shell string).
2. Feeds every PTY output byte into a server-side `vt100` parser with 50 000 lines of
   scrollback (parity with tmux `history-limit 50000`), and records `last_activity`
   (Unix epoch seconds) on every output chunk.
3. Serves the control protocol below on a named pipe.
4. Writes a registry file on startup and removes it on exit.

The host is spawned **detached** (`DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`) and keeps
running when its parent (the daemon) dies. Durability parity: agents survive daemon restarts.

**Exit semantics (tmux parity — the window disappears):** when the agent child exits, or when
a `kill` request is served, the host removes its registry file and exits with status 0. There
is no idle host without a child.

### Host command line (spawn contract for Track I)

```
repomon-agent-host.exe
    --session <session> --window <window> --cwd <dir>
    [--owner <token>] [--cols <n>] [--rows <n>] [--env KEY=VALUE]...
    -- <program> [args]...
```

- `--session` / `--window`: tmux-parity names (already validated `[A-Za-z0-9_-]+` upstream).
- `--cols` / `--rows` default to **220 × 50** (parity with tmux `new-session -x 220 -y 50`).
- `--env` adds/overrides variables on top of the host's inherited environment.
- `--owner`: opaque owner token (see §6). If absent, the host generates a random one.
- `<program>` is resolved like `CreateProcess` does; npm `.cmd` shims (e.g. `claude`) are
  handled by `portable-pty`'s `CommandBuilder`.
- Exit codes: `0` clean (child exited or `kill` served), `2` usage error, `1` runtime failure.

## 2. Pipe naming

Each host serves exactly one named pipe:

```
\\.\pipe\repomon-<session>-<window>
```

Example: session `repomon`, window `lane-3-1` → `\\.\pipe\repomon-repomon-lane-3-1`.

Multiple simultaneous client connections are supported (the daemon's control connection, a
byte subscriber, an attach client, …). The host keeps a spare pipe-server instance pending at
all times.

## 3. Security: per-user DACL (REQUIRED)

The pipe MUST be created with an explicit security descriptor that grants access **only to
the current user** (the SID of the host process's token), owner set to that SID, DACL
protected (no inheritance): SDDL shape `O:<sid>G:<sid>D:P(A;;GA;;;<sid>)`. The first instance
MUST be created with `FILE_FLAG_FIRST_PIPE_INSTANCE` so a squatter cannot pre-claim the name.
The default (permissive) named-pipe DACL is not acceptable. The registry directory inherits
the per-user protection of `<data_dir>` (under `%APPDATA%`); the owner token is a liveness /
identity check, **not** the security boundary — the DACL is.

## 4. Framing

Length-prefixed JSON, both directions:

```
[u32 length, little-endian][length bytes of UTF-8 JSON]
```

- `length` is the byte length of the JSON payload only (prefix excluded).
- Maximum frame size: **16 MiB** (`16 * 1024 * 1024` bytes). A peer receiving a larger
  length MUST treat the connection as corrupt and disconnect.
- One JSON document per frame. No trailing newline or padding.

## 5. Conversation model

A connection is in **request/response mode** until (if ever) a successful `subscribe_bytes`
switches it to **stream mode**.

- Request: `{"id": <u64>, "op": "<name>", ...op-specific fields}`
- Success: `{"id": <same u64>, "ok": {...op-specific result}}`
- Failure: `{"id": <same u64>, "err": "<human-readable message>"}`

`id` is chosen by the client and echoed verbatim. Clients MAY pipeline requests; the host
answers in order received. In stream mode the host pushes unsolicited frames (see
`subscribe_bytes`) and ignores any further client frames on that connection; disconnecting
is the only unsubscribe.

## 6. Owner-token handshake

Parity with the tmux `@repomon-owner` server option. The token is an opaque string fixed at
spawn (`--owner`, or host-generated). It appears in the registry file and in every `hello`
response. Adoption rule (Track I): after connecting and reading `hello`, a daemon compares
`owner` with its own identity — on mismatch the daemon MUST back off (not adopt, not reap,
not kill). There is no way to change a host's owner after spawn.

## 7. Requests

All examples show the full frame payload. Optional fields may be omitted entirely.

### 7.1 `hello`

Request: `{"id": 1, "op": "hello"}`

Response:

```json
{"id": 1, "ok": {
  "proto": 1,
  "session": "repomon",
  "window": "lane-3-1",
  "cwd": "C:\\Users\\me\\code\\proj",
  "program": "claude",
  "args": ["--permission-mode", "plan"],
  "agent_pid": 5678,
  "host_pid": 4321,
  "started_at": 1789000000,
  "last_activity": 1789000123,
  "owner": "daemon-DESKTOP-ME-1a2b3c"
}}
```

- `proto`: protocol version, always `1` for this document.
- `agent_pid`: the ConPTY child's process id.
- `started_at` / `last_activity`: Unix epoch seconds. `last_activity` is the time of the
  last PTY output chunk (parity with tmux `#{window_activity}`); equals `started_at` until
  the child first writes.

### 7.2 `capture`

Request: `{"id": 2, "op": "capture", "lines": 500}` (`lines` optional)

Response: `{"id": 2, "ok": {"text": "…"}}`

Parity with `tmux capture-pane -e -p [-S -<lines>]`: the visible screen's rows, preceded by
up to `lines` rows of scrollback when `lines` is present, joined with `\n`, each row carrying
inline SGR escape sequences. Rendered from the host's vt100 screen (the source of truth —
ConPTY-synthesized quirks do not leak into capture).

### 7.3 `cursor`

Request: `{"id": 3, "op": "cursor"}`

Response: `{"id": 3, "ok": {"col": 12, "row": 4, "visible": true}}`

0-based visible-pane coordinates. `visible` mirrors tmux `#{cursor_flag}`; clients treat
`visible: false` as "no cursor" (tmux `cursor_named` → `None`).

### 7.4 `size`

Request: `{"id": 4, "op": "size"}`

Response: `{"id": 4, "ok": {"cols": 220, "rows": 50}}`

### 7.5 `alternate_on`

Request: `{"id": 5, "op": "alternate_on"}`

Response: `{"id": 5, "ok": {"on": true}}`

Whether the child is on the alternate screen (full-screen TUI), parity with
`#{alternate_on}`.

### 7.6 `resize`

Request: `{"id": 6, "op": "resize", "cols": 190, "rows": 45}`

Response: `{"id": 6, "ok": {}}`

Resizes the ConPTY and the vt100 screen. **Last client wins** — no arbitration; the most
recent `resize` from any connection is in effect.

### 7.7 `send_literal`

Request: `{"id": 7, "op": "send_literal", "text": "y"}`

Response: `{"id": 7, "ok": {}}`

Writes the UTF-8 bytes of `text` to the child's input. No translation, no trailing newline
(parity with `send-keys -l`).

### 7.8 `send_text`

Request: `{"id": 8, "op": "send_text", "text": "continue"}`

Response: `{"id": 8, "ok": {}}`

Writes the UTF-8 bytes of `text`, then a carriage return (`\r`) — parity with
`send-keys -l <text>` + `send-keys Enter`.

### 7.9 `send_key`

Request: `{"id": 9, "op": "send_key", "key": "C-c"}`

Response: `{"id": 9, "ok": {}}`

`key` uses the tmux key vocabulary already emitted by the daemon and TUI:

- Named: `Enter`, `Escape`, `Tab`, `BTab`, `BSpace`, `DC`, `Home`, `End`, `PageUp`,
  `PageDown`, `Up`, `Down`, `Left`, `Right`, `F1`…`F12`, `Space`.
- Modified: `C-<key>` (Ctrl), `M-<key>` (Meta/Alt), for both single characters (`C-c`,
  `M-f`) and named keys (`C-Up`, `M-BSpace`).
- A single printable character stands for itself.

The host translates to conventional VT input sequences (e.g. `C-c` → `0x03`, `Up` →
`ESC [ A`, `BTab` → `ESC [ Z`, `C-Up` → `ESC [ 1 ; 5 A`, `M-x` → `ESC x`). Unknown names →
`err`.

### 7.10 `scroll_wheel`

Request: `{"id": 10, "op": "scroll_wheel", "up": true, "ticks": 3}`

Response: `{"id": 10, "ok": {}}`

Writes SGR mouse-wheel sequences to the child's input at the pane's top-left, `ticks` times:
button 64 (up) / 65 (down), i.e. `ESC [ < 64 ; 1 ; 1 M` per tick (exact tmux
`scroll_wheel_named` parity). `ticks: 0` is a no-op success.

### 7.11 `subscribe_bytes`

Request: `{"id": 11, "op": "subscribe_bytes"}`

Response: `{"id": 11, "ok": {}}`

After the `ok`, the connection is in stream mode. The host pushes frames of the shape:

```json
{"stream": "bytes", "data": "<base64>"}
```

- `data` is standard base64 (RFC 4648, with padding) of raw bytes.
- **The FIRST stream frame is a full current-screen replay**: a byte sequence that redraws
  the host's vt100 screen from scratch on an empty terminal of the current size (clear,
  contents with attributes, cursor position and visibility). A client that starts a fresh
  emulator, applies frame 1, then applies subsequent frames verbatim converges exactly.
- Every subsequent frame is a raw PTY output chunk, in order, as produced by the child.
- The subscription ends when the client disconnects, or when the host exits (pipe EOF).
  There is no unsubscribe request.

### 7.12 `kill`

Request: `{"id": 12, "op": "kill"}`

Response: `{"id": 12, "ok": {}}`

Parity with `kill-window`: the host terminates the agent child, removes its registry file,
and exits. The `ok` is sent before exit; clients must tolerate pipe EOF immediately after.

## 8. Registry files

Directory: `<data_dir>\hosts\<session>\`, one file per live window: `<window>.json`.
`<data_dir>` is repomon's data dir exactly as `repomon_core::config::data_dir()` computes it
(`REPOMON_DATA_DIR` env override honored; the host honors the same override).

Schema (v1):

```json
{
  "v": 1,
  "session": "repomon",
  "window": "lane-3-1",
  "pipe": "\\\\.\\pipe\\repomon-repomon-lane-3-1",
  "host_pid": 4321,
  "agent_pid": 5678,
  "program": "claude",
  "args": ["--permission-mode", "plan"],
  "cwd": "C:\\Users\\me\\code\\proj",
  "owner": "daemon-DESKTOP-ME-1a2b3c",
  "started_at": 1789000000
}
```

- Written **atomically** (temp file + rename) after the pipe server is listening, so a
  registry entry implies a connectable pipe (modulo crashes).
- Removed by the host on clean exit (child exit or `kill`).
- **Stale-entry GC (Track I):** a scanner that fails to connect to `pipe` (file gone /
  refused) may treat the entry as stale and delete the JSON file. `last_activity` is NOT in
  the registry — it changes constantly; read it via `hello`.
- Unknown JSON fields MUST be ignored; additions bump nothing (additive-only, `v` stays 1).

## 9. Mapping to `SessionBackend` (informative)

| Backend call | Host op |
|---|---|
| `capture_named` | `capture` |
| `cursor_named` | `cursor` |
| `size_named` | `size` |
| `alternate_on_named` | `alternate_on` |
| `resize_named` | `resize` |
| `send_literal_named` | `send_literal` |
| `send_text_named` | `send_text` |
| `send_key_named` | `send_key` |
| `scroll_wheel_named` | `scroll_wheel` |
| `open_byte_stream` | `subscribe_bytes` |
| `kill_named` | `kill` |
| `list_windows_with_activity` | registry scan + `hello` per host |
| `claim_or_verify_owner` | registry/`hello` `owner` comparison |
