import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import AttachmentComposer from "./AttachmentComposer";
import type { CommandCatalog } from "../../bindings";
vi.mock("@tauri-apps/api/core", () => ({ invoke:vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open:vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });

const emptyCatalog: CommandCatalog = { commands: [], models: [], model_command: null, efforts: [], effort_command: null };
function props(overrides: Partial<Parameters<typeof AttachmentComposer>[0]> = {}) {
  return {
    kind: "codex", disabled: false, busy: false, onSend: vi.fn(),
    catalog: emptyCatalog, catalogError: false, catalogLoading: false,
    onSelectEffort: () => {}, onSelectModel: vi.fn(), displayed: () => true,
    history: [] as string[], historyUnavailable: false, historyError: null as string | null,
    ...overrides,
  };
}

describe("chat attachments", () => {
  it("shows picked paths as removable chips and preserves the draft on send failure", async () => {
    vi.mocked(open).mockResolvedValue(["/Users/me/layout reference.png", "/Users/me/notes.md"]);
    const send = vi.fn().mockResolvedValueOnce(false).mockResolvedValueOnce(true);
    render(() => <AttachmentComposer {...props({ onSend: send })} />);
    fireEvent.input(screen.getByRole("textbox"), { target:{value:"Review this"} });
    fireEvent.click(screen.getByRole("button", {name:"Attach images or files"}));
    await screen.findByText("layout reference.png");
    expect(screen.getByRole("textbox")).toHaveValue("Review this\n\n[Image #1]");
    fireEvent.click(screen.getByRole("button", {name:"Remove notes.md"}));
    fireEvent.click(screen.getByRole("button", {name:"Send reply"}));
    await waitFor(() => expect(send).toHaveBeenCalledWith('Review this\n\nAttached file: "/Users/me/layout reference.png"'));
    expect(screen.getByText("layout reference.png")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", {name:"Send reply"}));
    await waitFor(() => expect(screen.queryByText("layout reference.png")).not.toBeInTheDocument());
    expect(screen.getByRole("textbox")).toHaveValue("");
  });
  it("stages pasted image bytes before permitting an attachment-only send", async () => {
    let saved!: (path:string) => void;
    vi.mocked(invoke).mockImplementation(() => new Promise((resolve) => { saved = resolve as typeof saved; }));
    const send = vi.fn().mockResolvedValue(true);
    render(() => <AttachmentComposer {...props({ kind: "claude-code", onSend: send })} />);
    const file = {name:"image.png",size:3,arrayBuffer:async () => new Uint8Array([1,2,3]).buffer};
    fireEvent.paste(screen.getByRole("textbox"), {clipboardData:{files:[file]}});
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("save_chat_attachment", {name:"image.png",bytes:[1,2,3]}));
    expect(screen.getByRole("button", {name:"Send reply"})).toBeDisabled();
    saved("/stable/attachment-image.png");
    await screen.findByText("image.png");
    fireEvent.click(screen.getByRole("button", {name:"Send reply"}));
    await waitFor(() => expect(send).toHaveBeenCalledWith('Attached file: "/stable/attachment-image.png"'));
  });
  it("routes a dropped image through the same save/marker pipeline as paste, with a visible drop target", async () => {
    vi.mocked(invoke).mockResolvedValue("/stable/dropped-image.png");
    const send = vi.fn().mockResolvedValue(true);
    const result = render(() => <AttachmentComposer {...props({ onSend: send })} />);
    const dropZone = result.container.querySelector(".conversation-reply")!;
    const file = { name:"dropped.png", size:3, arrayBuffer:async () => new Uint8Array([9,9,9]).buffer };
    fireEvent.dragEnter(dropZone, { dataTransfer:{ types:["Files"] } });
    expect(dropZone).toHaveClass("is-drag-target");
    expect(screen.getByText("Drop to attach")).toBeInTheDocument();
    fireEvent.drop(dropZone, { dataTransfer:{ types:["Files"], files:[file] } });
    expect(dropZone).not.toHaveClass("is-drag-target");
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("save_chat_attachment", { name:"dropped.png", bytes:[9,9,9] }));
    await screen.findByText("dropped.png");
    expect(screen.getByRole("textbox")).toHaveValue("[Image #1]");
    fireEvent.click(screen.getByRole("button", { name:"Send reply" }));
    await waitFor(() => expect(send).toHaveBeenCalledWith('Attached file: "/stable/dropped-image.png"'));
  });
  it("ignores a drag that carries no files, such as reordering an attachment chip", () => {
    const result = render(() => <AttachmentComposer {...props()} />);
    const dropZone = result.container.querySelector(".conversation-reply")!;
    fireEvent.dragEnter(dropZone, { dataTransfer:{ types:["text/plain"] } });
    expect(dropZone).not.toHaveClass("is-drag-target");
  });
  it("keeps typed content and offers recovery when paste staging fails", async () => {
    vi.mocked(invoke).mockRejectedValue(new Error("Disk full"));
    render(() => <AttachmentComposer {...props()} />);
    fireEvent.input(screen.getByRole("textbox"), {target:{value:"Keep this draft"}});
    fireEvent.paste(screen.getByRole("textbox"), {clipboardData:{files:[{name:"image.png",size:1,arrayBuffer:async () => new Uint8Array([1]).buffer}]}});
    expect(await screen.findByRole("alert")).toHaveTextContent("Disk full");
    expect(screen.getByRole("textbox")).toHaveValue("Keep this draft");
    expect(screen.getByRole("button", {name:"Attach images or files"})).toBeEnabled();
  });
});

