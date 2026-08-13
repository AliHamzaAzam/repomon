import { createMemo, createSignal } from "solid-js";

import type { FleetMessage, MessagePage } from "../bindings";
import { daemonCall, subscribeDaemon, type DaemonEvent } from "../ipc/rpc";

interface StoredMessageEvent {
  id?: unknown;
  message?: unknown;
}

interface MessageStoreOptions {
  list?: () => Promise<MessagePage>;
  markRead?: (id: string) => Promise<FleetMessage>;
  subscribe?: (onEvent: (event: DaemonEvent) => void) => Promise<() => void>;
}

function isFleetMessage(value: unknown): value is FleetMessage {
  return typeof value === "object"
    && value !== null
    && "id" in value
    && typeof value.id === "string"
    && "sender" in value
    && "recipient" in value
    && "body" in value;
}

export function mergeMessages(current: FleetMessage[], incoming: FleetMessage[]): FleetMessage[] {
  const byId = new Map(current.map((message) => [message.id, message]));
  for (const message of incoming) byId.set(message.id, message);
  return [...byId.values()]
    .sort((left, right) => right.created_at.localeCompare(left.created_at) || right.id.localeCompare(left.id))
    .slice(0, 200);
}

export function createMessageStore(
  onActivate?: (laneId: number, slot?: number | null) => void,
  options: MessageStoreOptions = {},
) {
  const [items, setItems] = createSignal<FleetMessage[]>([]);
  const list = options.list ?? (() => daemonCall("message.list", { limit: 200 }));
  const mark = options.markRead ?? ((id) => daemonCall("message.mark_read", { id }));
  const subscribe = options.subscribe ?? subscribeDaemon;
  let active = false;
  let unsubscribe: (() => void) | undefined;

  const unread = createMemo(() => items().filter((message) => message.read_state === "unread").length);
  const unreadByLane = createMemo(() => {
    const counts = new Map<number, number>();
    for (const message of items()) {
      const laneId = message.recipient.lane_id;
      if (laneId === null || message.read_state !== "unread") continue;
      counts.set(laneId, (counts.get(laneId) ?? 0) + 1);
    }
    return counts;
  });

  async function refresh() {
    const page = await list();
    if (active) setItems((current) => mergeMessages(current, page.messages));
  }

  function onEvent(event: DaemonEvent) {
    if (event.method !== "event.message.stored") return;
    const value = event.params as StoredMessageEvent;
    if (typeof value.id !== "string") return;
    const message = value.message;
    if (isFleetMessage(message)) {
      setItems((current) => mergeMessages(current, [message]));
    } else {
      void refresh().catch(() => undefined);
    }
  }

  async function start() {
    if (active) return;
    active = true;
    await refresh().catch(() => undefined);
    try {
      unsubscribe = await subscribe(onEvent);
    } catch {
      // Browser-only tests and startup reconnects may not have a Tauri channel yet.
    }
  }

  function stop() {
    active = false;
    unsubscribe?.();
    unsubscribe = undefined;
  }

  async function markRead(id: string) {
    const updated = await mark(id);
    setItems((current) => mergeMessages(current, [updated]));
    return updated;
  }

  async function open(message: FleetMessage) {
    const updated = message.read_state === "read" ? message : await markRead(message.id);
    if (updated.recipient.lane_id !== null) {
      onActivate?.(updated.recipient.lane_id, updated.recipient.slot);
    }
  }

  return { items, unread, unreadByLane, refresh, start, stop, markRead, open };
}

export type MessageStore = ReturnType<typeof createMessageStore>;
