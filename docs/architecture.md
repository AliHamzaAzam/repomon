# Architecture

repomon is a background **daemon** plus a thin **TUI** client, sharing a **core** library.
The daemon owns all state and all git/session work; the TUI only renders cached state and
forwards input. That split is what keeps the UI instant and lets agents outlive the UI.

The agent runtime is abstracted behind a `SessionBackend` trait: a tmux backend on
macOS/Linux and a host-process backend on Windows (see
[Session backends](#session-backends-tmux-on-unix-host-processes-on-windows) below). Local
IPC runs over a Unix socket on macOS/Linux and a named pipe on Windows; the JSON-RPC wire
protocol is identical on both.

```
      ┌───────────────────────── repomon-core ────────────────────────┐
      │ model · store(SQLite) · git(gix + worktree shellout) · watch  │
      │ registry · lane · agent(tmux runtime + Claude/Aider monitors) │
      │ analytics · session · indexer · service(launchd/systemd) · protocol │
      └──────────────▲──────────────────────────────▲─────────────────┘
                     │                              │
                     │ (lib)                        │ (lib)
        ┌─────────────────────────┐    ┌─────────────────────────┐
        │ repomon-daemon          │    │ repomon-tui             │
        │ repomond:               │    │ repomon:                │
        │   UnixListener + RPC    │ ◄─ │   DaemonClient (socket) │
        │   pubsub broadcast      │    │   app loop + views      │
        │   watchers / streamer   │    │   keybinds (arrows)     │
        │   indexer               │    │   cd-on-exit            │
        └─────────────────────────┘    └─────────────────────────┘
                                 socket · framed JSON-RPC
```

## Crates

- **repomon-core**: the engine. No UI, no socket server. Holds the data model, the SQLite
  store (a dedicated writer thread; never blocks the runtime), the gix-backed git reader and
  `git worktree` shellout, the notify watcher, the repo registry and lane manager, the agent
  runtime behind the `SessionBackend` trait (tmux on Unix, host processes on Windows) and the
  agent monitors, the local transport abstraction (Unix socket ⇄ named pipe), the Phase-3
  analytics/sessions/indexer, service management (launchd on macOS, systemd user units on
  Linux, Task Scheduler on Windows), and the shared JSON-RPC protocol + framing.
- **repomon-daemon** (`repomond`): hosts core behind a length-prefixed JSON-RPC endpoint (a
  Unix socket on macOS/Linux, a named pipe on Windows). Owns the `Ctx` (store, registry, lanes,
  the `SessionBackend`, event bus, viewport + focus, and the overlay/status caches), the
  per-connection handling, RPC dispatch, the output streamer, the desktop/remote notification
  watcher, and the background indexer.
- **repomon-tui** (`repomon`): the terminal client and headless CLI. A `DaemonClient` over
  the transport, an async app loop, the four-zoom views, the `repomon attach-host` pop-out
  attach client (Windows), and the `repomon …` subcommands.
- **repomon-host** (`repomon-agent-host.exe`, Windows only) is a small per-agent host process:
  one ConPTY child, a server-side `vt100` screen with scrollback, and a named-pipe control
  server. It is the Windows equivalent of a tmux window and survives daemon restarts. Its
  control protocol is frozen in [../crates/repomon-host/PROTOCOL.md](../crates/repomon-host/PROTOCOL.md).

## Key flows

**Fleet refresh.** The TUI calls `lane.list`; the daemon enumerates worktrees (porcelain),
computes live state with gix off the runtime, overlays agent sessions (Claude transcript →
Aider history → tmux-alive fallback), and returns lanes. The TUI renders cached state
immediately on every keystroke; git never runs on the UI thread.

**Live agents.** The daemon spawns each agent in its own session backend window: the first at
`lane-<id>`, additional agents sharing a worktree at `lane-<id>-2`, `lane-<id>-3`, … so
several run side by side. (On Unix a "window" is a tmux window; on Windows it is a host
process; see [Session backends](#session-backends-tmux-on-unix-host-processes-on-windows).)
The TUI tells the daemon which lanes are visible (and which agent window the focused lane
should stream) via `viewport.set`; a streamer fast-polls only those panes (`capture_named` /
`capture-pane`) and pushes `event.agent.output`. Input goes back via `agent.send_input`
(`send-keys` on Unix, a host `send_*` op on Windows), routed to a specific window in a
multi-agent lane; `agent.target` + a raw attach (`tmux attach`, or `repomon attach-host` on
Windows) gives the unmediated session.

**Change propagation.** A debounced notify watcher (250 ms; `.git/objects` and build/dependency
churn like `target` and `node_modules` excluded) plus the Claude projects directory feed
`event.repo.changed`; the TUI refetches. A 60 s tick is the safety net. The watcher is held in
the daemon's runtime context and watches/unwatches a tree on `repo.add` / `repo.remove`, so a
removed repo stops churning fsevents instead of being watched until the next restart.

**Desktop notifications.** A `notify_watch` task runs the shared edge-detection over the fleet
and fires an alert on each meaningful agent transition. Remote clients get an `event.notification`
broadcast (and APNs push) while `[remote]` is enabled; locally, the daemon fires desktop popups
itself only once the TUI stops covering them — it watches the TUI's ~1 s `lane.list` heartbeat
and, after 3 s of silence (the TUI parked in a full-screen attach or closed), takes over so an
alert still reaches you when you're heads-down in an agent pane.

**History.** On startup and after `repo.add`, the indexer walks HEAD history into SQLite, so
`timeline`, `sessions`, and `commit.search` work over history rather than just live HEAD.

**repomind.** The orchestrator is a daemon-owned agent session — Claude by default, Codex CLI
optionally (`orchestrator.start`'s `agent` param, the `orchestrator_agent` config) — running in
its own `orchestrator` tmux window (`orchestrator.start`/`.stop`), reachable like any other
window (`.target`/`.send_input`/`.key`/`.resize`). Only MCP-capable CLIs qualify (aider can't
drive the fleet tools, so it's rejected); a Codex-backed session degrades to pane-only
monitoring — no parsed transcript chat, no end-of-turn attention, no session pinning.
`repomon-mcp` (invoked as `repomond mcp`) is a stdio MCP server the orchestrator agent launches
as a subprocess and wires up as a tool server; it connects back to the same daemon socket as an
ordinary client and keeps a fleet snapshot refreshed by poll-and-diff (`lane.list` on a ~1.5s
cadence, woken early on a structural event — a lane created/deleted). Because the orchestrator
session pre-approves its own fleet tool calls (`--allowedTools` on Claude, the approval policy
on Codex — no permission dialog to intercept, unlike a worker agent), the MCP server's own
policy layer — autonomy level, a per-session action cap, a send-dedupe window, two-phase
confirm tokens for destructive actions — is the *sole* gate on what repomind can do. The
daemon's `notify_watch` tick, the same one that fires desktop alerts for lane agents, also
classifies repomind's own attention (a pending dialog, or — Claude only — an idle end-of-turn)
each pass and broadcasts it as `event.orchestrator.status`.

## Session backends (tmux on Unix, host processes on Windows)

Every place the runtime spawns, captures, sends input to, resizes, streams, or kills an agent
goes through one trait, `SessionBackend` (`crates/repomon-core/src/agent/backend.rs`). There
are two implementations and nothing else in the daemon knows which is live:

- **Unix, `TmuxRuntime`.** Unchanged behavior. tmux owns the durable, out-of-process agent
  runtime; the trait methods render to `tmux` subcommands (`new-window`, `capture-pane`,
  `send-keys`, `resize-window`, `pipe-pane`, `attach`, `kill-window`).
- **Windows, `WindowsBackend`.** No tmux. Each agent runs in its own detached host process,
  `repomon-agent-host.exe`, which owns a ConPTY child plus a server-side `vt100` screen with
  50 000 lines of scrollback (parity with tmux `history-limit 50000`). The backend is a
  named-pipe client of those hosts.

Commands are assembled structurally as a `SpawnSpec { program, args, cwd, env }` rather than a
shell string: the tmux backend renders it through its existing shell-quoting, and the Windows
host feeds it straight to ConPTY, so there is no `sh -c` or `cmd /c` quoting on Windows.

### tmux-parity mapping

| Concept | Unix (tmux) | Windows (host processes) |
|---|---|---|
| process registry | tmux window names on a dedicated tmux server | host registry dir + named pipes: `<data_dir>\hosts\<session>\<window>.json` |
| agent process owner | tmux server (out-of-process, durable) | one `repomon-agent-host.exe` per window, detached, survives daemon restarts |
| PTY | tmux-owned pty | ConPTY (`portable-pty`) inside the host |
| capture-pane | `tmux capture-pane -e -p` | server-side `vt100` screen render (the source of truth; ConPTY quirks do not leak) |
| byte stream | `pipe-pane` + `mkfifo` FIFO | host byte subscription over its pipe (`subscribe_bytes`) |
| send input | `tmux send-keys` | host writes translated VT sequences to the ConPTY |
| resize | `tmux resize-window` | host resizes the ConPTY + vt100 screen (last-client-wins) |
| attach | `tmux attach` (PTY handoff) | `repomon attach-host <window>` raw byte proxy in a new Windows Terminal tab |
| owner guard | `@repomon-owner` tmux server option | owner token in the registry file + `hello` handshake |
| kill-window | `tmux kill-window` | host kills the child, removes its registry entry, exits |

### Durability and re-adoption (Windows)

Hosts are spawned with `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` and keep running when the
daemon dies, the same durability tmux gives on Unix. On startup `WindowsBackend` scans the
registry directory, connects to each pipe, verifies the `hello` (owner token + liveness), GCs
stale entries whose pipe will not connect, and **re-adopts** the surviving hosts with scrollback
intact. This is the Windows equivalent of the daemon rediscovering an existing tmux server.
Liveness on Windows skips the Unix `ps`/`lsof`//proc probe entirely: a host authoritatively
knows whether its child is alive, so the backend asks it directly.

### Named-pipe control protocol

Each host serves one pipe, `\\.\pipe\repomon-<session>-<window>`, secured with a per-user DACL.
The wire format is length-prefixed JSON with request/response ops (`hello`, `capture`, `cursor`,
`size`, `alternate_on`, `resize`, `send_literal`, `send_text`, `send_key`, `scroll_wheel`,
`subscribe_bytes`, `kill`). The full, frozen contract is
[../crates/repomon-host/PROTOCOL.md](../crates/repomon-host/PROTOCOL.md). Three points worth
calling out because clients (the daemon backend and the attach client) depend on them:

- **The byte stream is a raw byte channel.** `subscribe_bytes` switches the connection to
  stream mode; its **first** frame is a full current-screen replay (a client that starts a
  fresh emulator, applies frame 1, then applies subsequent frames verbatim converges exactly),
  and every later frame is a raw PTY output chunk.
- **A client derives the pipe name from config.** The `<window>` half of the pipe name is the
  tmux/host window id; the `<session>` half is `config.tmux_session` (default `repomon`), the
  same session name the tmux backend uses. A client that knows the window and the configured
  session name can compute the pipe without a lookup.
- **An interactive attach uses two connections.** Once a connection has issued
  `subscribe_bytes` the host ignores any further client frames on it (disconnect is the only
  unsubscribe), so an attach client opens a **second** control connection for `resize` /
  `send_*` while the first streams bytes to the terminal.

## Performance posture

- All git and tmux work runs in `spawn_blocking`; the reactor never blocks.
- SQLite writes go through one owned connection on a dedicated thread.
- The output streamer fast-polls only *visible* lanes, on an activity-driven backoff (the
  focused pane stays fast; idle/background panes back off to a cap) with a per-tick capture cap,
  and each tmux capture is a single fork; at rest it does nothing.
- Hot reads are cached so rapid client polls don't re-scan: the `lane.list` overlay (lanes +
  agent sessions) behind a short TTL invalidated only on structural changes, plus per-worktree
  gix-status, per-repo `git worktree list`, and per-window pending-prompt caches. A transcript
  write touches no worktree, so it never triggers a status walk.
- The TUI renders cached state, so first paint doesn't wait on the daemon.

### Measured (release, macOS, 3 fixture repos × 8 commits)

| Gate | Target | Measured |
|---|---|---|
| daemon cold start | < 500 ms | ~15 ms |
| `repomon --print-once` (warm daemon) | < 100 ms | ~38 ms median |
| daemon CPU, monitoring agents (idle) | < 2 % | ~1.35 % avg · median 0 % |
| daemon CPU, 8-pane Grid live-streaming | < 6 % | ~5 % avg · 2.5 % median |

(`hyperfine` wasn't available, so the latency rows were timed with a `perf_counter` harness over
fixture repos; the CPU rows were sampled live via `ps` CPU-time deltas.) The per-lane Claude
status lookup is an O(1) encoded-directory check on the hot path; the encoding-drift fallback
scan is reserved for explicit use to keep refresh fast.

The CPU figures are post-optimization. The daemon previously pegged a core (~150 % sustained,
all fork/exec overhead from a flat-10 Hz multi-fork pane streamer and a per-call worktree-walk
storm — the tmux server itself idled at 0.3 %); the streamer backoff, single-fork captures, and
the overlay/status caches above brought it down to the numbers shown.

See [protocol.md](protocol.md) for the wire API and [agents.md](agents.md) for agent
integration.
