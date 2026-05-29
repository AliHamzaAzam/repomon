# Architecture

repomon is a background **daemon** plus a thin **TUI** client, sharing a **core** library.
The daemon owns all state and all git/tmux work; the TUI only renders cached state and
forwards input. That split is what keeps the UI instant and lets agents outlive the UI.

```
            ┌───────────────────────── repomon-core ─────────────────────────┐
            │ model · store(SQLite) · git(gix + worktree shellout) · watch    │
            │ registry · lane · agent(tmux runtime + Claude/Aider monitors)   │
            │ analytics · session · indexer · service(launchd) · protocol     │
            └───────────────▲───────────────────────────▲────────────────────┘
                            │ (lib)                      │ (lib)
                ┌───────────┴───────────┐    ┌───────────┴────────────┐
                │     repomon-daemon     │    │      repomon-tui        │
                │  repomond:             │    │  repomon:               │
                │   UnixListener + RPC   │◄───┤   DaemonClient (socket) │
                │   pubsub broadcast     │ socket   app loop + views    │
                │   watchers / streamer  │ JSON-RPC keybinds (arrows)   │
                │   indexer              │    │   cd-on-exit             │
                └────────────────────────┘    └─────────────────────────┘
```

## Crates

- **repomon-core** — the engine. No UI, no socket server. Holds the data model, the SQLite
  store (a dedicated writer thread; never blocks the runtime), the gix-backed git reader and
  `git worktree` shellout, the notify watcher, the repo registry and lane manager, the
  tmux-backed agent runtime and the agent monitors, the Phase-3 analytics/sessions/indexer,
  launchd service management, and the shared JSON-RPC protocol + framing.
- **repomon-daemon** (`repomond`) — hosts core behind a length-prefixed JSON-RPC Unix socket.
  Owns the `Ctx` (store, registry, lanes, tmux runtime, event bus, viewport), the per-
  connection socket handling, RPC dispatch, the output streamer, and the background indexer.
- **repomon-tui** (`repomon`) — the terminal client and headless CLI. A `DaemonClient` over
  the socket, an async app loop, the four-zoom views, and the `repomon …` subcommands.

## Key flows

**Fleet refresh.** The TUI calls `lane.list`; the daemon enumerates worktrees (porcelain),
computes live state with gix off the runtime, overlays agent sessions (Claude transcript →
Aider history → tmux-alive fallback), and returns lanes. The TUI renders cached state
immediately on every keystroke; git never runs on the UI thread.

**Live agents.** The daemon spawns each agent in a tmux window (`lane-<id>`). The TUI tells
the daemon which lanes are visible (`viewport.set`); a streamer fast-polls only those panes
(`capture-pane`) and pushes `event.agent.output`. Input goes back via `agent.send_input`
(`send-keys`); `agent.target` + a raw `tmux attach` gives the unmediated session.

**Change propagation.** A debounced notify watcher (250 ms, `.git/objects` excluded) plus the
Claude projects directory feed `event.repo.changed`; the TUI refetches. A 60 s tick is the
safety net.

**History.** On startup and after `repo.add`, the indexer walks HEAD history into SQLite, so
`timeline`, `sessions`, and `commit.search` work over history rather than just live HEAD.

## Performance posture

- All git and tmux work runs in `spawn_blocking`; the reactor never blocks.
- SQLite writes go through one owned connection on a dedicated thread.
- The output streamer only fast-polls *visible* lanes; at rest it does nothing.
- The TUI renders cached state, so first paint doesn't wait on the daemon.

### Measured (release, macOS, 3 fixture repos × 8 commits)

| Gate | Target | Measured |
|---|---|---|
| daemon cold start | < 500 ms | ~15 ms |
| `repomon --print-once` (warm daemon) | < 100 ms | ~38 ms median |
| idle daemon CPU (no agents) | < 1 % | 0.0 % |

(`hyperfine` wasn't available, so these were timed with a `perf_counter` harness over fixture
repos.) The per-lane Claude status lookup is an O(1) encoded-directory check on the hot path;
the encoding-drift fallback scan is reserved for explicit use to keep refresh fast.

See [protocol.md](protocol.md) for the wire API and [agents.md](agents.md) for agent
integration.
