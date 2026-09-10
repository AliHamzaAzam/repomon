import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import AttachmentPreview from "./AttachmentPreview";
vi.mock("@tauri-apps/api/core", () => ({ invoke:vi.fn(), convertFileSrc:(path:string) => `asset:${path}` }));
afterEach(() => {cleanup(); vi.clearAllMocks();});
const file = {path:"/stable/image.png",name:"image.png"};
it("loads a real asset-protocol image after granting its file and falls back on decode failure", async () => {
  vi.mocked(invoke).mockResolvedValue(file.path);
  const result = render(() => <AttachmentPreview file={file} number={1} />);
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("allow_chat_attachment_preview", {path:file.path}));
  await waitFor(() => expect(result.container.querySelector("img")).toHaveAttribute("src", "asset:/stable/image.png"));
  const img = result.container.querySelector("img")!;
  fireEvent.load(img);
  expect(screen.getByRole("img", {name:"Image #1: image.png"})).toBeVisible();
  expect(screen.queryByText("image.png")).not.toBeInTheDocument();
  fireEvent.error(img);
  expect(screen.getByText("image.png")).toBeInTheDocument();
  expect(result.container.textContent).not.toContain("/stable");
});
it("shows a file chip when native access fails", async () => {
  vi.mocked(invoke).mockRejectedValue(new Error("missing"));
  const result = render(() => <AttachmentPreview file={file} number={1} />);
  await waitFor(() => expect(invoke).toHaveBeenCalled());
  expect(screen.getByText("PNG")).toBeInTheDocument();
  expect(result.container.querySelector("img")).toBeNull();
  expect(result.container.textContent).not.toContain("/stable");
});
