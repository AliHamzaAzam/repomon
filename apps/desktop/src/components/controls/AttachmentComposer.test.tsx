import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import AttachmentComposer from "./AttachmentComposer";
vi.mock("@tauri-apps/api/core", () => ({ invoke:vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open:vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });
describe("chat attachments", () => {
  it("shows picked paths as removable chips and preserves the draft on send failure", async () => {
    vi.mocked(open).mockResolvedValue(["/Users/me/layout reference.png", "/Users/me/notes.md"]);
    const send = vi.fn().mockResolvedValueOnce(false).mockResolvedValueOnce(true);
    render(() => <AttachmentComposer kind="codex" disabled={false} busy={false} onSend={send} />);
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
    render(() => <AttachmentComposer kind="claude-code" disabled={false} busy={false} onSend={send} />);
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
    const result = render(() => <AttachmentComposer kind="codex" disabled={false} busy={false} onSend={send} />);
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
    const result = render(() => <AttachmentComposer kind="codex" disabled={false} busy={false} onSend={vi.fn()} />);
    const dropZone = result.container.querySelector(".conversation-reply")!;
    fireEvent.dragEnter(dropZone, { dataTransfer:{ types:["text/plain"] } });
    expect(dropZone).not.toHaveClass("is-drag-target");
  });
  it("keeps typed content and offers recovery when paste staging fails", async () => {
    vi.mocked(invoke).mockRejectedValue(new Error("Disk full"));
    render(() => <AttachmentComposer kind="codex" disabled={false} busy={false} onSend={vi.fn()} />);
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
    render(() => <AttachmentComposer kind="codex" disabled={false} busy={false} onSend={vi.fn()} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.input(field, { target:{value:"Check the layout please"} });
    field.setSelectionRange(10, 10); // right after "Check the "
    fireEvent.click(screen.getByRole("button", {name:"Attach images or files"}));
    await screen.findByText("shot.png");
    expect(field.value).toBe("Check the\n\n[Image #1]\n\nlayout please");
  });
  it("renumbers remaining markers in the draft after an earlier image is removed", async () => {
    vi.mocked(open).mockResolvedValue(["/Users/me/one.png", "/Users/me/two.png"]);
    render(() => <AttachmentComposer kind="codex" disabled={false} busy={false} onSend={vi.fn()} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.click(screen.getByRole("button", {name:"Attach images or files"}));
    await screen.findByText("two.png");
    expect(field.value).toBe("[Image #1]\n\n[Image #2]");
    fireEvent.click(screen.getByRole("button", {name:"Remove one.png"}));
    expect(field.value).toBe("[Image #1]");
  });
});

describe("composer history, cycling previous sent messages like the TUI (round 8 item 4)", () => {
  it("recalls the most recent sent message on ArrowUp from an empty composer, and ArrowDown returns to empty", async () => {
    const send = vi.fn().mockResolvedValue(true);
    render(() => <AttachmentComposer kind="codex" disabled={false} busy={false} onSend={send} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.input(field, { target: { value: "first message" } });
    fireEvent.click(screen.getByRole("button", { name: "Send reply" }));
    await waitFor(() => expect(field).toHaveValue(""));
    fireEvent.input(field, { target: { value: "second message" } });
    fireEvent.click(screen.getByRole("button", { name: "Send reply" }));
    await waitFor(() => expect(field).toHaveValue(""));
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
  it("does not recall history when the composer already holds an unsent single-line draft", async () => {
    const send = vi.fn().mockResolvedValue(true);
    render(() => <AttachmentComposer kind="codex" disabled={false} busy={false} onSend={send} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.input(field, { target: { value: "sent earlier" } });
    fireEvent.click(screen.getByRole("button", { name: "Send reply" }));
    await waitFor(() => expect(field).toHaveValue(""));
    fireEvent.input(field, { target: { value: "in-progress draft" } });
    fireEvent.keyDown(field, { key: "ArrowUp" });
    expect(field).toHaveValue("in-progress draft");
  });
  it("never steals the arrows while editing multiline text, typed or recalled", async () => {
    const send = vi.fn().mockResolvedValue(true);
    render(() => <AttachmentComposer kind="codex" disabled={false} busy={false} onSend={send} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.input(field, { target: { value: "line one\nline two" } });
    fireEvent.click(screen.getByRole("button", { name: "Send reply" }));
    await waitFor(() => expect(field).toHaveValue(""));
    fireEvent.keyDown(field, { key: "ArrowUp" });
    expect(field).toHaveValue("line one\nline two");
    // The recalled entry is itself multiline: a further Up must not cycle again, it is ordinary
    // cursor movement inside it now.
    fireEvent.keyDown(field, { key: "ArrowUp" });
    expect(field).toHaveValue("line one\nline two");
  });
  it("exits history mode the moment the operator edits a recalled entry, preserving that edit as an ordinary draft", async () => {
    const send = vi.fn().mockResolvedValue(true);
    render(() => <AttachmentComposer kind="codex" disabled={false} busy={false} onSend={send} />);
    const field = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.input(field, { target: { value: "sent message" } });
    fireEvent.click(screen.getByRole("button", { name: "Send reply" }));
    await waitFor(() => expect(field).toHaveValue(""));
    fireEvent.keyDown(field, { key: "ArrowUp" });
    expect(field).toHaveValue("sent message");
    fireEvent.input(field, { target: { value: "sent message, edited" } });
    fireEvent.keyDown(field, { key: "ArrowDown" });
    expect(field).toHaveValue("sent message, edited");
  });
});

it("shrinks after deleting text and after a successful send", async () => {
  render(() => <AttachmentComposer kind="codex" disabled={false} busy={false} onSend={vi.fn().mockResolvedValue(true)} />);
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
