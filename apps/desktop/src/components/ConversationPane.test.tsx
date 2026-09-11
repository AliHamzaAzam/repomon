import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DaemonEvent } from "../ipc/rpc";
import type { TranscriptItem } from "../bindings";
import { daemonCall, subscribeDaemon } from "../ipc/rpc";
import { resetTranscriptCacheForTests } from "../stores/transcriptCache";
import ConversationPane, { activityLabel, dialogSummary, groupTurnWork } from "./ConversationPane";
vi.mock("../ipc/rpc", () => ({ daemonCall:vi.fn(), subscribeDaemon:vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl:vi.fn() }));
let emit: (event: DaemonEvent) => void;
let items: TranscriptItem[];
const target = { lane_id:7, window:"lane-7/1", session_id:"s7", kind:"codex" };
const row = (id: string, text: string, partial = false): TranscriptItem => ({ id, kind:"assistant", role:"assistant", text, at:null, partial });
function update(incoming: TranscriptItem[], removed_ids: string[] = [], overrides = {}) {
  emit({ jsonrpc:"2.0", method:"event.agent.transcript", params:{ lane_id:7, window:"lane-7/1", subscription_id:91, items:incoming, removed_ids, next_before:120, ...overrides } });
}
beforeEach(() => {
  resetTranscriptCacheForTests();
  items = [row("live:1", "Still writing", true)];
  vi.mocked(subscribeDaemon).mockImplementation(async (callback) => { emit = callback; return vi.fn(); });
  vi.mocked(daemonCall).mockImplementation(async (method, ...args) => {
    if (method === "agent.transcript_watch") return (args[0] as {on:boolean}).on ? { items, next_before:120 } : null;
    if (method === "agent.transcript_page") return { items:[row("old:1", "Earlier history")], next_before:40 };
    if (method === "agent.capture") return { content:"line one\nline two\nline three" };
    if (method === "agent.prompt") return { dialog:null };
    return null;
  });
});
afterEach(() => { cleanup(); vi.clearAllMocks(); });
const mount = () => render(() => <ConversationPane target={target} kind="codex" visible onTerminal={vi.fn()} />);
describe("ConversationPane", () => {
  it("renders a partial immediately and replaces it in the same DOM row, retaining loaded history", async () => {
    const result = mount();
    expect(await screen.findByText("Still writing")).toBeInTheDocument();
    const node = result.container.querySelector('[data-transcript-id="live:1"]');
    expect(node).toHaveAttribute("data-partial", "true");
    fireEvent.click(screen.getByRole("button", { name:"Load earlier messages" }));
    await screen.findByText("Earlier history");
    update([row("live:1", "Final answer")]);
    await screen.findByText("Final answer");
    expect(result.container.querySelector('[data-transcript-id="live:1"]')).toBe(node);
    expect(node).not.toHaveAttribute("data-partial");
    expect(screen.getByText("Earlier history")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name:"Load earlier messages" }));
    await waitFor(() => expect(daemonCall).toHaveBeenCalledWith("agent.transcript_page", { ...target, before:40 }));
    update([], ["live:1"]);
    expect(screen.queryByText("Final answer")).not.toBeInTheDocument();
  });
  it("keeps malformed and unknown entries readable as monospace blocks", async () => {
    items = [row("ok", "Intact message"), { id:"bad", role:"tools", kind:"unknown-future-kind", text:'{"broken":', at:null }];
    const result = mount();
    await screen.findByText("Intact message");
    fireEvent.click(screen.getByRole("button", { name:/Unformatted entry/ }));
    expect(result.container.querySelector('pre.conversation-raw')).toHaveTextContent('{"broken":');
    expect(screen.getAllByRole("article")).toHaveLength(2);
    expect(screen.getByText("entry")).toBeInTheDocument();
  });
  it("shows running and failed tools with keyboard-operable disclosure", async () => {
    items = [{ id:"tool", role:"tools", kind:"tool_call", name:"exec_command", input_summary:"bun run build", status:"running", text:"build starting", at:null }];
    mount();
    const button = await screen.findByRole("button", { name:/Using 1 tool/ });
    expect(button).toHaveAttribute("aria-expanded", "false");
    update([{ ...items[0], status:"error", text:"Build exited with code 1" }]);
    fireEvent.click(screen.getByRole("button", { name:/Used 1 tool.*1 failed/ }));
    expect(screen.getByText("Build exited with code 1")).toBeInTheDocument();
  });
  it("answers the current dialog using zero-based choice and the exact stale-prompt guard", async () => {
    const dialog = { title:"Bash command", question:"Do you want to proceed?", body:["bun run build"], options:[{number:1,text:"Yes"},{number:2,text:"No"}], selected:0 };
    const original = vi.mocked(daemonCall).getMockImplementation()!;
    vi.mocked(daemonCall).mockImplementation(async (method, ...args) => method === "agent.prompt" ? {dialog} : original(method, ...args));
    mount();
    await screen.findByText(/Do you want to proceed/);
    expect(screen.getByRole("textbox", { name:"Reply to codex" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name:"No" }));
    await waitFor(() => expect(daemonCall).toHaveBeenCalledWith("agent.answer", { lane_id:7, window:"lane-7/1", choice:1, expect_summary:"Bash command \u2014 Do you want to proceed?" }));
    await waitFor(() => expect(screen.getByRole("textbox", { name:"Reply to codex" })).not.toBeDisabled());
    expect(dialogSummary({ ...dialog, question:"x".repeat(140) })).toHaveLength(120);
  });
  it("sends a reply on Enter, leaving Shift+Enter for a newline", async () => {
    mount();
    await screen.findByText("Still writing");
    const input = screen.getByRole("textbox", { name:"Reply to codex" });
    fireEvent.input(input, {target:{value:"Please continue"}});
    fireEvent.keyDown(input, {key:"Enter",shiftKey:true});
    expect(daemonCall).not.toHaveBeenCalledWith("agent.send_input", expect.anything());
    fireEvent.keyDown(input, {key:"Enter"});
    await waitFor(() => expect(daemonCall).toHaveBeenCalledWith("agent.send_input", {lane_id:7,window:"lane-7/1",text:"Please continue",enter:true}));
    await waitFor(() => expect(input).toHaveValue(""));
  });
  it("isolates windows and stops the watch when hidden", async () => {
    const [visible, setVisible] = createSignal(true);
    render(() => <ConversationPane target={target} kind="codex" visible={visible()} onTerminal={vi.fn()} />);
    await screen.findByText("Still writing");
    update([row("other", "Other window")], [], {window:"lane-7/2"});
    expect(screen.queryByText("Other window")).not.toBeInTheDocument();
    setVisible(false);
    await waitFor(() => expect(daemonCall).toHaveBeenCalledWith("agent.transcript_watch", {lane_id:7,window:"lane-7/1",on:false}));
  });
});

