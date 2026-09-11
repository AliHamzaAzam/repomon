import { createRoot, createSignal } from "solid-js";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DaemonEvent } from "../ipc/rpc";
import { daemonCall, subscribeDaemon } from "../ipc/rpc";
import { markChatLatency } from "../ipc/chatLatency";
import { getCachedTranscriptPage, resetTranscriptCacheForTests } from "./transcriptCache";
import { createTranscript, orderRows, transcriptRow, type ConversationRow } from "./transcript";

vi.mock("../ipc/rpc", () => ({ daemonCall: vi.fn(), subscribeDaemon: vi.fn() }));
vi.mock("../ipc/chatLatency", () => ({ markChatLatency: vi.fn() }));

beforeEach(() => { resetTranscriptCacheForTests(); vi.mocked(markChatLatency).mockClear(); });

const row = (id: string): ConversationRow => transcriptRow({ id, kind: "assistant", role: "assistant", text: id, at: null }, id);

describe("orderRows", () => {
  it("reseats a row the daemon's order puts earlier than where it happened to arrive", () => {
    // Arrival order put the assistant reply first (its partial showed up before the user row that
    // asked for it landed), but the daemon's order says the user row answers it, so it belongs
    // first - upsert order alone can never fix this, since both rows already exist by the time
    // order disagrees with arrival.
    const current = [row("assistant-1"), row("user-1")];
    const ordered = orderRows(current, ["user-1", "assistant-1"]);
    expect(ordered.map((r) => r.key)).toEqual(["user-1", "assistant-1"]);
  });
  it("keeps already-loaded older rows absent from the window's order as a leading block", () => {
    const current = [row("older-1"), row("older-2"), row("live-1")];
    const ordered = orderRows(current, ["live-1"]);
    expect(ordered.map((r) => r.key)).toEqual(["older-1", "older-2", "live-1"]);
  });
  it("drops ids the order references but the store never received a row for", () => {
    const current = [row("a")];
    expect(orderRows(current, ["ghost", "a"]).map((r) => r.key)).toEqual(["a"]);
  });
});

