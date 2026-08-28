import { For, Show, createMemo, createSignal, type JSX } from "solid-js";

import type { FleetMessage } from "../bindings";
import { translateError } from "../ipc/errors";
import type { ActionsStore } from "../stores/actions";
import type { FleetStore } from "../stores/fleet";
import type { MessageStore } from "../stores/messages";
import { formatTime } from "./automation";
import Select from "./controls/Select";
import { IconChevronRight, IconMail, IconRefresh, IconTrash, IconZap } from "./icons";

export interface MailPanelProps {
  fleet?: FleetStore;
  messages?: MessageStore;
  actions?: ActionsStore;
}

export interface MailThread {
  id: string;
  messages: FleetMessage[];
}

export function groupMailThreads(messages: FleetMessage[]): MailThread[] {
  const byThread = new Map<string, FleetMessage[]>();
  for (const message of messages) {
    const thread = byThread.get(message.thread_id);
    if (thread) thread.push(message);
    else byThread.set(message.thread_id, [message]);
  }
  return [...byThread.entries()]
    .map(([id, thread]) => ({
      id,
      messages: [...thread].sort((left, right) =>
        right.created_at.localeCompare(left.created_at) || right.id.localeCompare(left.id)),
    }))
    .sort((left, right) =>
      right.messages[0].created_at.localeCompare(left.messages[0].created_at)
      || right.id.localeCompare(left.id));
}

function messageLane(message: FleetMessage): number | null {
  return message.sender.lane_id ?? message.recipient.lane_id;
}

function sourceAddress(message: FleetMessage) {
  return message.sender.lane_id !== null ? message.sender : message.recipient;
}

function deliveryLabel(message: FleetMessage): string {
  return message.delivery_state === "delivered" ? "sent" : message.delivery_state;
}

function deliveryClass(message: FleetMessage): string {
  switch (message.delivery_state) {
    case "delivered": return "bg-signal/15 text-signal";
    case "failed": return "bg-fault/15 text-fault";
    default: return "bg-attention/15 text-attention";
  }
}