it("buffers live output arriving before the initial watch response", async () => {
  let finish!: (page: { items: TranscriptItem[]; next_before: null }) => void;
  const page = new Promise<{ items: TranscriptItem[]; next_before: null }>((resolve) => { finish = resolve; });
  const original = vi.mocked(daemonCall).getMockImplementation()!;
  vi.mocked(daemonCall).mockImplementation(async (method, ...args) => method === "agent.transcript_watch" && (args[0] as {on:boolean}).on ? page : original(method, ...args));
  mount();
  await waitFor(() => expect(daemonCall).toHaveBeenCalledWith("agent.transcript_watch", { ...target, on:true }));
  update([row("live:1", "Newer partial", true)]);
  finish({ items:[row("live:1", "Initial partial", true)], next_before:null });
  expect(await screen.findByText("Newer partial")).toBeInTheDocument();
  expect(screen.queryByText("Initial partial")).not.toBeInTheDocument();
});

it("retains older pages across a Terminal / Chat round trip", async () => {
  const [visible, setVisible] = createSignal(true);
  render(() => <ConversationPane target={target} kind="codex" visible={visible()} onTerminal={vi.fn()} />);
  await screen.findByText("Still writing");
  fireEvent.click(screen.getByRole("button", { name:"Load earlier messages" }));
  await screen.findByText("Earlier history");
  setVisible(false);
  await waitFor(() => expect(daemonCall).toHaveBeenCalledWith("agent.transcript_watch", {lane_id:7,window:"lane-7/1",on:false}));
  setVisible(true);
  await waitFor(() => expect(vi.mocked(daemonCall).mock.calls.filter(([method, params]) => method === "agent.transcript_watch" && (params as {on:boolean}).on)).toHaveLength(2));
  expect(screen.getByText("Earlier history")).toBeInTheDocument();
});