describe("createTranscript, end to end", () => {
  it("uses the daemon's order to seat a delayed user row before the assistant partial it answers", async () => {
    let emit!: (event: DaemonEvent) => void;
    vi.mocked(subscribeDaemon).mockImplementation(async (callback) => { emit = callback; return vi.fn(); });
    vi.mocked(daemonCall).mockImplementation(async (method, ...args) => {
      if (method === "agent.transcript_watch") return (args[0] as { on: boolean }).on
        ? { items: [{ id: "a1", kind: "assistant", role: "assistant", text: "Answer", at: null }], next_before: null, order: ["a1"] }
        : null;
      return null;
    });
    const { transcript, dispose } = createRoot((dispose) => ({ transcript: createTranscript(() => ({ lane_id: 1, window: "lane-1" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(transcript.rows().map((r) => r.key)).toEqual(["a1"]);
    emit({
      jsonrpc: "2.0", method: "event.agent.transcript",
      params: { lane_id: 1, window: "lane-1", subscription_id: 1, items: [{ id: "u1", kind: "user", role: "user", text: "Do the thing", at: null }], removed_ids: [], next_before: null, order: ["u1", "a1"] },
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(transcript.rows().map((r) => r.key)).toEqual(["u1", "a1"]);
    dispose();
  });

  it("replaces the input_states snapshot each update and reflects a pending row's own state", async () => {
    let emit!: (event: DaemonEvent) => void;
    vi.mocked(subscribeDaemon).mockImplementation(async (callback) => { emit = callback; return vi.fn(); });
    vi.mocked(daemonCall).mockImplementation(async (method, ...args) => {
      if (method === "agent.transcript_watch") return (args[0] as { on: boolean }).on
        ? { items: [{ id: "u1", kind: "user", role: "user", text: "Do the thing", at: null, partial: true }], next_before: null, order: ["u1"], input_states: { u1: "queued" } }
        : null;
      return null;
    });
    const { transcript, dispose } = createRoot((dispose) => ({ transcript: createTranscript(() => ({ lane_id: 1, window: "lane-1" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(transcript.inputStates()).toEqual({ u1: "queued" });
    // Consumption: the daemon upserts the same id, clears partial, and drops it from input_states.
    emit({
      jsonrpc: "2.0", method: "event.agent.transcript",
      params: { lane_id: 1, window: "lane-1", subscription_id: 1, items: [{ id: "u1", kind: "user", role: "user", text: "Do the thing", at: null, partial: false }], removed_ids: [], next_before: null, order: ["u1"], input_states: {} },
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(transcript.inputStates()).toEqual({});
    expect(transcript.rows().find((r) => r.key === "u1")?.item.partial).toBe(false);
    dispose();
  });

  it("does not bump revision for a metadata-only push (round 6 item 1, candidate 4)", async () => {
    // ConversationPane's follow-scroll effect depends on revision alone; a push that only ticks
    // activity or input_states, with no row content and no order change, must not fire it -
    // that is exactly "runs the follow-scroll on every single update, including ones that
    // changed nothing visible."
    let emit!: (event: DaemonEvent) => void;
    vi.mocked(subscribeDaemon).mockImplementation(async (callback) => { emit = callback; return vi.fn(); });
    vi.mocked(daemonCall).mockImplementation(async (method, ...args) => {
      if (method === "agent.transcript_watch") return (args[0] as { on: boolean }).on
        ? { items: [{ id: "a1", kind: "assistant", role: "assistant", text: "Answer", at: null, partial: true }], next_before: null, order: ["a1"], activity: { verb: "Whisking", elapsed_seconds: 1, token_count: null, thought_seconds: null, model: null, effort: null } }
        : null;
      return null;
    });
    const { transcript, dispose } = createRoot((dispose) => ({ transcript: createTranscript(() => ({ lane_id: 1, window: "lane-1" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    await new Promise((resolve) => setTimeout(resolve, 0));
    const before = transcript.revision();
    emit({
      jsonrpc: "2.0", method: "event.agent.transcript",
      params: { lane_id: 1, window: "lane-1", subscription_id: 1, items: [], removed_ids: [], next_before: null, order: ["a1"], activity: { verb: "Whisking", elapsed_seconds: 2, token_count: null, thought_seconds: null, model: null, effort: null } },
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(transcript.activity()?.elapsed_seconds).toBe(2);
    expect(transcript.revision()).toBe(before);
    dispose();
  });

  it("caches the loaded page under its watch identity and paints a later mount of the exact same identity from it", async () => {
    vi.mocked(subscribeDaemon).mockImplementation(async () => vi.fn());
    vi.mocked(daemonCall).mockImplementation(async (method, ...args) => (method === "agent.transcript_watch" && (args[0] as { on: boolean }).on
      ? { items: [{ id: "a1", kind: "assistant", role: "assistant", text: "Answer", at: null }], next_before: 40, older_message_count: 12 }
      : null));
    const first = createRoot((dispose) => ({ transcript: createTranscript(() => ({ lane_id: 1, window: "lane-1" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(first.transcript.rows().map((r) => r.key)).toEqual(["a1"]);
    const cached = getCachedTranscriptPage(JSON.stringify({ lane_id: 1, window: "lane-1" }));
    expect(cached?.rows.map((r) => r.key)).toEqual(["a1"]);
    expect(cached?.nextBefore).toBe(40);
    first.dispose();
    // A never-resolving watch on the fresh instance proves the paint came from the cache, not
    // from this mount's own (still pending) network round trip.
    vi.mocked(daemonCall).mockImplementation(() => new Promise(() => {}));
    const second = createRoot((dispose) => ({ transcript: createTranscript(() => ({ lane_id: 1, window: "lane-1" })), dispose }));
    expect(second.transcript.rows().map((r) => r.key)).toEqual(["a1"]);
    expect(second.transcript.nextBefore()).toBe(40);
    second.dispose();
  });

  it("never seeds a different identity's cached history, and a window whose identity the daemon cannot resolve falls back to its own live pane excerpt with no history and no hard error", async () => {
    vi.mocked(subscribeDaemon).mockImplementation(async () => vi.fn());
    vi.mocked(daemonCall).mockImplementation(async (method, ...args) => {
      if (method === "agent.transcript_watch") {
        const params = args[0] as { on: boolean; window: string };
        if (!params.on) return null;
        if (params.window === "lane-2/1") return { items: [{ id: "other", kind: "assistant", role: "assistant", text: "Other agent's reply", at: null }], next_before: null };
        throw new Error("session_id does not belong to window lane-2/2; omit session_id to address this window");
      }
      if (method === "agent.capture") return { content: "$ codex\nWaiting for input.\n› " };
      return null;
    });
    const other = createRoot((dispose) => ({ transcript: createTranscript(() => ({ lane_id: 2, window: "lane-2/1" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(other.transcript.rows().map((r) => r.key)).toEqual(["other"]);
    other.dispose();
    const unresolved = createRoot((dispose) => ({ transcript: createTranscript(() => ({ lane_id: 2, window: "lane-2/2", session_id: "stale-session" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(unresolved.transcript.error()).toBeNull();
    expect(unresolved.transcript.rows().map((r) => r.item.text)).toEqual(["$ codex\nWaiting for input.\n› "]);
    expect(unresolved.transcript.rows().every((r) => r.paneExcerpt)).toBe(true);
    unresolved.dispose();
  });
});

describe("chat-mode first-open latency breakdown (round 9)", () => {
  it("marks pane_mount, subscribe_daemon_resolved, and both transcript_watch marks, in order, for a genuinely new pane", async () => {
    vi.mocked(subscribeDaemon).mockImplementation(async () => vi.fn());
    vi.mocked(daemonCall).mockImplementation(async (method, ...args) => {
      if (method === "agent.transcript_watch") return (args[0] as { on: boolean }).on
        ? { items: [{ id: "a1", kind: "assistant", role: "assistant", text: "Answer", at: null }], next_before: null }
        : null;
      return null;
    });
    const { dispose } = createRoot((dispose) => ({ transcript: createTranscript(() => ({ lane_id: 9, window: "lane-9" })), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    await new Promise((resolve) => setTimeout(resolve, 0));
    const labels = vi.mocked(markChatLatency).mock.calls.map(([label]) => label);
    expect(labels).toEqual(["pane_mount", "subscribe_daemon_resolved", "transcript_watch_issued", "transcript_watch_resolved"]);
    for (const call of vi.mocked(markChatLatency).mock.calls) {
      expect(call[1]).toEqual({ lane_id: 9, window: "lane-9" });
    }
    const resolvedCall = vi.mocked(markChatLatency).mock.calls.find(([label]) => label === "transcript_watch_resolved")!;
    expect(typeof resolvedCall[2]).toBe("number");
    dispose();
  });

  it("does not mark pane_mount again when the effect re-runs for the same retained identity", async () => {
    vi.mocked(subscribeDaemon).mockImplementation(async () => vi.fn());
    vi.mocked(daemonCall).mockImplementation(async (method, ...args) => (
      method === "agent.transcript_watch" && (args[0] as { on: boolean }).on ? { items: [], next_before: null } : null
    ));
    const [trigger, setTrigger] = createSignal(0);
    // Reads a signal but always returns the same logical identity, so the effect re-runs
    // (something a reconnect can genuinely do) without this being a new pane.
    const target = () => { trigger(); return { lane_id: 9, window: "lane-9" }; };
    const { dispose } = createRoot((dispose) => ({ transcript: createTranscript(target), dispose }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(vi.mocked(markChatLatency).mock.calls.map(([label]) => label)).toContain("pane_mount");
    vi.mocked(markChatLatency).mockClear();
    setTrigger(1);
    await new Promise((resolve) => setTimeout(resolve, 0));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(vi.mocked(markChatLatency).mock.calls.map(([label]) => label)).not.toContain("pane_mount");
    dispose();
  });
});
