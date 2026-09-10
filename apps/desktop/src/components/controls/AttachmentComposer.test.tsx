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
    expect(screen.getByRole("textbox")).toHaveValue("Review this");
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