it("groups work by turn and renders the daemon cost text only once inside expanded work", async () => {
  items = [
    {id:"u",role:"user",kind:"user",text:"Check it",at:null},
    {id:"start",role:"tools",kind:"status",status_kind:"turn_started",text:"Turn started",at:null},
    {id:"tool",role:"tools",kind:"tool_call",status:"ok",name:"Bash",text:"Done",at:null},
    {id:"cost",role:"tools",kind:"status",status_kind:"turn_cost",text:"Turn cost $0.0511",cost_usd:0.0511,at:null},
    {id:"end",role:"tools",kind:"status",status_kind:"turn_finished",text:"Turn finished",at:null},
  ];
  const [detail, setDetail] = createSignal<"normal" | "verbose">("normal");
  render(() => <ConversationPane target={target} kind="codex" visible detail={detail()} onTerminal={vi.fn()} />);
  await screen.findByRole("button", {name:"Used 1 tool"});
  expect(screen.queryByText(/Turn cost/)).not.toBeInTheDocument();
  setDetail("verbose");
  expect(await screen.findByText("Turn cost $0.0511")).toBeInTheDocument();
  expect(document.body.textContent?.match(/\$0\.0511/g)).toHaveLength(1);
  const rows = [...items, {...items[1],id:"next-start"}, {...items[2],id:"next-tool"}].map((item) => ({key:item.id!,item,fallback:false}));
  expect([...groupTurnWork(rows).groups.keys()]).toEqual(["start", "next-start"]);
});

it("collapses a live pane excerpt beside real history and replaces it in place with the final answer", async () => {
  const pane = "Merge to main, yes or no?\nCogitated for 10m 39s\n› yes merge it\nauto mode on (shift+tab to cycle)";
  items = [row("history", "A real conversation"), {id:"live:2",kind:"terminal_block",role:"tools",text:pane,at:null,partial:true}];
  const result = mount();
  const button = await screen.findByRole("button", {name:/Terminal excerpt/});
  expect(button).toHaveAttribute("aria-expanded", "false");
  expect(screen.queryByText(/Cogitated/)).not.toBeInTheDocument();
  expect(screen.queryByText(/Writing/)).not.toBeInTheDocument();
  const node = result.container.querySelector('[data-transcript-id="live:2"]');
  fireEvent.click(button);
  expect(screen.getByText(/Cogitated/)).toBeInTheDocument();
  update([row("live:2", "The merge is complete.")]);
  expect(screen.getByText("The merge is complete.")).toBeInTheDocument();
  expect(result.container.querySelector('[data-transcript-id="live:2"]')).toBe(node);
  expect(screen.queryByRole("button", {name:/Terminal excerpt/})).not.toBeInTheDocument();
  expect(screen.getByText("A real conversation")).toBeInTheDocument();
});
it("renders delivered paths as image references and typed file chips for user and assistant", async () => {
  const text = 'Please check this.\n\nAttached file: "/stable/image.png"\n\nAttached file: "/stable/notes.md"';
  items = [{id:"u",role:"user",kind:"user",text,at:null},row("a",text)];
  const result = mount();
  await screen.findAllByText("[Image #1]");
  expect(result.container.textContent).not.toContain("/stable/");
  expect(screen.getAllByText("notes.md")).toHaveLength(2);
  expect(screen.getAllByText("MD")).toHaveLength(2);
  expect(result.container.querySelector('[title="/stable/image.png"]')).toBeInTheDocument();
});

