---
name: verify
description: Drive a real repomon TUI + daemon end-to-end in an isolated instance (own socket, DB, config, tmux server) without touching the live fleet.
---

# Verifying repomon changes live

**Branch first.** Feature work starts with `git checkout -b feat/...` BEFORE the first
edit — twice now a whole feature landed on local main and had to be surgically moved
(`git checkout -b <branch> && git branch -f main origin/main`).

## Isolated instance (never touches the real fleet)

The real daemon runs on the default socket with tmux server `repomon`. An isolated
instance needs ALL FOUR of: config home, data dir, socket, and a distinct
`tmux_session` (the tmux server socket is named after the session, so this is what
prevents `lane-N` window collisions with the real fleet):

```sh
mkdir -p /tmp/rmv/cfg/repomon /tmp/rmv/data /tmp/rmv/repo
cat > /tmp/rmv/cfg/repomon/config.toml <<'EOF'
tmux_session = "rmv"
default_agent = "fake"
spawn_prompt = false          # 'e' spawns the default agent with no picker
[agents]
fake = "/tmp/rmv/fake-agent.sh"
EOF
export XDG_CONFIG_HOME=/tmp/rmv/cfg REPOMON_DATA_DIR=/tmp/rmv/data
alias rmv='~/.cargo/bin/repomon --socket /tmp/rmv/repomon.sock'
```

`rmv add <repo>` auto-spawns an isolated daemon (`repomond` must be installed —
`cargo install --path crates/repomon-daemon`; the TUI: `--path crates/repomon-tui`).

## Faking an agent state

Any script registered as a custom agent becomes a "real" agent: the daemon sniffs
its tmux pane. To fake a permission dialog, print a Claude-style box
(`╭─╮` + `│` borders, a "Do you want to proceed?" line, `❯ 1. …` options), then
`read _` so answering (Enter) visibly changes the pane. Running-status sniff TTL is
5s — wait ~6-8s after spawn for the ⏸ flip.

## Driving the TUI

Run the TUI inside your own driver tmux server (separate from both the real and
isolated agent servers):

```sh
tmux -L drv new-session -d -s v -x 170 -y 45 "env XDG_CONFIG_HOME=... REPOMON_DATA_DIR=... ~/.cargo/bin/repomon --socket /tmp/rmv/repomon.sock"
tmux -L drv send-keys -t v <keys>; tmux -L drv capture-pane -t v -p
```

Agent panes live on the isolated server: `tmux -L rmv capture-pane -t "rmv:=lane-1" -p`.

## Gotchas

- `e` (spawn) auto-focuses into Split; send Escape to get back to Fleet before
  reading fleet badges.
- **Esc in Fleet QUITS the TUI** (zoom out past the top level). Never send a
  "just in case" Escape when already in Fleet — the driver session dies and
  later captures report "no server running". Track which view you're in.
- The Notifications view (`5`) swallows most keys; Esc out before pressing
  global keys like `v`.
- `repomon daemon stop|status|restart` honor `--socket` since fix/daemon-cli-socket
  (2026-07-07); on OLDER installed binaries they resolve from config and hit the
  REAL daemon — with an old binary, stop isolated daemons by pid instead.
- Time-based states (stall = 5 min of frozen pane): set the scenario up, verify
  the negative early, and come back on a background timer; poking the agent pane
  (`tmux send-keys -l "x"` — tty echo changes the content) resets the clock for
  edge-refire tests. Launch the driver with `remain-on-exit on` so a dying TUI
  leaves its exit status behind.
- After a rebuild the USER's real daemon still runs old code: finish with
  `repomon daemon restart` (agents survive — they live in tmux).
- Teardown: quit the TUI (`q`), kill the isolated daemon by pid, kill both tmux
  servers (`tmux -L drv kill-server`, `tmux -L rmv kill-server`), rm the tmp dir.
