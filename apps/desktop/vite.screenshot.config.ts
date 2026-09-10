/// Dev-only config for the qa/ screenshot harness: the real app, the real index.css tokens,
/// the real icons, against fixture data instead of the real daemon IPC (which needs the Tauri
/// shell). Never used by `tauri dev`, `vite build`, or `vitest` - those all still resolve
/// `vite.config.ts`. Run with:
///   bunx --bun vite --config vite.screenshot.config.ts --port <port>
///
/// The real IPC seam is `@tauri-apps/api/core`'s `invoke` (and `@tauri-apps/api/event`'s
/// `listen`), not `ipc/rpc.ts` alone: `ipc/connection.ts`'s connection probe and a handful of
/// other `ipc/*.ts` files call `invoke` directly, one level below `daemonCall`. Redirecting
/// those two packages, via a resolveId hook rather than `resolve.alias`, covers every one of
/// them while letting the real `daemonCall`/`getConnectionStatus`/etc. run unmodified against
/// fixture data. A plain `resolve.alias` on the bare package specifier would also catch the
/// fixture modules' own re-export of the real package, so this uses `importer` to exempt them.
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";
import solid from "vite-plugin-solid";

const CORE_FIXTURE = fileURLToPath(new URL("./src/ipc/tauriCore.fixture.ts", import.meta.url));
const EVENT_FIXTURE = fileURLToPath(new URL("./src/ipc/tauriEvent.fixture.ts", import.meta.url));

function tauriFixtureRedirect(): Plugin {
  return {
    name: "screenshot-tauri-fixture-redirect",
    enforce: "pre",
    resolveId(source, importer) {
      if (source === "@tauri-apps/api/core" && importer !== CORE_FIXTURE) return CORE_FIXTURE;
      if (source === "@tauri-apps/api/event" && importer !== EVENT_FIXTURE) return EVENT_FIXTURE;
      return null;
    },
  };
}

export default defineConfig({
  server: { fs: { allow: [fileURLToPath(new URL("../../", import.meta.url))] } },
  plugins: [tauriFixtureRedirect(), solid(), tailwindcss()],
  resolve: {
    alias: [{ find: "@", replacement: fileURLToPath(new URL("./src", import.meta.url)) }],
  },
});