describe("session activity, pinned above the composer", () => {
  it("formats elapsed seconds and compact tokens, and drops parts that are absent", () => {
    expect(activityLabel({ verb:"Whisking", elapsed_seconds:33, token_count:1100, thought_seconds:null, model:null, effort:null })).toBe("Whisking… (33s, 1.1k tokens)");
    expect(activityLabel({ verb:"Thinking", elapsed_seconds:75, token_count:null, thought_seconds:null, model:null, effort:null })).toBe("Thinking… (1m 15s)");
    expect(activityLabel({ verb:"Idle", elapsed_seconds:null, token_count:null, thought_seconds:null, model:null, effort:null })).toBe("Idle…");
    expect(activityLabel({ verb:null, elapsed_seconds:null, token_count:null, thought_seconds:null, model:"gpt-6-astra", effort:"high" })).toBeNull();
    expect(activityLabel({ verb:"Editing", elapsed_seconds:12, token_count:null, thought_seconds:null, model:"gpt-6-astra", effort:"high" })).toBe("Editing… (12s, gpt-6-astra, high)");
  });
  it("renders left of Open live terminal from the watch response, updates from an event, and leaves no hole when idle", async () => {
    const original = vi.mocked(daemonCall).getMockImplementation()!;
    vi.mocked(daemonCall).mockImplementation(async (method, ...args) => {
      if (method === "agent.transcript_watch") return (args[0] as {on:boolean}).on ? { items, next_before:120, activity:{verb:"Whisking", elapsed_seconds:33, token_count:1100, thought_seconds:null, model:null, effort:null} } : null;
      return original(method, ...args);
    });
    mount();
    await screen.findByText("Whisking… (33s, 1.1k tokens)");
    expect(screen.getByRole("button", {name:"Expand terminal"})).toBeInTheDocument();
    update([], [], {activity:null});
    await waitFor(() => expect(screen.queryByText(/Whisking/)).not.toBeInTheDocument());
  });
});

describe("streaming flash (round 6 item 1), instrumented", () => {
  function observe(container: HTMLElement) {
    const mutations: MutationRecord[] = [];
    const observer = new MutationObserver((records) => mutations.push(...records));
    observer.observe(container.querySelector(".conversation-ledger")!, { childList: true, subtree: true });
    return { mutations, stop: () => observer.disconnect(), flush: () => observer.takeRecords().forEach((r) => mutations.push(r)) };
  }
  it("does not add or remove any DOM node across a streamed run whose order stays byte-identical", async () => {
    items = [
      { id: "u1", kind: "user", role: "user", text: "Do it", at: null },
      { id: "live:1", kind: "assistant", role: "assistant", text: "Start", at: null, partial: true },
    ];
    const original = vi.mocked(daemonCall).getMockImplementation()!;
    vi.mocked(daemonCall).mockImplementation(async (method, ...args) => {
      if (method === "agent.transcript_watch") return (args[0] as { on: boolean }).on ? { items, next_before: null, order: ["u1", "live:1"] } : null;
      return original(method, ...args);
    });
    const result = mount();
    await screen.findByText("Start");
    const ledger = result.container.querySelector(".conversation-ledger")!;
    const rowNode = result.container.querySelector('[data-transcript-id="live:1"]')!;
    const probe = observe(result.container);
    for (let i = 0; i < 8; i++) {
      // next_before must be pinned explicitly: the shared update() helper otherwise defaults it
      // to a truthy value, which flips the pagination affordance on mid-stream - a real proof
      // artifact once, not a fixture default worth repeating.
      update([{ id: "live:1", kind: "assistant", role: "assistant", text: `Start plus token ${i}`, at: null, partial: true }], [], { order: ["u1", "live:1"], next_before: null });
    }
    await screen.findByText(/Start plus token 7/);
    probe.flush();
    // Row-internal mutations (markdown re-parsing new text into existing nodes) are expected;
    // what must never happen is the ledger's own direct children being added/removed/reordered.
    const topLevel = probe.mutations.filter((m) => m.target === ledger);
    expect(topLevel).toHaveLength(0);
    expect(result.container.querySelector('[data-transcript-id="live:1"]')).toBe(rowNode);
    probe.stop();
  });
  it("keeps a row's own DOM node across a same-membership order that resequences (robustness, not the observed cause)", async () => {
    // Confirmed separately (crates/repomon-core/src/agent/conversation.rs) that order and items
    // come from one Vec-built snapshot per poll and never reshuffle for stable membership - this
    // scenario does not occur in practice. Kept as a robustness check: even if it did, Solid's
    // keyed reconcile must not treat a repositioned id as a new row.
    items = [
      { id: "u1", kind: "user", role: "user", text: "Do it", at: null },
      { id: "t1", kind: "tool_call", role: "tools", name: "exec", status: "ok", text: "ran", at: null },
      { id: "live:1", kind: "assistant", role: "assistant", text: "Start", at: null, partial: true },
    ];
    const original = vi.mocked(daemonCall).getMockImplementation()!;
    vi.mocked(daemonCall).mockImplementation(async (method, ...args) => {
      if (method === "agent.transcript_watch") return (args[0] as { on: boolean }).on ? { items, next_before: null, order: ["u1", "t1", "live:1"] } : null;
      return original(method, ...args);
    });
    const result = mount();
    await screen.findByText("Start");
    const node = result.container.querySelector('[data-transcript-id="live:1"]');
    const rotations = [["t1", "u1", "live:1"], ["u1", "live:1", "t1"], ["u1", "t1", "live:1"]];
    for (const order of rotations) update([{ id: "live:1", kind: "assistant", role: "assistant", text: "Start updated", at: null, partial: true }], [], { order, next_before: null });
    await screen.findByText("Start updated");
    expect(result.container.querySelector('[data-transcript-id="live:1"]')).toBe(node);
  });
});

