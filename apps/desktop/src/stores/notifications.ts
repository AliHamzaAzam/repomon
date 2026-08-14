import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { createSignal } from "solid-js";

import type { PendingDialog } from "../bindings";
import { desktopSoundPlayer, type SoundCue, type SoundPlayer } from "../audio/sound";
import { setAgentIconOverrides } from "../components/icons";
import { daemonCall, subscribeDaemon, type ConfigView, type DaemonEvent } from "../ipc/rpc";

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

export interface NativeAlert {
  title: string;
  body: string;
  laneId?: number;
}

export type SoundPreferences = Pick<ConfigView,
  | "notify_enabled"
  | "notify_sound"
  | "notify_sound_volume"
  | "notify_sound_unfocused_only"
  | "notify_sound_agent_needs_you"
  | "notify_sound_agent_finished"
  | "notify_sound_repomind_needs_you"
  | "notify_sound_error_or_stall"
  | "notify_sound_incoming_message"
  | "notify_sound_update_ready"
>;

export const DEFAULT_SOUND_PREFERENCES: SoundPreferences = {
  notify_enabled: true,
  notify_sound: true,
  notify_sound_volume: 0.25,
  notify_sound_unfocused_only: true,
  notify_sound_agent_needs_you: true,
  notify_sound_agent_finished: true,
  notify_sound_repomind_needs_you: true,
  notify_sound_error_or_stall: true,
  notify_sound_incoming_message: true,
  notify_sound_update_ready: true,
};

const CUE_TOGGLE: Record<SoundCue, keyof SoundPreferences> = {
  "agent-needs-you": "notify_sound_agent_needs_you",
  "agent-finished": "notify_sound_agent_finished",
  "repomind-needs-you": "notify_sound_repomind_needs_you",
  "error-or-stall": "notify_sound_error_or_stall",
  "incoming-message": "notify_sound_incoming_message",
  "update-ready": "notify_sound_update_ready",
};

function isFleetNotification(value: unknown): value is Omit<FleetNotification, "received_at" | "read"> {
  return typeof value === "object"
    && value !== null
    && "id" in value
    && "lane_id" in value
    && "title" in value
    && "body" in value;
}

export function cueForFleetNotification(notification: Pick<FleetNotification, "kind" | "attention">): SoundCue | null {
  if (notification.kind === "needs_you") {
    if (notification.attention === "permission" || notification.attention === "decision") {
      return "agent-needs-you";
    }
    if (notification.attention === "end_of_turn" || notification.attention === "done_candidate") {
      return "agent-finished";
    }
    return null;
  }
  if (notification.kind === "idle") return "agent-finished";
  if (notification.kind === "stalled" || notification.kind === "rate_limited") return "error-or-stall";
  return null;
}

export interface SoundArbitration {
  eligible: boolean;
  customScheduled: boolean;
  nativeSilent: boolean;
}

/** Custom audio is primary. Native sound is allowed only when an eligible custom cue is unavailable. */
export function arbitrateSound(
  cue: SoundCue | null,
  preferences: SoundPreferences,
  sound: SoundPlayer,
  focused: boolean,
): SoundArbitration {
  const eligible = cue !== null
    && preferences.notify_enabled
    && preferences.notify_sound
    && Boolean(preferences[CUE_TOGGLE[cue]])
    && (!preferences.notify_sound_unfocused_only || !focused);
  if (!eligible || cue === null) return { eligible: false, customScheduled: false, nativeSilent: true };
  const customScheduled = sound.play(cue, preferences.notify_sound_volume);
  return { eligible: true, customScheduled, nativeSilent: customScheduled };
}

export function showNativeAlert(
  alert: NativeAlert,
  silent: boolean,
  onActivate?: (laneId: number) => void,
) {
  try {
    const popup = new Notification(alert.title, { body: alert.body, silent });
    popup.onclick = () => {
      popup.close();
      if (alert.laneId !== undefined) onActivate?.(alert.laneId);
      window.focus();
      const appWindow = getCurrentWindow();
      void appWindow.unminimize().then(() => appWindow.setFocus()).catch(() => undefined);
    };
  } catch {
    sendNotification({ title: alert.title, body: alert.body, silent });
  }
}

export function showNativeNotification(
  notification: FleetNotification,
  onActivate?: (laneId: number) => void,
  silent = true,
) {
  showNativeAlert(
    { title: notification.title, body: notification.body, laneId: notification.lane_id },
    silent,
    onActivate,
  );
}

interface MessageStoredEvent {
  id?: unknown;
  lane_id?: unknown;
  from?: unknown;
  body?: unknown;
}

interface NotificationStoreOptions {
  sound?: SoundPlayer;
  hasFocus?: () => boolean;
  showNative?: (alert: NativeAlert, silent: boolean, onActivate?: (laneId: number) => void) => void;
  loadConfig?: () => Promise<ConfigView>;
  subscribe?: (onEvent: (event: DaemonEvent) => void) => Promise<() => void>;
  now?: () => number;
}

