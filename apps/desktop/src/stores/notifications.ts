import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { createSignal } from "solid-js";

import type { PendingDialog } from "../bindings";
import { subscribeDaemon } from "../ipc/rpc";

export interface FleetNotification {
  id: string;
  lane_id: number;
  session_id?: string | null;
  kind: "needs_you" | "rate_limited" | "resumed" | "idle" | "stalled";
  title: string;
  body: string;
  prompt?: string | null;
  attention: string;
  dialog?: PendingDialog | null;
  received_at: number;
  read: boolean;
}

function isFleetNotification(value: unknown): value is Omit<FleetNotification, "received_at" | "read"> {
  return typeof value === "object"
    && value !== null
    && "id" in value
    && "lane_id" in value
    && "title" in value
    && "body" in value;
}

export function createNotificationStore() {
  const [items, setItems] = createSignal<FleetNotification[]>([]);
  const [nativeEnabled, setNativeEnabled] = createSignal(false);
  let active = false;
  let unsubscribe: (() => void) | undefined;

  async function start() {
    if (active) return;
    active = true;
    setNativeEnabled(await isPermissionGranted().catch(() => false));
    try {
      unsubscribe = await subscribeDaemon((event) => {
        if (event.method !== "event.notification" || !isFleetNotification(event.params)) return;
        const notification: FleetNotification = {
          ...event.params,
          received_at: Date.now(),
          read: false,
        };
        setItems((current) => {
          if (current.some((item) => item.id === notification.id)) return current;
          return [notification, ...current].slice(0, 200);
        });
        if (nativeEnabled()) {
          sendNotification({ title: notification.title, body: notification.body });
        }
      });
    } catch {
      // Browser-only tests and the brief startup gap have no Tauri channel yet.
    }
  }

  function stop() {
    active = false;
    unsubscribe?.();
    unsubscribe = undefined;
  }

  async function enableNative() {
    const permission = await requestPermission();
    const granted = permission === "granted";
    setNativeEnabled(granted);
    return granted;
  }

  function markAllRead() {
    setItems((current) => current.map((item) => ({ ...item, read: true })));
  }

  function clear() {
    setItems([]);
  }

  return {
    items,
    unread: () => items().filter((item) => !item.read).length,
    nativeEnabled,
    enableNative,
    markAllRead,
    clear,
    start,
    stop,
  };
}

export type NotificationStore = ReturnType<typeof createNotificationStore>;