describe("pending queued/sent user turns", () => {
  it("reads a sent-but-unconsumed user row as waiting, not as Writing or failed, and resolves in place on consumption", async () => {
    items = [{ id:"u1", kind:"user", role:"user", text:"Do the thing", at:null, partial:true }];
    const original = vi.mocked(daemonCall).getMockImplementation()!;
    vi.mocked(daemonCall).mockImplementation(async (method, ...args) => {
      if (method === "agent.transcript_watch") return (args[0] as {on:boolean}).on ? { items, next_before:null, input_states:{u1:"queued"} } : null;
      return original(method, ...args);
    });
    const result = mount();
    // Matches the TUI: not yet read by the agent means not seated in the transcript as though
    // delivered - it lives in the pinned queue below the ledger, not as an ordinary ledger row.
    await screen.findByText("Queued");
    expect(screen.queryByText(/Writing/)).not.toBeInTheDocument();
    expect(result.container.querySelector(".conversation-pending-queue [data-transcript-id=\"u1\"]")).not.toBeNull();
    expect(result.container.querySelector(".conversation-ledger [data-transcript-id=\"u1\"]")).toBeNull();
    update([{ id:"u1", kind:"user", role:"user", text:"Do the thing", at:null, partial:false }], [], {input_states:{}});
    await waitFor(() => expect(screen.queryByText("Queued")).not.toBeInTheDocument());
    expect(result.container.querySelector(".conversation-pending-queue")).toBeNull();
    const seated = result.container.querySelector(".conversation-ledger [data-transcript-id=\"u1\"]");
    expect(seated).not.toBeNull();
    expect(seated?.textContent).not.toContain("Queued");
    expect(seated?.textContent).not.toContain("Sent");
  });
  it("treats consumed as an ordinary seated row with no label, even while still briefly partial", async () => {
    items = [{ id:"u1", kind:"user", role:"user", text:"Do the thing", at:null, partial:true }];
    const original = vi.mocked(daemonCall).getMockImplementation()!;
    vi.mocked(daemonCall).mockImplementation(async (method, ...args) => {
      if (method === "agent.transcript_watch") return (args[0] as {on:boolean}).on ? { items, next_before:null, input_states:{u1:"consumed"} } : null;
      return original(method, ...args);
    });
    const result = mount();
    await screen.findByText("Do the thing");
    expect(result.container.querySelector(".conversation-pending-queue")).toBeNull();
    const seated = result.container.querySelector('.conversation-ledger [data-transcript-id="u1"]');
    expect(seated).not.toBeNull();
    expect(seated?.textContent).not.toContain("Queued");
    expect(seated?.textContent).not.toContain("Sent");
    expect(screen.queryByText(/Writing/)).not.toBeInTheDocument();
  });
});

describe("Codex tool-rollup summaries", () => {
  it("shows the rollup summary without a redundant tool_summary label, inside the normal tool disclosure", async () => {
    items = [{ id:"t1", kind:"tool_call", role:"tools", name:"tool_summary", input_summary:"Ran 1 shell command", status:"ok", text:"Ran 1 shell command", at:null }];
    mount();
    fireEvent.click(await screen.findByRole("button", {name:/Used 1 tool/}));
    const toolButton = await screen.findByRole("button", {name:/Ran 1 shell command/});
    expect(toolButton.textContent).not.toContain("tool_summary");
  });
});

