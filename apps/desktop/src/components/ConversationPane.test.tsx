import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DaemonEvent } from "../ipc/rpc";
import type { TranscriptItem } from "../bindings";
import { daemonCall, subscribeDaemon } from "../ipc/rpc";
import ConversationPane, { dialogSummary, groupTurnWork } from "./ConversationPane";
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