export default function MailPanel(props: MailPanelProps): JSX.Element {
  const [laneFilter, setLaneFilter] = createSignal("all");
  const [unreadOnly, setUnreadOnly] = createSignal(false);
  const [loading, setLoading] = createSignal(false);
  const [busyId, setBusyId] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);

  const laneOptions = createMemo(() => [
    { value: "all", label: "All lanes" },
    ...(props.fleet?.lanes() ?? []).map((lane) => ({
      value: String(lane.id),
      label: `${lane.repo.label || lane.repo.name} · ${lane.worktree.branch || lane.worktree.name}`,
    })),
  ]);
  const laneLabels = createMemo(() => new Map(
    laneOptions()
      .filter((option) => option.value !== "all")
      .map((option) => [Number(option.value), option.label]),
  ));

  const filtered = createMemo(() => {
    const selectedLane = laneFilter() === "all" ? null : Number(laneFilter());
    return (props.messages?.items() ?? []).filter((message) => {
      const laneMatches = selectedLane === null
        || message.sender.lane_id === selectedLane
        || message.recipient.lane_id === selectedLane;
      return laneMatches && (!unreadOnly() || message.read_state === "unread");
    });
  });
  const threads = createMemo(() => groupMailThreads(filtered()));

  async function run(action: () => Promise<unknown>, id?: string) {
    if (id) setBusyId(id);
    else setLoading(true);
    setError(null);
    try {
      await action();
    } catch (cause) {
      setError(translateError(cause).friendly);
    } finally {
      if (id) setBusyId(null);
      else setLoading(false);
    }
  }

  function refresh() {
    if (props.messages) void run(() => props.messages!.refresh());
  }

  function loadOlder() {
    if (props.messages) void run(() => props.messages!.loadMore());
  }

  function confirmDelete(message: FleetMessage) {
    if (!props.actions || !props.messages) return;
    props.actions.confirm({
      title: "Delete repomail?",
      message: "Permanently remove this message from its thread. This cannot be undone.",
      confirmLabel: "Delete",
      danger: true,
      onConfirm: async () => {
        setBusyId(message.id);
        setError(null);
        try {
          await props.messages!.deleteMessage(message.id);
        } catch (cause) {
          setError(translateError(cause).friendly);
          throw cause;
        } finally {
          setBusyId(null);
        }
      },
    });
  }

  return (
    <div class="flex h-full min-h-0 flex-col bg-surface">
      <div class="flex h-10 shrink-0 items-center justify-between border-b border-line bg-surface/95 px-3.5">
        <div class="flex min-w-0 items-center gap-2">
          <IconMail size={14} class="shrink-0 text-foreground" />
          <span class="text-xs font-semibold text-foreground">Repomail</span>
          <Show when={(props.messages?.unread() ?? 0) > 0}>
            <span class="rounded-full bg-signal/15 px-1.5 py-0.5 font-mono text-[9px] font-semibold text-signal">
              {props.messages!.unread()} unread
            </span>
          </Show>
        </div>

        <button
          type="button"
          class="focus-ring flex size-6 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
          onClick={refresh}
          disabled={!props.messages || loading()}
          title="Refresh repomail"
          aria-label="Refresh repomail"
        >
          <IconRefresh size={12} class={loading() ? "animate-spin" : ""} />
        </button>
      </div>

      <div class="flex shrink-0 items-center gap-2 border-b border-line bg-raised/15 px-3 py-2">
        <Select
          ariaLabel="Filter repomail by lane"
          size="sm"
          class="min-w-0 flex-1"
          value={laneFilter()}
          options={laneOptions()}
          onChange={setLaneFilter}
        />
        <button
          type="button"
          class={`focus-ring h-7 shrink-0 rounded-lg border px-2.5 font-mono text-[10px] uppercase tracking-wider transition-colors ${
            unreadOnly()
              ? "border-signal/40 bg-signal/15 text-signal"
              : "border-line bg-surface text-muted hover:text-foreground"
          }`}
          aria-pressed={unreadOnly()}
          onClick={() => setUnreadOnly((value) => !value)}
        >
          Unread
        </button>
      </div>

      <Show when={error()} keyed>
        {(message) => (
          <div role="alert" class="m-3 mb-0 flex items-start justify-between gap-3 rounded-xl border border-fault/30 bg-fault/10 p-3 text-xs text-fault">
            <div class="min-w-0">
              <p class="font-semibold">Couldn't update repomail</p>
              <p class="mt-0.5 break-words text-fault/80">{message}</p>
            </div>
            <button
              type="button"
              class="focus-ring shrink-0 rounded-lg border border-fault/40 bg-surface px-2.5 py-1 text-xs font-medium text-foreground transition-colors hover:bg-fault/20"
              onClick={refresh}
            >
              Retry
            </button>
          </div>
        )}
      </Show>

      <div class="min-h-0 flex-1 overflow-y-auto p-3">
        <Show
          when={threads().length > 0}
          fallback={(
            <div class="flex min-h-48 items-center justify-center">
              <div class="max-w-[230px] text-center">
                <p class="text-xs font-medium text-foreground">
                  {(props.messages?.items().length ?? 0) > 0 ? "No messages match these filters" : "No repomail yet"}
                </p>
                <p class="mt-1 text-[11px] leading-relaxed text-muted">
                  {(props.messages?.items().length ?? 0) > 0
                    ? "Try another lane or include messages already read."
                    : "Fleet messages will appear here as soon as they are stored."}
                </p>
              </div>
            </div>
          )}
        >
          <div class="space-y-4">
            <For each={threads()}>
              {(thread) => {
                const root = () => thread.messages[thread.messages.length - 1];
                const laneLabel = () => {
                  const laneId = messageLane(root());
                  return laneId === null ? null : laneLabels().get(laneId) ?? null;
                };
                return (
                  <section aria-label={`Mail thread ${thread.id}`}>
                    <div class="mb-1.5 flex min-w-0 items-center justify-between gap-2 px-1">
                      <div class="min-w-0">
                        <div class="flex min-w-0 items-center gap-1 font-mono text-[10px] text-muted">
                          <span class="max-w-[7rem] truncate">{root().sender.address}</span>
                          <IconChevronRight size={9} class="shrink-0 text-muted/60" />
                          <span class="max-w-[7rem] truncate">{root().recipient.address}</span>
                          <span class="shrink-0 text-muted/60">· {thread.messages.length} {thread.messages.length === 1 ? "message" : "messages"}</span>
                        </div>
                        <Show when={laneLabel()} keyed>
                          {(label) => <div class="mt-0.5 truncate text-[10px] text-foreground/70" title={label}>{label}</div>}
                        </Show>
                      </div>
                      <span class="shrink-0 font-mono text-[9px] text-muted/60" title={thread.id}>
                        {thread.id.slice(0, 8)}
                      </span>
                    </div>

                    <div class="divide-y divide-line/70 overflow-hidden rounded-xl border border-line bg-raised/15">
                      <For each={thread.messages}>
                        {(message) => {
                          const source = () => sourceAddress(message);
                          return (
                            <article class="p-3 text-xs">
                              <div class="flex min-w-0 items-start justify-between gap-2">
                                <div class="flex min-w-0 items-center gap-1 font-mono text-[10px] text-muted">
                                  <span class="max-w-[8rem] truncate font-semibold text-foreground">{message.sender.address}</span>
                                  <IconChevronRight size={9} class="shrink-0 text-muted/60" />
                                  <span class="max-w-[8rem] truncate">{message.recipient.address}</span>
                                </div>
                                <span class={`shrink-0 rounded-full px-1.5 py-0.5 font-mono text-[9px] font-semibold uppercase ${deliveryClass(message)}`}>
                                  {deliveryLabel(message)}
                                </span>
                              </div>

                              <p class="mt-2 max-h-32 overflow-y-auto whitespace-pre-wrap break-words text-xs leading-relaxed text-foreground/90">
                                {message.body}
                              </p>

                              <Show when={message.delivery_error}>
                                {(deliveryError) => (
                                  <p class="mt-2 break-words rounded-lg bg-fault/10 px-2 py-1.5 font-mono text-[10px] leading-relaxed text-fault">
                                    {deliveryError()}
                                  </p>
                                )}
                              </Show>

                              <div class="mt-2.5 flex flex-wrap items-center justify-between gap-2 border-t border-line/50 pt-2">
                                <div class="min-w-0 font-mono text-[9px] uppercase tracking-wide text-muted">
                                  <span>{formatTime(message.created_at)}</span>
                                  <span class="mx-1 text-muted/40">·</span>
                                  <span class={message.read_state === "unread" ? "text-attention" : ""}>{message.read_state}</span>
                                </div>
                                <div class="ml-auto flex flex-wrap items-center justify-end gap-1">
                                  <Show when={message.delivery_state !== "delivered" && message.recipient.lane_id !== null}>
                                    <button
                                      type="button"
                                      class="focus-ring flex items-center gap-1 rounded px-1.5 py-1 font-mono text-[9px] font-semibold text-attention transition-colors hover:bg-attention/15 disabled:opacity-40"
                                      disabled={!props.messages || busyId() === message.id}
                                      onClick={() => props.messages && void run(() => props.messages!.forceSend(message.id), message.id)}
                                    >
                                      <IconZap size={8} />
                                      Force send
                                    </button>
                                  </Show>
                                  <Show when={message.read_state === "unread"}>
                                    <button
                                      type="button"
                                      class="focus-ring rounded px-1.5 py-1 font-mono text-[9px] text-muted transition-colors hover:bg-surface hover:text-foreground disabled:opacity-40"
                                      disabled={busyId() === message.id}
                                      onClick={() => props.messages && void run(() => props.messages!.markRead(message.id), message.id)}
                                    >
                                      Mark read
                                    </button>
                                  </Show>
                                  <Show when={messageLane(message) !== null}>
                                    <button
                                      type="button"
                                      class="focus-ring flex items-center gap-1 rounded bg-surface px-1.5 py-1 font-mono text-[9px] font-semibold text-foreground transition-colors hover:bg-signal/15 hover:text-signal disabled:opacity-40"
                                      disabled={!props.messages || busyId() === message.id}
                                      aria-label={`Open source ${source().address}`}
                                      onClick={() => props.messages && void run(() => props.messages!.open(message), message.id)}
                                    >
                                      Open lane
                                      <IconChevronRight size={8} />
                                    </button>
                                  </Show>
                                  <button
                                    type="button"
                                    class="focus-ring flex items-center gap-1 rounded px-1.5 py-1 font-mono text-[9px] text-muted transition-colors hover:bg-fault/10 hover:text-fault disabled:opacity-40"
                                    disabled={!props.actions || !props.messages || busyId() === message.id}
                                    onClick={() => confirmDelete(message)}
                                  >
                                    <IconTrash size={8} />
                                    Delete
                                  </button>
                                </div>
                              </div>
                            </article>
                          );
                        }}
                      </For>
                    </div>
                  </section>
                );
              }}
            </For>

            <Show when={props.messages?.nextBefore()}>
              <button
                type="button"
                class="focus-ring flex h-8 w-full items-center justify-center rounded-lg border border-line bg-raised/20 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground disabled:opacity-40"
                disabled={loading()}
                onClick={loadOlder}
              >
                {loading() ? "Loading…" : "Load older messages"}
              </button>
            </Show>
          </div>
        </Show>
      </div>
    </div>
  );
}