describe("per-kind fallback state", () => {
  it("reads Hermes' missing transcript as a deliberate, named explanation rather than a failure", async () => {
    items = [{id:"source:lane-10", kind:"status", role:"assistant", text:"Hermes Agent: no state.db session has been uniquely matched to this window. The live terminal excerpt remains available below.", status_kind:"source_unavailable", at:null}, {id:"pane:lane-10", kind:"terminal_block", role:"tools", text:"$ hermes\nWaiting for input.\n› ", at:null}];
    render(() => <ConversationPane target={target} kind="hermes" visible onTerminal={vi.fn()} />);
    const note = await screen.findByText(/Hermes Agent/);
    expect(note.textContent).toContain("no state.db session has been uniquely matched");
    fireEvent.click(await screen.findByRole("button", {name:/Terminal excerpt/}));
    expect(screen.getByLabelText("Terminal excerpt content")).toHaveTextContent("Waiting for input.");
    expect(note.textContent?.toLowerCase()).not.toContain("broken");
    expect(await screen.findByRole("button", {name:/Terminal excerpt/})).toBeInTheDocument();
  });
  it("no longer shows the no-transcript note for Antigravity, one of the daemon's four scanned kinds", async () => {
    items = [row("a1", "The scanner now feeds this lane's real transcript.")];
    render(() => <ConversationPane target={target} kind="antigravity" visible onTerminal={vi.fn()} />);
    await screen.findByText("The scanner now feeds this lane's real transcript.");
    expect(screen.queryByText(/sends its output straight to the terminal/)).not.toBeInTheDocument();
  });
});

describe("selecting transcript text copies it, Claude TUI style", () => {
  it("writes a selection inside the ledger to the clipboard on mouseup, but not one outside it", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    items = [row("a1", "Copy this reply text.")];
    const result = mount();
    const node = await screen.findByText("Copy this reply text.");
    const range = document.createRange();
    range.selectNodeContents(node);
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
    fireEvent.mouseUp(result.container.querySelector(".conversation")!);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("Copy this reply text."));
    writeText.mockClear();
    const outside = document.createElement("p");
    outside.textContent = "Outside the ledger";
    document.body.appendChild(outside);
    const outsideRange = document.createRange();
    outsideRange.selectNodeContents(outside);
    selection.removeAllRanges();
    selection.addRange(outsideRange);
    fireEvent.mouseUp(result.container.querySelector(".conversation")!);
    expect(writeText).not.toHaveBeenCalled();
    outside.remove();
  });
});

describe("earlier-message pagination affordance", () => {
  it("shows the daemon's count, a loading state while fetching, and a beginning-of-conversation result", async () => {
    items = [row("live:1", "Now")];
    const original = vi.mocked(daemonCall).getMockImplementation()!;
    let resolvePage!: (value: {items: TranscriptItem[]; next_before: number | null; older_message_count?: number | null}) => void;
    vi.mocked(daemonCall).mockImplementation(async (method, ...args) => {
      if (method === "agent.transcript_watch") return (args[0] as {on:boolean}).on ? { items, next_before:10, older_message_count:42 } : null;
      if (method === "agent.transcript_page") return new Promise((resolve) => { resolvePage = resolve; });
      return original(method, ...args);
    });
    mount();
    const older = await screen.findByRole("button", {name:"Load 42 earlier messages"});
    fireEvent.click(older);
    await waitFor(() => expect(screen.getByRole("button", {name:"Loading earlier messages…"})).toBeDisabled());
    resolvePage({ items:[row("old:1", "The very first message")], next_before:null, older_message_count:null });
    await screen.findByText("The very first message");
    expect(await screen.findByText("Beginning of conversation")).toBeInTheDocument();
    expect(screen.queryByRole("button", {name:/Load/})).not.toBeInTheDocument();
  });

  it("reveals already-loaded rows beyond the render cap for free before asking the daemon for more", async () => {
    items = Array.from({length: 260}, (_, i) => row(`h${i}`, `Message ${i}`));
    const result = mount();
    await screen.findByText("Message 259");
    expect(result.container.querySelectorAll("article")).toHaveLength(250);
    expect(screen.queryByText("Message 0")).not.toBeInTheDocument();
    const older = await screen.findByRole("button", {name:"Load 10 earlier messages"});
    fireEvent.click(older);
    await screen.findByText("Message 0");
    expect(result.container.querySelectorAll("article")).toHaveLength(260);
    expect(daemonCall).not.toHaveBeenCalledWith("agent.transcript_page", expect.anything());
  });
});

