import { relaunch } from "@tauri-apps/plugin-process";
import { check, type DownloadEvent } from "@tauri-apps/plugin-updater";

export interface UpdateProgress {
  version: string;
  downloaded: number;
  total?: number;
}

export interface AvailableUpdate {
  version: string;
  install: (onProgress: (progress: UpdateProgress) => void) => Promise<void>;
}

/// Check for an update without installing it. Returns a handle to install later (used by the
/// launch-time banner), or null when the app is current. Throws outside a Tauri build.
export async function checkForUpdate(): Promise<AvailableUpdate | null> {
  const update = await check({ timeout: 15_000 });
  if (!update) return null;
  return {
    version: update.version,
    install: async (onProgress) => {
      let downloaded = 0;
      let total: number | undefined;
      await update.downloadAndInstall((event: DownloadEvent) => {
        if (event.event === "Started") total = event.data.contentLength;
        if (event.event === "Progress") downloaded += event.data.chunkLength;
        onProgress({ version: update.version, downloaded, total });
      });
      await relaunch();
    },
  };
}

export async function installAvailableUpdate(
  onProgress: (progress: UpdateProgress) => void,
): Promise<"current" | "relaunching"> {
  const update = await check({ timeout: 15_000 });
  if (!update) return "current";
  let downloaded = 0;
  let total: number | undefined;
  await update.downloadAndInstall((event: DownloadEvent) => {
    if (event.event === "Started") total = event.data.contentLength;
    if (event.event === "Progress") downloaded += event.data.chunkLength;
    onProgress({ version: update.version, downloaded, total });
  });
  await relaunch();
  return "relaunching";
}
