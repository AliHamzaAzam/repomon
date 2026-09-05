import { invoke } from "@tauri-apps/api/core";

/// What the app knows about the `repomon` command line on this machine. The version is read by
/// running the installed copy, so it reports a working CLI rather than a file of the right name.
export interface CliStatus {
  installed: boolean;
  dir: string;
  on_path: boolean;
  version: string | null;
  tools: string[];
  missing: string[];
  path_hint: string | null;
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