function stubScrollMetrics(el: HTMLElement, metrics: { scrollHeight: number; clientHeight: number; scrollTop: number }) {
  Object.defineProperty(el, "scrollHeight", { value: metrics.scrollHeight, configurable: true });
  Object.defineProperty(el, "clientHeight", { value: metrics.clientHeight, configurable: true });
  el.scrollTop = metrics.scrollTop;
}

describe("cached first paint and the skeleton loading state (round 6 item 2)", () => {
  it("shows skeleton rows, not a hard-coded loading paragraph, while a never-before-seen window is still loading", async () => {
    vi.mocked(daemonCall).mockImplementation(() => new Promise(() => {}));
    const result = mount();
    expect(screen.getByRole("status", { name: "Opening conversation" })).toBeInTheDocument();
    expect(screen.queryByText("Opening conversation…")).not.toBeInTheDocument();
    expect(result.container.querySelectorAll(".conversation-skeleton-row")).toHaveLength(3);
  });

  it("paints the previously loaded page instantly on a fresh mount of the exact same window, with no skeleton", async () => {
    items = [row("a1", "First answer")];
    const first = mount();
    await screen.findByText("First answer");
    first.unmount();
    cleanup();
    // A never-resolving watch on the fresh mount proves the paint came from the cache, not from
    // this mount's own (still pending) round trip.
    vi.mocked(daemonCall).mockImplementation((method) => (method === "agent.transcript_watch" ? new Promise(() => {}) : Promise.resolve(null)));
    mount();
    expect(screen.queryByRole("status", { name: "Opening conversation" })).not.toBeInTheDocument();
    expect(screen.getByText("First answer")).toBeInTheDocument();
  });
});

