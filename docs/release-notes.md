# Release notes draft

## Fixes

- Keep daemon and managed tmux sockets in a per-user runtime directory so temporary-file cleanup cannot
  disconnect an otherwise live fleet. Adopt and recover surviving legacy tmux servers, recreate an unlinked
  daemon listener within ten minutes, and guard agent restoration while resuming saved Claude and Codex sessions.
