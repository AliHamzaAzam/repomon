# Repomon Desktop

Repomon's multiplatform desktop client uses a Tauri host as its only daemon protocol peer. The Solid frontend communicates through typed IPC. The [desktop guide](../../docs/desktop.md) covers shortcuts, settings and branding; maintainer-local brand sources produce the tracked `src-tauri/icons` and `src-tauri/macos/Assets.car` assets.

## Development

1. From `apps/desktop`, install locked dependencies and run the frontend checks.

   ```sh
   bun install --frozen-lockfile
   bun run check && bun run test
   bun run build
   ```

2. Set `REPOMON_SOCKET` to an isolated daemon endpoint, then launch the app. Without the override, it uses `repomon_core::config::socket_path` and starts `repomond` when needed.

   ```sh
   bun run tauri dev
   ```

## Packaging

1. Configure the signing secrets below before tagging a release. The [release workflow](../../.github/workflows/desktop-release.yml) produces signed updater artifacts and injects the public key into a temporary Tauri config. The checked-in config contains the preview verification key and updater endpoint; artifacts need a matching signature.
2. From `apps/desktop`, prepare the daemon, CLI and platform session sidecars in `src-tauri/binaries`, then build the native bundle.

   ```sh
   bun run tauri:build
   ```

| Platform | Bundle formats |
|---|---|
| macOS | dmg |
| Windows | NSIS |
| Linux | AppImage, deb, rpm; packages declare a tmux runtime dependency |

| Signing purpose | Repository secrets |
|---|---|
| Updater | `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, `TAURI_SIGNING_PUBLIC_KEY` |
| macOS notarization | Apple certificate, password, signing identity, Apple ID, team ID, app-specific password |
