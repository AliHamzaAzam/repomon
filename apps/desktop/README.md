# Repomon Desktop

The multiplatform mission-control client for Repomon. The Tauri host is the only daemon protocol
peer; the Solid frontend talks to it through a small typed IPC surface.

## Development

```sh
bun install --frozen-lockfile
bun run check
bun run test
bun run build
bun run tauri dev
```

Set `REPOMON_SOCKET` to point the host at an isolated daemon endpoint. Without it, the app uses
the endpoint resolved by `repomon_core::config::socket_path` and starts `repomond` when needed.

The approved design lives at
`../../docs/superpowers/specs/2026-07-20-desktop-gui-design.md`.
