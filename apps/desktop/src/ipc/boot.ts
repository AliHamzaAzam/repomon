import { invoke } from "@tauri-apps/api/core";

/// Reports whether the resolved daemon binary can execute, independently of whether it has bound
/// its endpoint.
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
