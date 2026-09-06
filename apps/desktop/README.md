# Repomon Desktop

Run the development checks from `apps/desktop`. The Tauri host is the daemon protocol peer; the Solid frontend uses its typed IPC surface.

## Develop the desktop app

Use an isolated daemon endpoint when testing against a fleet. **Time: 5-10 minutes with Rust and Bun installed; the first native compile can take 10-20 minutes.**

1. Open this directory and install locked dependencies.

   ```sh
   cd apps/desktop
   bun install --frozen-lockfile
   ```

2. Check types, run tests and build the frontend.

   ```sh
   bun run check
   bun run test
   bun run build
   ```

3. Point `REPOMON_SOCKET` at your isolated daemon endpoint, then start the app.

   ```sh
   bun run tauri dev
   ```

**You know it worked when:** all checks exit successfully and the connected desktop window shows the isolated fleet.

<details>
<summary>Details: endpoint and user documentation</summary>

Without `REPOMON_SOCKET`, the app uses `repomon_core::config::socket_path` and starts `repomond` when needed.

The [desktop user guide](../../docs/desktop.md) covers keyboard shortcuts, settings and the branding workflow.

Brand sources are maintainer-local. Builds consume the tracked icons in `src-tauri/icons` and `src-tauri/macos/Assets.car`.

</details>

## Package a release

Configure signing before tagging a release. **Time: 10-20 minutes to build after dependencies and credentials are ready; notarization time varies.**

1. Configure the repository secrets listed below.
2. From `apps/desktop`, prepare sidecars and build the native bundle.

   ```sh
   bun run tauri:build
   ```

3. Use the [desktop release workflow](../../.github/workflows/desktop-release.yml) for signed updater artifacts.

**You know it worked when:** the native bundle exists and its updater artifact has a signature matching the configured public key.

<details>
<summary>Details: formats, sidecars and signing</summary>

The packaging command prepares the daemon, CLI and platform session sidecars in `src-tauri/binaries` before building.

| Platform | Bundle formats |
|---|---|
| macOS | dmg |
| Windows | NSIS |
| Linux | AppImage, deb and rpm; Linux packages declare a tmux runtime dependency |

| Secret group | Required values |
|---|---|
| Updater private key | `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` |
| Updater public key | `TAURI_SIGNING_PUBLIC_KEY` |
| macOS notarization | Apple certificate, password, signing identity, Apple ID, team ID and app-specific password |

The release workflow injects the public key into a temporary Tauri config.

The checked-in config contains the preview verification key and updater endpoint. Artifacts must have a matching signature.

</details>
