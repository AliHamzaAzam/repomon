import { createMemo, createSignal } from "solid-js";

import type { FleetMessage, MessagePage } from "../bindings";
import { daemonCall, subscribeDaemon, type DaemonEvent } from "../ipc/rpc";

interface StoredMessageEvent {
  id?: unknown;
  message?: unknown;
}

interface MessageStoreOptions {
  list?: (params?: { limit?: number; before?: string }) => Promise<MessagePage>;
  markRead?: (id: string) => Promise<FleetMessage>;
  forceSend?: (id: string) => Promise<FleetMessage>;
  deleteMessage?: (id: string) => Promise<void>;
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
    .sort((left, right) => right.created_at.localeCompare(left.created_at) || right.id.localeCompare(left.id));
}

export function createMessageStore(
  onActivate?: (laneId: number, slot?: number | null, window?: string | null) => void,
  options: MessageStoreOptions = {},
) {
  const [items, setItems] = createSignal<FleetMessage[]>([]);
  const [nextBefore, setNextBefore] = createSignal<string | null>(null);
  const list = options.list ?? ((params) => daemonCall("message.list", { limit: 200, ...params }));
  const mark = options.markRead ?? ((id) => daemonCall("message.mark_read", { id }));
  const force = options.forceSend ?? ((id) => daemonCall("message.force_send", { id }));
  const remove = options.deleteMessage ?? ((id) => daemonCall("message.delete", { id }));
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
    const page = await list({ limit: 200 });
    if (active) {
      setItems((current) => mergeMessages(current, page.messages));
      setNextBefore(page.next_before);
    }
  }

  async function loadMore() {
    const before = nextBefore();
    if (!before) return;
    const page = await list({ limit: 200, before });
    if (active) {
      setItems((current) => mergeMessages(current, page.messages));
      setNextBefore(page.next_before);
    }
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

  async function forceSend(id: string) {
    const updated = await force(id);
    setItems((current) => mergeMessages(current, [updated]));
    return updated;
  }

  async function deleteMessage(id: string) {
    await remove(id);
    setItems((current) => current.filter((message) => message.id !== id));
  }

  async function open(message: FleetMessage) {
    const updated = message.read_state === "read" ? message : await markRead(message.id);
    const source = updated.sender.lane_id !== null ? updated.sender : updated.recipient;
    if (source.lane_id !== null) {
      onActivate?.(source.lane_id, source.slot, source.window);
    }
  }

  return {
    items,
    unread,
    unreadByLane,
    nextBefore,
    refresh,
    loadMore,
    start,
    stop,
    markRead,
    forceSend,
    deleteMessage,
    open,
  };
}

export type MessageStore = ReturnType<typeof createMessageStore>;
