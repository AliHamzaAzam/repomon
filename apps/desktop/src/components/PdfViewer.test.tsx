import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());
const convertFileSrcMock = vi.hoisted(() =>
  vi.fn((path: string) => `asset://localhost/${encodeURIComponent(path)}`),
);
const openPathMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
  convertFileSrc: convertFileSrcMock,
}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openPath: openPathMock }));

import PdfViewer, { isLinuxWebview } from "./PdfViewer";
import { resetWorktreeAssetsAllowedCacheForTests } from "../ipc/assets";

const originalUserAgent = navigator.userAgent;

function setUserAgent(value: string) {
  Object.defineProperty(navigator, "userAgent", { value, configurable: true });
}

beforeEach(() => {
  invokeMock.mockReset().mockResolvedValue(undefined);
  convertFileSrcMock.mockClear();
  openPathMock.mockReset().mockResolvedValue(undefined);
  resetWorktreeAssetsAllowedCacheForTests();
  setUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15");
});

afterEach(() => {
  cleanup();
  setUserAgent(originalUserAgent);
  vi.useRealTimers();
});

describe("isLinuxWebview", () => {
  it("is true for a Linux WebKitGTK user agent", () => {
    expect(isLinuxWebview("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15")).toBe(true);
  });

  it("is false for Android, which also contains \"Linux\"", () => {
    expect(isLinuxWebview("Mozilla/5.0 (Linux; Android 14)")).toBe(false);
  });

  it("is false for macOS and Windows", () => {
    expect(isLinuxWebview("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe(false);
    expect(isLinuxWebview("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe(false);
  });
});

describe("PdfViewer", () => {
  it("renders the iframe with a convertFileSrc-derived URL on macOS", async () => {
    render(() => <PdfViewer worktreeRoot="/repo/lane" path="docs/report.pdf" size={2048} />);

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("allow_worktree_assets", { path: "/repo/lane" }));

    const iframe = await screen.findByTitle("report.pdf");
    expect(iframe.tagName).toBe("IFRAME");
    expect(convertFileSrcMock).toHaveBeenCalledWith("/repo/lane/docs/report.pdf");
    expect(iframe.getAttribute("src")).toBe(
      `asset://localhost/${encodeURIComponent("/repo/lane/docs/report.pdf")}`,
    );
    expect(screen.getByText("report.pdf")).toBeInTheDocument();
    expect(screen.getByText("2.0 KB")).toBeInTheDocument();
  });

  it("renders the fallback note on a Linux user agent and never mounts an iframe", async () => {
    setUserAgent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko)");

    render(() => <PdfViewer worktreeRoot="/repo/lane" path="docs/report.pdf" />);

    expect(await screen.findByText("Preview unavailable")).toBeInTheDocument();
    expect(screen.getByText(/not available on Linux/)).toBeInTheDocument();
    expect(screen.queryByTitle("report.pdf")).not.toBeInTheDocument();
    // The asset scope is only ever needed to feed the iframe, so Linux never requests it.
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("calls the opener with the absolute path when Open in system viewer is clicked", async () => {
    render(() => <PdfViewer worktreeRoot="/repo/lane" path="docs/report.pdf" />);
    await screen.findByTitle("report.pdf");

    fireEvent.click(screen.getByText("Open in system viewer"));

    await waitFor(() => expect(openPathMock).toHaveBeenCalledWith("/repo/lane/docs/report.pdf"));
  });

  it("shows the load-failure fallback when the iframe fires an error event", async () => {
    render(() => <PdfViewer worktreeRoot="/repo/lane" path="docs/report.pdf" />);
    const iframe = await screen.findByTitle("report.pdf");

    fireEvent.error(iframe);

    expect(await screen.findByText("Couldn't load preview")).toBeInTheDocument();
    expect(screen.queryByTitle("report.pdf")).not.toBeInTheDocument();
  });

  it("treats a load that never fires within 5 seconds as failed", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });

    render(() => <PdfViewer worktreeRoot="/repo/lane" path="docs/report.pdf" />);
    await vi.waitFor(() => expect(screen.queryByTitle("report.pdf")).not.toBeNull());

    await vi.advanceTimersByTimeAsync(5000);

    expect(screen.getByText("Couldn't load preview")).toBeInTheDocument();
  });
});
