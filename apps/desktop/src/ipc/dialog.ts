import { open } from "@tauri-apps/plugin-dialog";

/// Native OS folder picker. Returns the chosen absolute path, or null if cancelled.
export async function pickDirectory(title: string): Promise<string | null> {
  const result = await open({ directory: true, multiple: false, title });
  return typeof result === "string" ? result : null;
}
