import { invoke } from "@tauri-apps/api/core";

export interface DaemonServiceInfo {
  service_managed: boolean;
  status: string;
}

export function getDaemonServiceInfo(): Promise<DaemonServiceInfo> {
  return invoke<DaemonServiceInfo>("daemon_service_info");
}

export function stopDaemon(): Promise<void> {
  return invoke("daemon_stop");
}

export function startDaemon(): Promise<void> {
  return invoke("daemon_start");
}

export function restartDaemon(): Promise<void> {
  return invoke("daemon_restart");
}