describe("inline image markers, positioned like Claude TUI", () => {
  it("drops the marker where the caret was, not appended at the end", async () => {
    vi.mocked(open).mockResolvedValue(["/Users/me/shot.png"]);
    render(() => <AttachmentComposer {...props()} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.input(field, { target:{value:"Check the layout please"} });
    field.setSelectionRange(10, 10); // right after "Check the "
    fireEvent.click(screen.getByRole("button", {name:"Attach images or files"}));
    await screen.findByText("shot.png");
    expect(field.value).toBe("Check the\n\n[Image #1]\n\nlayout please");
  });
  it("renumbers remaining markers in the draft after an earlier image is removed", async () => {
    vi.mocked(open).mockResolvedValue(["/Users/me/one.png", "/Users/me/two.png"]);
    render(() => <AttachmentComposer {...props()} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.click(screen.getByRole("button", {name:"Attach images or files"}));
    await screen.findByText("two.png");
    expect(field.value).toBe("[Image #1]\n\n[Image #2]");
    fireEvent.click(screen.getByRole("button", {name:"Remove one.png"}));
    expect(field.value).toBe("[Image #1]");
  });
});

describe("composer history, cycling the agent's own recall like the TUI (round 8 item 4, round 13 wired to agent.input_history)", () => {
  it("recalls the agent's own history on ArrowUp from an empty composer, oldest last, and ArrowDown returns to empty", () => {
    render(() => <AttachmentComposer {...props({ history: ["first message", "second message"] })} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.keyDown(field, { key: "ArrowUp" });
    expect(field).toHaveValue("second message");
    fireEvent.keyDown(field, { key: "ArrowUp" });
    expect(field).toHaveValue("first message");
    fireEvent.keyDown(field, { key: "ArrowUp" }); // clamps at the oldest entry
    expect(field).toHaveValue("first message");
    fireEvent.keyDown(field, { key: "ArrowDown" });
    expect(field).toHaveValue("second message");
    fireEvent.keyDown(field, { key: "ArrowDown" });
    expect(field).toHaveValue("");
  });
  it("does not recall history when the composer already holds an unsent single-line draft", () => {
    render(() => <AttachmentComposer {...props({ history: ["sent earlier"] })} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.input(field, { target: { value: "in-progress draft" } });
    fireEvent.keyDown(field, { key: "ArrowUp" });
    expect(field).toHaveValue("in-progress draft");
  });
  it("never steals the arrows while editing multiline text, typed or recalled", () => {
    render(() => <AttachmentComposer {...props({ history: ["line one\nline two"] })} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.keyDown(field, { key: "ArrowUp" });
    expect(field).toHaveValue("line one\nline two");
    // The recalled entry is itself multiline: a further Up must not cycle again, it is ordinary
    // cursor movement inside it now.
    fireEvent.keyDown(field, { key: "ArrowUp" });
    expect(field).toHaveValue("line one\nline two");
  });
  it("exits history mode the moment the operator edits a recalled entry, preserving that edit as an ordinary draft", () => {
    render(() => <AttachmentComposer {...props({ history: ["sent message"] })} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.keyDown(field, { key: "ArrowUp" });
    expect(field).toHaveValue("sent message");
    fireEvent.input(field, { target: { value: "sent message, edited" } });
    fireEvent.keyDown(field, { key: "ArrowDown" });
    expect(field).toHaveValue("sent message, edited");
  });
  it("says so, rather than silently doing nothing, when this kind has no readable history store", () => {
    render(() => <AttachmentComposer {...props({ history: [], historyUnavailable: true, kind: "hermes" })} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.keyDown(field, { key: "ArrowUp" });
    expect(field).toHaveValue("");
    expect(screen.getByRole("alert")).toHaveTextContent("No input history available for hermes.");
  });
  it("surfaces a failed history fetch distinctly, not as a silent empty history", () => {
    render(() => <AttachmentComposer {...props({ history: [], historyError: "daemon unreachable" })} />);
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "ArrowUp" });
    expect(screen.getByRole("alert")).toHaveTextContent("daemon unreachable");
  });
  it("does nothing and shows nothing on ArrowUp while history is still loading (empty, unavailable false, no error)", () => {
    render(() => <AttachmentComposer {...props({ history: [] })} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.keyDown(field, { key: "ArrowUp" });
    expect(field).toHaveValue("");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});

it("shrinks after deleting text and after a successful send", async () => {
  render(() => <AttachmentComposer {...props({ onSend: vi.fn().mockResolvedValue(true) })} />);
  const field = screen.getByRole("textbox") as HTMLTextAreaElement;
  Object.defineProperty(field, "scrollHeight", {get:() => field.value.length > 50 ? 200 : 40});
  fireEvent.input(field, {target:{value:"long draft ".repeat(20)}});
  expect(field.style.height).toBe("160px");
  fireEvent.input(field, {target:{value:"One line"}});
  expect(field.style.height).toBe("40px");
  fireEvent.input(field, {target:{value:"long draft ".repeat(20)}});
  fireEvent.click(screen.getByRole("button", {name:"Send reply"}));
  await waitFor(() => expect(field).toHaveValue(""));
  expect(field.style.height).toBe("40px");
});

describe("native slash-command palette (round 10)", () => {
  const catalog: CommandCatalog = {
    commands: [
      { name: "model", description: "Change the active model", source: "builtin", one_shot: true },
      { name: "compact", description: "Summarize the conversation", source: "builtin", one_shot: true },
      { name: "myplugin:review", description: "Review the diff", source: "plugin", one_shot: true },
    ],
    models: [],
    model_command: null, efforts: [], effort_command: null,
  };

  it("opens on a bare slash, filters as the operator types, and shows the highlighted row's description", () => {
    render(() => <AttachmentComposer {...props({ catalog })} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.input(field, { target: { value: "/" } });
    expect(screen.getByRole("listbox", { name: "Slash commands" })).toBeInTheDocument();
    expect(screen.getAllByRole("option")).toHaveLength(3);
    fireEvent.input(field, { target: { value: "/co" } });
    const options = screen.getAllByRole("option");
    expect(options).toHaveLength(1);
    expect(options[0]).toHaveTextContent("compact");
    expect(screen.getByText("Summarize the conversation")).toBeInTheDocument();
  });

  it("shows a plugin command namespaced with its bare alias in parentheses", () => {
    render(() => <AttachmentComposer {...props({ catalog })} />);
    fireEvent.input(screen.getByRole("textbox"), { target: { value: "/rev" } });
    expect(screen.getByRole("option")).toHaveTextContent("myplugin:review (review)");
  });

  it("closes the palette the moment a space starts an argument", () => {
    render(() => <AttachmentComposer {...props({ catalog })} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.input(field, { target: { value: "/model" } });
    expect(screen.getByRole("listbox", { name: "Slash commands" })).toBeInTheDocument();
    fireEvent.input(field, { target: { value: "/model " } });
    expect(screen.queryByRole("listbox", { name: "Slash commands" })).not.toBeInTheDocument();
  });

  it("renders 'no commands known for this agent' rather than a guess, for an empty catalog", () => {
    render(() => <AttachmentComposer {...props()} />);
    fireEvent.input(screen.getByRole("textbox"), { target: { value: "/" } });
    expect(screen.getByText("No commands known for this agent.")).toBeInTheDocument();
  });

  it("arrows move the highlighted row and Enter sends the highlighted command as a one-shot line, not history recall", async () => {
    const send = vi.fn().mockResolvedValue(true);
    render(() => <AttachmentComposer {...props({ catalog, onSend: send })} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.input(field, { target: { value: "/" } });
    fireEvent.keyDown(field, { key: "ArrowDown" });
    expect(screen.getAllByRole("option")[1]).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(field, { key: "Enter" });
    await waitFor(() => expect(send).toHaveBeenCalledWith("/compact"));
  });

  it("Escape dismisses the palette without clearing the typed text, and does not touch history", () => {
    render(() => <AttachmentComposer {...props({ catalog })} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.input(field, { target: { value: "/mo" } });
    fireEvent.keyDown(field, { key: "Escape" });
    expect(screen.queryByRole("listbox", { name: "Slash commands" })).not.toBeInTheDocument();
    expect(field).toHaveValue("/mo");
  });

  it("does not let ArrowUp fall through to history recall while the palette is open", () => {
    render(() => <AttachmentComposer {...props({ catalog, history: ["sent earlier"] })} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.input(field, { target: { value: "/" } });
    fireEvent.keyDown(field, { key: "ArrowUp" });
    // Moved the palette highlight (wrapped to the last row), not recalled "sent earlier".
    expect(field).toHaveValue("/");
    expect(screen.getAllByRole("option")[2]).toHaveAttribute("aria-selected", "true");
  });

  it("shows a distinct message for a failed catalog fetch, not the empty-catalog wording", () => {
    render(() => <AttachmentComposer {...props({ catalogError: true })} />);
    fireEvent.input(screen.getByRole("textbox"), { target: { value: "/" } });
    expect(screen.getByRole("alert")).toHaveTextContent(/Couldn't load commands/);
    expect(screen.queryByText("No commands known for this agent.")).not.toBeInTheDocument();
  });

  it("shows a loading message instead of the empty-catalog wording while the first fetch is in flight", () => {
    render(() => <AttachmentComposer {...props({ catalogLoading: true })} />);
    fireEvent.input(screen.getByRole("textbox"), { target: { value: "/" } });
    expect(screen.getByText("Loading commands…")).toBeInTheDocument();
    expect(screen.queryByText("No commands known for this agent.")).not.toBeInTheDocument();
  });

  it("never portals the palette into a pane that is not the one on screen", () => {
    render(() => <AttachmentComposer {...props({ catalog, displayed: () => false })} />);
    fireEvent.input(screen.getByRole("textbox"), { target: { value: "/" } });
    expect(screen.queryByRole("listbox", { name: "Slash commands" })).not.toBeInTheDocument();
  });

  it("tears the palette portal down the moment the pane stops being displayed", () => {
    const [displayed, setDisplayed] = createSignal(true);
    render(() => <AttachmentComposer {...props({ catalog, displayed })} />);
    fireEvent.input(screen.getByRole("textbox"), { target: { value: "/" } });
    expect(screen.getByRole("listbox", { name: "Slash commands" })).toBeInTheDocument();
    setDisplayed(false);
    expect(screen.queryByRole("listbox", { name: "Slash commands" })).not.toBeInTheDocument();
  });
});

describe("native model picker (round 10)", () => {
  const catalog: CommandCatalog = {
    commands: [],
    models: [
      { id: "opus", label: "Claude Opus", current: false },
      { id: "sonnet", label: "Claude Sonnet", current: true },
      { id: "haiku", label: "Claude Haiku", current: false },
    ],
    model_command: "/model", efforts: [], effort_command: null,
  };

  it("opens a panel from the model chip listing every model, a check on the current one", () => {
    render(() => <AttachmentComposer {...props({ catalog, model: "Claude Sonnet" })} />);
    fireEvent.click(screen.getByRole("button", { name: "Change codex model" }));
    const panel = screen.getByRole("menu", { name: "Choose model" });
    expect(panel).toBeInTheDocument();
    expect(screen.getAllByRole("menuitemradio")).toHaveLength(3);
    expect(screen.getByRole("menuitemradio", { name: /Claude Sonnet/ })).toHaveAttribute("aria-checked", "true");
  });

  it("selecting a model calls onSelectModel with its id and closes the panel", () => {
    const onSelectModel = vi.fn();
    render(() => <AttachmentComposer {...props({ catalog, onSelectModel })} />);
    fireEvent.click(screen.getByRole("button", { name: "Change codex model" }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: /Claude Opus/ }));
    expect(onSelectModel).toHaveBeenCalledWith("opus");
    expect(screen.queryByRole("menu", { name: "Choose model" })).not.toBeInTheDocument();
  });

  it("shows the plain non-interactive label when the catalog has no models at all", () => {
    const result = render(() => <AttachmentComposer {...props({ model: "gpt-5-codex" })} />);
    expect(screen.queryByRole("button", { name: "Change codex model" })).not.toBeInTheDocument();
    expect(result.container.querySelector(".composer-agent")?.textContent).toBe("codex · gpt-5-codex");
  });

  it("never falls back to a terminal route: a kind with models but no confirmed one-shot form still opens the native panel", () => {
    const unconfirmed: CommandCatalog = {
      commands: [],
      models: [{ id: "gpt-6-astra", label: "GPT-6-Astra", current: true }],
      model_command: null, efforts: [], effort_command: null,
    };
    render(() => <AttachmentComposer {...props({ catalog: unconfirmed })} />);
    fireEvent.click(screen.getByRole("button", { name: "Change codex model" }));
    const panel = screen.getByRole("menu", { name: "Choose model" });
    expect(panel).toBeInTheDocument();
    expect(panel).toHaveTextContent(/can't switch codex's model/i);
    expect(screen.queryByRole("menuitemradio")).not.toBeInTheDocument();
  });

  it("tears the model panel portal down the moment the pane stops being displayed", () => {
    const [displayed, setDisplayed] = createSignal(true);
    render(() => <AttachmentComposer {...props({ catalog, displayed })} />);
    fireEvent.click(screen.getByRole("button", { name: "Change codex model" }));
    expect(screen.getByRole("menu", { name: "Choose model" })).toBeInTheDocument();
    setDisplayed(false);
    expect(screen.queryByRole("menu", { name: "Choose model" })).not.toBeInTheDocument();
  });
});
