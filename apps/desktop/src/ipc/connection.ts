import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ConnectionPhase = "starting" | "connecting" | "connected" | "retrying";

export interface DaemonStatus {
  uptime_secs: number;
  repos: number;
  lanes: number;
  db_size_bytes: number;
  version: string;
}

export interface ConnectionSnapshot {
  phase: ConnectionPhase;
  endpoint: string;
  message: string | null;
  daemon: DaemonStatus | null;
}

export interface ConnectionSource {
  current(): Promise<ConnectionSnapshot>;
  subscribe(onSnapshot: (snapshot: ConnectionSnapshot) => void): Promise<UnlistenFn>;
}

export const initialConnection: ConnectionSnapshot = {
  phase: "starting",
  endpoint: "Resolving local daemon endpoint",
  message: null,
  daemon: null,
};

export function getConnectionStatus(): Promise<ConnectionSnapshot> {
  return invoke<ConnectionSnapshot>("connection_status");
}

export function subscribeConnection(
  onSnapshot: (snapshot: ConnectionSnapshot) => void,
): Promise<UnlistenFn> {
  return listen<ConnectionSnapshot>("connection-state", (event) => onSnapshot(event.payload));
}

export const tauriConnectionSource: ConnectionSource = {
  current: getConnectionStatus,
  subscribe: subscribeConnection,
};
