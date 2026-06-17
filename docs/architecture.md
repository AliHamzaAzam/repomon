# Architecture

repomon is a background **daemon** plus a thin **TUI** client, sharing a **core** library.
The daemon owns all state and all git/tmux work; the TUI only renders cached state and
forwards input. That split is what keeps the UI instant and lets agents outlive the UI.

```
      ┌───────────────────────── repomon-core ────────────────────────┐
      │ model · store(SQLite) · git(gix + worktree shellout) · watch  │
      │ registry · lane · agent(tmux runtime + Claude/Aider monitors) │
      │ analytics · session · indexer · service(launchd) · protocol   │
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

- **repomon-core** — the engine. No UI, no socket server. Holds the data model, the SQLite
  store (a dedicated writer thread; never blocks the runtime), the gix-backed git reader and
  `git worktree` shellout, the notify watcher, the repo registry and lane manager, the
  tmux-backed agent runtime and the agent monitors, the Phase-3 analytics/sessions/indexer,
  launchd service management, and the shared JSON-RPC protocol + framing.
- **repomon-daemon** (`repomond`) — hosts core behind a length-prefixed JSON-RPC Unix socket.
  Owns the `Ctx` (store, registry, lanes, tmux runtime, event bus, viewport + focus, and the
  overlay/status caches), the per-connection socket handling, RPC dispatch, the output streamer,
  the desktop/remote notification watcher, and the background indexer.
- **repomon-tui** (`repomon`) — the terminal client and headless CLI. A `DaemonClient` over
  the socket, an async app loop, the four-zoom views, and the `repomon …` subcommands.

## Key flows

**Fleet refresh.** The TUI calls `lane.list`; the daemon enumerates worktrees (porcelain),
computes live state with gix off the runtime, overlays agent sessions (Claude transcript →
Aider history → tmux-alive fallback), and returns lanes. The TUI renders cached state
immediately on every keystroke; git never runs on the UI thread.

**Live agents.** The daemon spawns each agent in its own tmux window — the first at
`lane-<id>`, additional agents sharing a worktree at `lane-<id>-2`, `lane-<id>-3`, … so
several run side by side. The TUI tells the daemon which lanes are visible (and which agent
window the focused lane should stream) via `viewport.set`; a streamer fast-polls only those
panes (`capture-pane`) and pushes `event.agent.output`. Input goes back via `agent.send_input`
(`send-keys`), routed to a specific window in a multi-agent lane; `agent.target` + a raw
`tmux attach` gives the unmediated session.

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
