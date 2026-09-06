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

The user guide, current keyboard map, and branding workflow are in
[docs/desktop.md](../../docs/desktop.md). Brand sources are maintainer-local; builds consume
the tracked rendered icons under `src-tauri/icons` and `src-tauri/macos/Assets.car`.

## Packaging

`bun run tauri:build` prepares the daemon, CLI, and platform session sidecars in
`src-tauri/binaries`, then builds the native bundle. Native bundle formats are dmg on macOS,
NSIS on Windows, and AppImage plus deb/rpm on Linux. The Linux packages declare `tmux` as a runtime
dependency.

Signed updater artifacts are produced by `.github/workflows/desktop-release.yml`. Configure these
repository secrets before tagging a release:

- `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
- `TAURI_SIGNING_PUBLIC_KEY`
- Apple certificate, password, signing identity, Apple ID, team ID, and app-specific password for
  notarized macOS releases

The release workflow injects the public key into a temporary Tauri config. The checked-in config
contains the preview verification key and updater endpoint; artifacts must have a matching signature.
