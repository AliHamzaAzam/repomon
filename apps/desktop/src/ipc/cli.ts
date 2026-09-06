import { invoke } from "@tauri-apps/api/core";

/// Reports the installed CLI’s health by running its version command rather than checking only file
/// existence.
export interface CliStatus {
  installed: boolean;
  dir: string;
  on_path: boolean | null;
  version: string | null;
  tools: string[];
  missing: string[];
  path_hint: string | null;
  notes: string[];
}

export function cliStatus(): Promise<CliStatus> {
  return invoke<CliStatus>("cli_status");
}

export function cliInstall(): Promise<CliStatus> {
  return invoke<CliStatus>("cli_install");
}

export function cliUninstall(): Promise<CliStatus> {
  return invoke<CliStatus>("cli_uninstall");
}
