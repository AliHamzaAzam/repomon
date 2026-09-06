import { open } from "@tauri-apps/plugin-dialog";

/// Returns the absolute path chosen by the native folder picker, or null on cancellation.
export async function pickDirectory(title: string): Promise<string | null> {
  const result = await open({ directory: true, multiple: false, title });
  return typeof result === "string" ? result : null;
}
