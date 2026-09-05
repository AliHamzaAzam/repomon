import { invoke } from "@tauri-apps/api/core";

/// The result of running `repomond --version` from wherever the app resolves the daemon. This is
/// the one check that separates "the daemon is slow to bind" from "the daemon binary cannot run
/// on this machine at all", which is the failure the connection pill used to hide behind an
/// endless "Retrying".
export interface DaemonBootCheck {
  ok: boolean;
  path: string;
  version: string | null;
  error: string | null;
  hint: string | null;
  log_path: string;
}

/// Whether the webview is running inside the Tauri shell. Outside it (unit tests, `vite preview`)
/// there is no command bridge, so callers skip the probe instead of rendering a failure that says
/// nothing about the user's machine.
export function hasTauriBridge(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export function daemonBootCheck(): Promise<DaemonBootCheck> {
  return invoke<DaemonBootCheck>("daemon_boot_check");
}

export function openDaemonLog(): Promise<void> {
  return invoke("open_daemon_log");
}

export function daemonDiagnostics(): Promise<string> {
  return invoke<string>("daemon_diagnostics");
}