describe("adaptive dialog polling (round 6 item 3)", () => {
  it("stops polling agent.prompt once the pane is no longer the displayed chat view, though it stays visible", async () => {
    const [shown, setShown] = createSignal(true);
    render(() => <ConversationPane target={target} kind="codex" visible shown={shown()} onTerminal={vi.fn()} />);
    await waitFor(() => expect(daemonCall).toHaveBeenCalledWith("agent.prompt", { lane_id: 7, window: "lane-7/1" }));
    vi.mocked(daemonCall).mockClear();
    setShown(false);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(daemonCall).not.toHaveBeenCalledWith("agent.prompt", expect.anything());
  });

  it("latches a dialog straight from the transcript's own dialog item, even while agent.prompt never resolves", async () => {
    const original = vi.mocked(daemonCall).getMockImplementation()!;
    vi.mocked(daemonCall).mockImplementation((method, ...args) => (method === "agent.prompt" ? new Promise(() => {}) : original(method, ...args)));
    items = [{ id: "d1", kind: "dialog", role: "tools", text: "Allow Bash: bun run build?", at: null, dialog: { title: null, question: "Allow Bash: bun run build?", body: [], options: [{ number: 1, text: "Yes" }, { number: 2, text: "No" }], selected: 0 } }];
    mount();
    // A unique query: the ledger also renders the raw dialog item's own question text, so this
    // asserts on the footer's answer control rather than risking a multi-match on the text alone.
    expect(await screen.findByRole("button", { name: "No" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Reply to codex" })).toBeDisabled();
  });
});

describe("Latest output pill: right-edge anchoring, unread count, and auto-paging (round 6 items 4 and 5)", () => {
  it("auto-loads exactly one page per near-top scroll gesture, preserving the scroll anchor, and never fires again mid-gesture", async () => {
    items = Array.from({ length: 5 }, (_, i) => row(`m${i}`, `Message ${i}`));
    const result = mount();
    await screen.findByText("Message 4");
    const scrollEl = result.container.querySelector(".conversation-scroll") as HTMLElement;
    stubScrollMetrics(scrollEl, { scrollHeight: 2000, clientHeight: 400, scrollTop: 1600 });
    fireEvent.scroll(scrollEl);
    expect(screen.queryByRole("button", { name: /Latest output/ })).not.toBeInTheDocument();
    stubScrollMetrics(scrollEl, { scrollHeight: 2000, clientHeight: 400, scrollTop: 10 });
    fireEvent.scroll(scrollEl);
    await waitFor(() => expect(daemonCall).toHaveBeenCalledWith("agent.transcript_page", { ...target, before: 120 }));
    // Simulate the page landing and the ledger growing before the anchor-restoring frame runs.
    stubScrollMetrics(scrollEl, { scrollHeight: 2300, clientHeight: 400, scrollTop: 10 });
    await screen.findByText("Earlier history");
    await waitFor(() => expect(scrollEl.scrollTop).toBe(310));
    // Still sitting at the top after the restore (a short loaded page): a second near-top scroll
    // event must not fire a second fetch until the gesture actually ends.
    fireEvent.scroll(scrollEl);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(vi.mocked(daemonCall).mock.calls.filter(([method]) => method === "agent.transcript_page")).toHaveLength(1);
  });

  it("shows an unread count on the pill while scrolled up and clears it once the operator returns to the latest output", async () => {
    items = [row("a1", "First answer")];
    const result = mount();
    await screen.findByText("First answer");
    const scrollEl = result.container.querySelector(".conversation-scroll") as HTMLElement;
    // Mid-scroll, deliberately not near the top: isolates the unread badge from auto-paging
    // (covered separately above), since older() explicitly excludes its own loaded rows from
    // this count and a real scroll gesture would rarely land both at once regardless.
    stubScrollMetrics(scrollEl, { scrollHeight: 2000, clientHeight: 400, scrollTop: 800 });
    fireEvent.scroll(scrollEl);
    const pill = await screen.findByRole("button", { name: /Latest output/ });
    expect(result.container.querySelector(".conversation-latest-count")).toBeNull();
    update([row("a2", "Second answer")]);
    await screen.findByText("Second answer");
    expect(result.container.querySelector(".conversation-latest-count")).toHaveTextContent("1");
    stubScrollMetrics(scrollEl, { scrollHeight: 2000, clientHeight: 400, scrollTop: 1600 });
    fireEvent.click(pill);
    expect(result.container.querySelector(".conversation-latest-count")).toBeNull();
  });

  it("anchors the pill to the ledger's own right edge, not the pane's outer edge", async () => {
    items = [row("a1", "First answer")];
    const result = mount();
    await screen.findByText("First answer");
    const scrollEl = result.container.querySelector(".conversation-scroll") as HTMLElement;
    stubScrollMetrics(scrollEl, { scrollHeight: 2000, clientHeight: 400, scrollTop: 10 });
    fireEvent.scroll(scrollEl);
    await screen.findByRole("button", { name: /Latest output/ });
    const anchor = result.container.querySelector(".conversation-latest-anchor");
    expect(anchor?.parentElement).toHaveClass("conversation-scroll-area");
    expect(anchor?.parentElement).not.toHaveClass("conversation-main");
  });
});


describe("structured fleet mail", () => {
  it("shows sender and time and seats a consumed delivery outside the pinned queue", async () => {
    const mail: TranscriptItem = {id:"sent:mail",kind:"mail",role:"user",text:"Please review the changes",at:"2026-09-11T10:36:00Z",partial:true,mail:{id:"m1",sender:"lane-2/1",reply_to:null}};
    items = [mail];
    const original = vi.mocked(daemonCall).getMockImplementation()!;
    vi.mocked(daemonCall).mockImplementation(async (method,...args) => method === "agent.transcript_watch" ? {items,next_before:null,input_states:{"sent:mail":"sent"}} : original(method,...args));
    const result = mount();
    await screen.findByText(/Mail from lane-2\/1/);
    expect(result.container.querySelector(".conversation-pending-queue time")).toHaveAttribute("datetime",mail.at);
    update([{...mail,partial:false}],[],{input_states:{},order:["sent:mail"]});
    await waitFor(() => expect(result.container.querySelector(".conversation-pending-queue")).toBeNull());
    expect(result.container.querySelector(".conversation-ledger")).toHaveTextContent("Please review the changes");
    expect(result.container.textContent).not.toContain("[REPOMAIL");
  });
});