export function createNotificationStore(
  onActivate?: (laneId: number) => void,
  options: NotificationStoreOptions = {},
) {
  const [items, setItems] = createSignal<FleetNotification[]>([]);
  const [nativeEnabled, setNativeEnabled] = createSignal(false);
  const sound = options.sound ?? desktopSoundPlayer;
  const hasFocus = options.hasFocus ?? (() => document.hasFocus());
  const native = options.showNative ?? showNativeAlert;
  const loadConfig = options.loadConfig ?? (() => daemonCall("config.get"));
  const subscribe = options.subscribe ?? subscribeDaemon;
  const now = options.now ?? Date.now;
  let preferences: SoundPreferences = { ...DEFAULT_SOUND_PREFERENCES };
  let configLoaded = false;
  let active = false;
  let unsubscribe: (() => void) | undefined;
  let repomindHadAttention: boolean | undefined;
  const messageIds = new Set<string>();
  const updateVersions = new Set<string>();

  function setConfig(config: Partial<ConfigView>) {
    preferences = { ...preferences, ...config };
    if (config.agent_icons) {
      setAgentIconOverrides(config.agent_icons);
    }
    configLoaded = true;
  }

  async function ensureConfig(): Promise<boolean> {
    if (configLoaded) return true;
    try {
      setConfig(await loadConfig());
      return true;
    } catch {
      // Keep the defaults for this attempt, but permit a later connected call to retry.
      return false;
    }
  }

  function announce(cue: SoundCue | null, alert: NativeAlert) {
    const arbitration = configLoaded
      ? arbitrateSound(cue, preferences, sound, hasFocus())
      : { eligible: false, customScheduled: false, nativeSilent: true };
    if (nativeEnabled()) native(alert, arbitration.nativeSilent, onActivate);
    return arbitration;
  }

  function handleFleetNotification(value: unknown) {
    if (!isFleetNotification(value)) return;
    const notification: FleetNotification = {
      ...value,
      received_at: now(),
      read: false,
    };
    let isNew = false;
    setItems((current) => {
      if (current.some((item) => item.id === notification.id)) return current;
      isNew = true;
      return [notification, ...current].slice(0, 200);
    });
    // Keep cue mapping and every native/custom sound decision behind this exact ID guard.
    if (isNew) {
      announce(
        cueForFleetNotification(notification),
        { title: notification.title, body: notification.body, laneId: notification.lane_id },
      );
    }
  }

  function handleRepomindStatus(value: unknown) {
    const attention = typeof value === "object" && value !== null && "attention" in value
      ? String(value.attention ?? "none")
      : "none";
    const hasAttention = attention !== "none" && attention !== "";
    if (repomindHadAttention === false && hasAttention && preferences.notify_enabled) {
      announce("repomind-needs-you", {
        title: "Repomind needs you",
        body: typeof value === "object" && value !== null && "headline" in value
          ? String(value.headline ?? "Repomind is waiting for a response.")
          : "Repomind is waiting for a response.",
      });
    }
    repomindHadAttention = hasAttention;
  }

  function handleStoredMessage(value: MessageStoredEvent) {
    if (typeof value.id !== "string" || messageIds.has(value.id)) return;
    messageIds.add(value.id);
    if (!preferences.notify_enabled) return;
    const laneId = typeof value.lane_id === "number" ? value.lane_id : undefined;
    announce("incoming-message", {
      title: typeof value.from === "string" ? `Message from ${value.from}` : "New fleet message",
      body: typeof value.body === "string" ? value.body : "A new fleet message is ready.",
      laneId,
    });
  }

  function onDaemonEvent(event: DaemonEvent) {
    if (event.method === "event.config.changed") {
      setConfig(event.params as Partial<ConfigView>);
    } else if (event.method === "event.notification") {
      handleFleetNotification(event.params);
    } else if (event.method === "event.orchestrator.status") {
      handleRepomindStatus(event.params);
    } else if (event.method === "event.message.stored") {
      handleStoredMessage(event.params as MessageStoredEvent);
    }
  }

  async function start() {
    if (active) return;
    active = true;
    await ensureConfig();
    let granted = await isPermissionGranted().catch(() => false);
    if (!granted) {
      granted = await requestPermission().then((permission) => permission === "granted").catch(() => false);
    }
    setNativeEnabled(granted);
    try {
      unsubscribe = await subscribe(onDaemonEvent);
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

  async function notifyUpdateReady(version: string) {
    if (!version || updateVersions.has(version)) return false;
    updateVersions.add(version);
    if (!await ensureConfig()) {
      updateVersions.delete(version);
      return false;
    }
    if (!preferences.notify_enabled) return false;
    announce("update-ready", {
      title: `Repomon ${version} is ready`,
      body: "Open Settings to install the update.",
    });
    return true;
  }

  function preview(cue: SoundCue, volume = preferences.notify_sound_volume) {
    return sound.play(cue, volume);
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
    setConfig,
    notifyUpdateReady,
    preview,
  };
}

export type NotificationStore = ReturnType<typeof createNotificationStore>;
