import { createEffect, createSignal, onCleanup } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import type { TranscriptItem } from "../bindings";
import { daemonCall, subscribeDaemon, type ActivitySnapshot, type TranscriptTarget, type TranscriptUpdate } from "../ipc/rpc";
import { getCachedTranscriptPage, setCachedTranscriptPage } from "./transcriptCache";
import { markChatLatency } from "../ipc/chatLatency";

export interface ConversationRow { key: string; item: TranscriptItem; fallback: boolean; paneExcerpt?: boolean }
const KINDS = new Set(["user", "assistant", "tool_call", "dialog", "status", "terminal_block", "mail"]);

export function transcriptRow(value: unknown, fallbackKey: string): ConversationRow {
  const raw = value && typeof value === "object" ? value as Record<string, unknown> : {};
  const key = typeof raw.id === "string" && raw.id ? raw.id : fallbackKey;
  const text = typeof raw.text === "string" ? raw.text : typeof value === "string" ? value : JSON.stringify(value) ?? String(value);
  const kind = raw.kind ?? (raw.role === "user" || raw.role === "assistant" ? raw.role : "terminal_block");
  const malformed = typeof raw.text !== "string" || typeof kind !== "string" || !KINDS.has(kind)
    || ["name", "model", "input_summary", "result_summary", "diff", "status_kind"].some((field) => raw[field] != null && typeof raw[field] !== "string")
    || (raw.mail != null && (typeof raw.mail !== "object" || typeof (raw.mail as Record<string, unknown>).id !== "string" || typeof (raw.mail as Record<string, unknown>).sender !== "string"))
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

// The daemon's authoritative window order: place every id in `order`, in that sequence, as a
// suffix after whatever loaded (older-page) rows are absent from it. Never a timestamp sort -
// ordered upserts alone can't seat a newly-ingested user row before an assistant partial that
// answered it, since the partial kept an earlier arrival slot; `order` is what can.
// `everLive` distinguishes a row order has never mentioned (genuinely older, paged-in history -
// belongs leading) from one that dropped out of this one order snapshot after previously being in
// it (a daemon-side hiccup mid-turn, e.g. a pending input handed off between representations) -
// the latter keeps its seated position instead of jumping back above already-settled history.
export function orderRows(rows: ConversationRow[], order: string[], everLive?: ReadonlySet<string>): ConversationRow[] {
  const byId = new Map(rows.map((row) => [row.key, row]));
  const inOrder = new Set(order);
  const isStale = (row: ConversationRow) => !inOrder.has(row.key) && !!everLive?.has(row.key);
  const seated = rows.filter((row) => !inOrder.has(row.key) && !isStale(row));
  for (const id of order) { const row = byId.get(id); if (row) seated.push(row); }
  // Seated position means the index it already occupies, so put each stale row back directly
  // below the nearest row above it that survived into `seated` (the head, when none did).
  // Appending them instead would hand a row the daemon momentarily stopped naming the newest
  // slot in the window - the exact jump this branch exists to prevent.
  for (const [index, row] of rows.entries()) {
    if (!isStale(row)) continue;
    let anchor = -1;
    for (let above = index - 1; above >= 0 && anchor < 0; above--) anchor = seated.findIndex((seat) => seat.key === rows[above].key);
    seated.splice(anchor + 1, 0, row);
  }
  return seated;
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
  const [activity, setActivity] = createSignal<ActivitySnapshot | null>(null);
  const [inputStates, setInputStates] = createSignal<Record<string, "sent" | "queued" | "consumed" | "delivered">>({});
  let epoch = 0;
  let paged = false;
  let historyTarget = "";
  let lifecycle: Promise<void> = Promise.resolve();
  // Mirrors state.rows' key -> index so a pure upsert (every incoming key already present, no
  // removals, no prepend, no order change - the common case for a streamed partial/final delta)
  // can patch just the changed rows in place instead of rebuilding and re-diffing the whole
  // loaded history on every token. Structural changes (paging, removal, a brand new row, or the
  // daemon's order actually moving something) still rebuild fully below and refresh this index;
  // those happen far less often than a streamed content update.
  let indexByKey = new Map<string, number>();
  const rebuildIndex = () => { indexByKey = new Map(state.rows.map((row, i) => [row.key, i])); };
  let lastOrderKey: string | undefined;
  const orderKeyOf = (order: string[]) => order.join(" ");
  // Every id this pane's live watch has ever placed in an `order` array - see orderRows' `everLive`
  // param. Reset alongside lastOrderKey whenever the pane identity changes.
  let seenInOrder = new Set<string>();

  const apply = (rows: ConversationRow[], removed: string[] = [], prepend = false, order?: string[]): boolean => {
    const structural = prepend || removed.length > 0 || rows.some((row) => !indexByKey.has(row.key));
    // A metadata-only push (activity or input_states ticking with no row content changed) calls
    // apply([]) with an unchanged order; skip the revision bump so the follow-scroll effect below
    // does not run on updates that changed nothing visible - it depends on revision alone.
    let changed = rows.length > 0 || removed.length > 0;
    if (structural) {
      let merged = mergeTranscript([...state.rows], rows, removed, prepend);
      if (order && !prepend) {
        for (const id of order) seenInOrder.add(id);
        merged = orderRows(merged, order, seenInOrder);
        lastOrderKey = orderKeyOf(order);
      }
      setState("rows", reconcile(merged, { key: "key" }));
      rebuildIndex();
    } else {
      for (const row of rows) setState("rows", indexByKey.get(row.key)!, reconcile(row));
      if (order) {
        const key = orderKeyOf(order);
        if (key !== lastOrderKey) {
          for (const id of order) seenInOrder.add(id);
          setState("rows", reconcile(orderRows([...state.rows], order, seenInOrder), { key: "key" }));
          rebuildIndex();
          changed = true;
        }
        lastOrderKey = key;
      }
    }
    if (changed) setRevision((n) => n + 1);
    return changed;
  };
  // Snapshots the loaded page under the identity currently active, so a future mount of this
  // exact (lane, window, session, kind) can paint from it immediately instead of blanking out
  // while its own watch re-attaches. Read by `getCachedTranscriptPage` on the next fresh mount.
  const persistPage = () => {
    if (!historyTarget) return;
    // Plain copies, not the store's own reactive proxies: this snapshot outlives the store that
    // produced it (read back by a future, unrelated `createTranscript` instance), so it must not
    // carry a reference into this one's reactivity graph.
    const rows = state.rows.map((row) => ({ key: row.key, fallback: row.fallback, paneExcerpt: row.paneExcerpt, item: { ...row.item } }));
    setCachedTranscriptPage(historyTarget, { rows, nextBefore: nextBefore(), remaining: remaining() });
  };
  // The daemon could not resolve this window's transcript identity (e.g. a session id that no
  // longer matches the window's current occupant). Rather than a permanent error banner, fall
  // back to the same live-pane-excerpt device already used for kinds with no transcript source:
  // one collapsed terminal_block row built from a fresh capture, replacing whatever rows were
  // showing (including anything seeded from a stale cache entry) so the pane never shows another
  // agent's history under this window's name. Returns null when the capture itself also fails,
  // leaving the caller to report the original error.
  async function captureFallbackRow(params: TranscriptTarget): Promise<ConversationRow | null> {
    try {
      const capture = await daemonCall("agent.capture", { lane_id: params.lane_id, window: params.window });
      if (!capture?.content) return null;
      const key = `pane:${params.lane_id}:${params.window ?? ""}`;
      return transcriptRow({ id: key, kind: "terminal_block", role: "tools", text: capture.content, at: null }, key);
    } catch {
      return null;
    }
  }

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
      // Chat-mode first-open latency breakdown (round 9): a genuinely new pane identity, not a
      // reconnect retry of one already open.
      markChatLatency("pane_mount", params);
      historyTarget = identity;
      paged = false;
      // A pane opened for the first time this mount still might not be the first time this exact
      // identity was ever watched this session - paint from what it last showed while the watch
      // re-attaches, instead of the blank "Opening conversation…" wait.
      const cachedPage = getCachedTranscriptPage(identity);
      setState("rows", cachedPage?.rows ?? []);
      indexByKey = new Map((cachedPage?.rows ?? []).map((row, i) => [row.key, i]));
      lastOrderKey = undefined;
      seenInOrder = new Set();
      setNextBefore(cachedPage?.nextBefore ?? null);
      setRemaining(cachedPage?.remaining ?? null);
      setPagedOnce(false);
      setActivity(null);
      setInputStates({});
    }
    setLoading(true);
    setError(null);
    const update = (value: TranscriptUpdate) => {
      const changed = apply(value.items.map((item, index) => transcriptRow(item, `event:${run}:${index}`)), value.removed_ids, false, value.order);
      if (!paged) { setNextBefore(value.next_before); setRemaining(value.older_message_count ?? null); }
      if (value.activity !== undefined) setActivity(value.activity);
      if (value.input_states !== undefined) setInputStates(value.input_states);
      if (changed) persistPage();
    };
    lifecycle = lifecycle.catch(() => undefined).then(async () => {
      if (disposed) return;
      try {
        // Chat-mode first-open latency breakdown (round 9): timestamps either side of the two
        // calls this pane's first content is gated behind, plus the moment each resolves.
        const subscribeStart = performance.now();
        unsubscribe = await subscribeDaemon((event) => {
          if (disposed || event.method !== "event.agent.transcript") return;
          const value = event.params as TranscriptUpdate;
          if (!value || value.lane_id !== params.lane_id || value.window !== params.window || !Array.isArray(value.items)) return;
          if (!initialized) buffered.push(value); else update(value);
        });
        markChatLatency("subscribe_daemon_resolved", params, performance.now() - subscribeStart);
        if (disposed) { unsubscribe(); return; }
        try {
          markChatLatency("transcript_watch_issued", params);
          const watchStart = performance.now();
          const page = await daemonCall("agent.transcript_watch", { ...params, on: true });
          markChatLatency("transcript_watch_resolved", params, performance.now() - watchStart);
          if (disposed || run !== epoch) return;
          if (!page) throw new Error("The transcript watch returned no page. Open terminal or retry.");
          const incoming = page.items.map((item, index) => transcriptRow(item, `page:latest:${index}`));
          const present = new Set(incoming.map((row) => row.key));
          const staleLive = state.rows.filter((row) => !present.has(row.key) && (row.item.partial || ["status", "dialog", "terminal_block"].includes(row.item.kind ?? ""))).map((row) => row.key);
          apply(incoming, staleLive, false, page.order);
          if (!paged) { setNextBefore(page.next_before); setRemaining(page.older_message_count ?? null); }
          if (page.activity !== undefined) setActivity(page.activity);
          if (page.input_states !== undefined) setInputStates(page.input_states);
          initialized = true;
          buffered.forEach(update);
          persistPage();
          setRevision((n) => n + 1);
        } catch (cause) {
          if (disposed || run !== epoch) return;
          // The daemon could not resolve this window's identity (e.g. a stale session id). Show
          // this window's own live pane content instead of a hard error or another agent's
          // cached history, and never surface the error banner when that fallback succeeds.
          const fallback = await captureFallbackRow(params);
          if (disposed || run !== epoch) return;
          if (!fallback) throw cause;
          apply([fallback], state.rows.map((row) => row.key), false);
          setNextBefore(null);
          setRemaining(null);
          initialized = true;
          buffered.forEach(update);
          persistPage();
          setRevision((n) => n + 1);
        }
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
      const rows = page.items.map((item, index) => transcriptRow(item, `page:${before}:${index}`));
      const ordered = page.order ? orderRows(rows, page.order) : rows;
      apply(ordered, page.removed_ids ?? [], true);
      setNextBefore(page.next_before);
      setRemaining(page.older_message_count ?? null);
      persistPage();
      return page.items.length;
    } catch (cause) {
      if (run === epoch) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (run === epoch) setLoading(false);
    }
  }
  return { rows: () => state.rows, nextBefore, remaining, loading, error, revision, pagedOnce, activity, inputStates, loadOlder };
}
