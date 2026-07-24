import { For, Show, createEffect, createSignal, untrack } from "solid-js";

import type { TranscriptItem } from "../bindings";
import { daemonCall } from "../ipc/rpc";

interface AgentHistoryProps {
  laneId: number;
  sessionId: string | null;
  visible: boolean;
}

function errorMessage(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

function itemTime(value: string | null) {
  if (!value) return "";
  const date = new Date(value);
  return Number.isNaN(date.valueOf())
    ? ""
    : date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

export default function AgentHistory(props: AgentHistoryProps) {
  let scroller!: HTMLDivElement;
  let activeKey = "";
  let requestEpoch = 0;
  const [items, setItems] = createSignal<TranscriptItem[]>([]);
  const [nextBefore, setNextBefore] = createSignal<number | null | undefined>(undefined);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  async function load(older: boolean) {
    if (loading() || !props.sessionId) return;
    const epoch = requestEpoch;
    const sessionId = props.sessionId;
    let before: number | undefined;
    if (older) {
      const cursor = nextBefore();
      if (cursor == null) return;
      before = cursor;
    }
    const oldHeight = scroller?.scrollHeight ?? 0;
    const oldTop = scroller?.scrollTop ?? 0;

    setLoading(true);
    setError(null);
    try {
      const page = await daemonCall("agent.transcript_page", {
        lane_id: props.laneId,
        session_id: sessionId,
        before,
      });
      if (epoch !== requestEpoch) return;
      setItems((current) => older ? [...page.items, ...current] : page.items);
      setNextBefore(page.next_before);
      requestAnimationFrame(() => {
        if (epoch !== requestEpoch || !scroller) return;
        if (older) {
          scroller.scrollTop = oldTop + scroller.scrollHeight - oldHeight;
        } else {
          scroller.scrollTop = scroller.scrollHeight;
        }
      });
    } catch (cause) {
      if (epoch === requestEpoch) setError(errorMessage(cause));
    } finally {
      if (epoch === requestEpoch) setLoading(false);
    }
  }

  createEffect(() => {
    const key = `${props.laneId}:${props.sessionId ?? ""}`;
    const visible = props.visible;
    if (key !== activeKey) {
      activeKey = key;
      requestEpoch += 1;
      setItems([]);
      setNextBefore(undefined);
      setLoading(false);
      setError(null);
    }
    if (visible && props.sessionId && nextBefore() === undefined && !loading()) {
      untrack(() => void load(false));
    }
  });

  return (
    <div
      ref={scroller}
      class="agent-history h-full overflow-y-auto overscroll-contain bg-background px-4 py-3"
      aria-label="Full agent history"
      onScroll={() => {
        if (
          scroller.scrollTop < 160
          && items().length
          && nextBefore() != null
          && !loading()
        ) {
          void load(true);
        }
      }}
    >
      <div class="mx-auto flex max-w-4xl flex-col gap-3">
        <Show when={props.sessionId} fallback={
          <p class="border-l border-line pl-3 text-xs leading-relaxed text-muted">
            Structured history is not available for this agent session.
          </p>
        }>
          <div class="flex min-h-7 items-center justify-between gap-2">
            <div class="flex flex-1 justify-center">
              <Show
                when={nextBefore() !== null}
                fallback={<span class="font-mono text-[0.52rem] uppercase tracking-[0.08em] text-muted">Beginning of session</span>}
              >
                <button
                  type="button"
                  class="focus-ring rounded border border-line bg-surface px-2 py-1 font-mono text-[0.52rem] uppercase tracking-[0.08em] text-muted hover:border-signal/40 hover:text-foreground disabled:opacity-50"
                  disabled={loading()}
                  onClick={() => void load(Boolean(items().length))}
                >
                  {loading() ? "Loading..." : items().length ? "Load earlier" : "Load history"}
                </button>
              </Show>
            </div>
            <Show when={items().length}>
              <button
                type="button"
                class="focus-ring rounded px-1.5 py-1 font-mono text-[0.5rem] uppercase tracking-[0.08em] text-muted hover:text-foreground disabled:opacity-50"
                title="Reload the newest page"
                aria-label="Refresh latest history"
                disabled={loading()}
                onClick={() => void load(false)}
              >Refresh</button>
            </Show>
          </div>

          <Show when={error()}>
            {(message) => (
              <div role="alert" class="rounded border border-fault/40 bg-fault/8 p-2 text-xs text-fault">
                Could not load agent history: {message()}
              </div>
            )}
          </Show>

          <For each={items()}>
            {(item) => (
              <article class={`agent-history-message is-${item.role}`}>
                <div class="mb-1 flex items-center justify-between gap-3 font-mono text-[0.5rem] uppercase tracking-[0.1em] text-muted">
                  <span>{item.role}</span>
                  <Show when={itemTime(item.at)}>{(time) => <time>{time()}</time>}</Show>
                </div>
                <p class="whitespace-pre-wrap break-words text-xs leading-relaxed">{item.text}</p>
              </article>
            )}
          </For>

          <Show when={nextBefore() !== undefined && !items().length && !loading() && !error()}>
            <p class="border-l border-line pl-3 text-xs leading-relaxed text-muted">
              No conversation messages were recorded for this session.
            </p>
          </Show>
        </Show>
      </div>
    </div>
  );
}
