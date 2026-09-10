/// Dev-only screenshot fixture, aliased in for `vite.screenshot.config.ts` over the real
/// `@tauri-apps/api/event`. `ipc/connection.ts` calls `listen("connection-state", ...)` for
/// live connection updates; the screenshot only needs the one-shot `connection_status` fixture
/// in `tauriCore.fixture.ts`; this never fires, so it resolves to a no-op unlisten instead of
/// throwing.
export async function listen(): Promise<() => void> {
  return () => undefined;
}
