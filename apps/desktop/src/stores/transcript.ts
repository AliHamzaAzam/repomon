import { createEffect, createSignal, onCleanup } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import type { TranscriptItem } from "../bindings";
import { daemonCall, subscribeDaemon, type TranscriptTarget, type TranscriptUpdate } from "../ipc/rpc";

export interface ConversationRow { key: string; item: TranscriptItem; fallback: boolean; paneExcerpt?: boolean }
const KINDS = new Set(["user", "assistant", "tool_call", "dialog", "status", "terminal_block"]);

export function transcriptRow(value: unknown, fallbackKey: string): ConversationRow {
  const raw = value && typeof value === "object" ? value as Record<string, unknown> : {};
  const key = typeof raw.id === "string" && raw.id ? raw.id : fallbackKey;
  const text = typeof raw.text === "string" ? raw.text : typeof value === "string" ? value : JSON.stringify(value) ?? String(value);
  const kind = raw.kind ?? (raw.role === "user" || raw.role === "assistant" ? raw.role : "terminal_block");
  const malformed = typeof raw.text !== "string" || typeof kind !== "string" || !KINDS.has(kind)
    || ["name", "model", "input_summary", "result_summary", "diff", "status_kind"].some((field) => raw[field] != null && typeof raw[field] !== "string")
    || (raw.cost_usd != null && (typeof raw.cost_usd !== "number" || !Number.isFinite(raw.cost_usd)))
    || (raw.partial != null && typeof raw.partial !== "boolean")
    || (raw.status != null && !["running", "ok", "error"].includes(String(raw.status)));
  if (malformed) return { key, fallback: true, item: { role: "tools", kind: "terminal_block", text, at: null } };
  return { key, fallback: kind === "terminal_block", paneExcerpt: raw.kind === "terminal_block" && (raw.partial === true || key.startsWith("pane:")), item: { ...(value as TranscriptItem), text, kind: kind as string } };
}

export function mergeTranscript(current: ConversationRow[], incoming: ConversationRow[], removed: string[] = [], prepend = false): ConversationRow[] {
  const deleted = new Set(removed);
  const updates = new Map(incoming.map((row) => [row.key, row]));
  const known = new Set(current.map((row) => row.key));
  const kept = current.filter((row) => !deleted.has(row.key)).map((row) => prepend ? row : updates.get(row.key) ?? row);
  const added = [...updates.values()].filter((row) => !known.has(row.key) && !deleted.has(row.key));
  return prepend ? [...added, ...kept] : [...kept, ...added];
}

/// One watch per mounted pane. Reconciliation keeps Solid's row proxies and DOM nodes alive
/// across partial/final upserts. The latest-page cursor never overwrites an older page cursor.
export function createTranscript(target: () => TranscriptTarget | null) {
  const [state, setState] = createStore<{ rows: ConversationRow[] }>({ rows: [] });
  const [nextBefore, setNextBefore] = createSignal<number | null>(null);
  const [remaining, setRemaining] = createSignal<number | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [revision, setRevision] = createSignal(0);
  const [pagedOnce, setPagedOnce] = createSignal(false);
  let epoch = 0;
  let paged = false;
  let historyTarget = "";
  let lifecycle: Promise<void> = Promise.resolve();
  // Mirrors state.rows' key -> index so a pure upsert (every incoming key already present, no
  // removals, no prepend - the common case for a streamed partial/final delta) can patch just
  // the changed rows in place instead of rebuilding and re-diffing the whole loaded history on
  // every token. Structural changes (paging, removal, a brand new row) still rebuild fully below
  // and then refresh this index; those happen far less often than a streamed content update.
  let indexByKey = new Map<string, number>();
  const rebuildIndex = () => { indexByKey = new Map(state.rows.map((row, i) => [row.key, i])); };

  const apply = (rows: ConversationRow[], removed: string[] = [], prepend = false) => {
    const structural = prepend || removed.length > 0 || rows.some((row) => !indexByKey.has(row.key));
    if (structural) {
      setState("rows", reconcile(mergeTranscript([...state.rows], rows, removed, prepend), { key: "key" }));
      rebuildIndex();
    } else {
      for (const row of rows) setState("rows", indexByKey.get(row.key)!, reconcile(row));
    }
    setRevision((n) => n + 1);
  };

  createEffect(() => {
    const params = target();
    const run = ++epoch;
    if (!params) return;
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    let initialized = false;
    const buffered: TranscriptUpdate[] = [];
    const identity = JSON.stringify(params);
    const retained = historyTarget === identity;
    if (!retained) {
      historyTarget = identity;
      paged = false;
      setState("rows", []);
      indexByKey = new Map();
      setNextBefore(null);
      setRemaining(null);
      setPagedOnce(false);
    }
    setLoading(true);
    setError(null);
    const update = (value: TranscriptUpdate) => {
      apply(value.items.map((item, index) => transcriptRow(item, `event:${run}:${index}`)), value.removed_ids);
      if (!paged) { setNextBefore(value.next_before); setRemaining(value.remaining_before ?? null); }
    };
    lifecycle = lifecycle.catch(() => undefined).then(async () => {
      if (disposed) return;
      try {
        unsubscribe = await subscribeDaemon((event) => {
          if (disposed || event.method !== "event.agent.transcript") return;
          const value = event.params as TranscriptUpdate;
          if (!value || value.lane_id !== params.lane_id || value.window !== params.window || !Array.isArray(value.items)) return;
          if (!initialized) buffered.push(value); else update(value);
        });
        if (disposed) { unsubscribe(); return; }
        const page = await daemonCall("agent.transcript_watch", { ...params, on: true });
        if (disposed || run !== epoch) return;
        if (!page) throw new Error("The transcript watch returned no page. Open terminal or retry.");
        const incoming = page.items.map((item, index) => transcriptRow(item, `page:latest:${index}`));
        const present = new Set(incoming.map((row) => row.key));
        const staleLive = state.rows.filter((row) => !present.has(row.key) && (row.item.partial || ["status", "dialog", "terminal_block"].includes(row.item.kind ?? ""))).map((row) => row.key);
        apply(incoming, staleLive);
        if (!paged) { setNextBefore(page.next_before); setRemaining(page.remaining_before ?? null); }
        initialized = true;
        buffered.forEach(update);
        setRevision((n) => n + 1);
      } catch (cause) {
        if (!disposed) setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        if (!disposed) setLoading(false);
      }
    });
    onCleanup(() => {
      disposed = true;
      unsubscribe?.();
      // Queue stop after a pending start, before the next activation of this same pane.
      lifecycle = lifecycle.catch(() => undefined).then(async () => {
        unsubscribe?.();
        await daemonCall("agent.transcript_watch", { lane_id: params.lane_id, window: params.window, on: false }).catch(() => undefined);
      });
    });
  });

  async function loadOlder() {
    const params = target();
    const before = nextBefore();
    if (!params || before === null || loading()) return;
    const run = epoch;
    setLoading(true);
    setError(null);
    try {
      const page = await daemonCall("agent.transcript_page", { ...params, before });
      if (run !== epoch) return;
      paged = true;
      setPagedOnce(true);
      apply(page.items.map((item, index) => transcriptRow(item, `page:${before}:${index}`)), [], true);
      setNextBefore(page.next_before);
      setRemaining(page.remaining_before ?? null);
      return page.items.length;
    } catch (cause) {
      if (run === epoch) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (run === epoch) setLoading(false);
    }
  }
  return { rows: () => state.rows, nextBefore, remaining, loading, error, revision, pagedOnce, loadOlder };
}
