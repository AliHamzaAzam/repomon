import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());
const convertFileSrcMock = vi.hoisted(() =>
  vi.fn((path: string) => `asset://localhost/${encodeURIComponent(path)}`),
);
const openPathMock = vi.hoisted(() => vi.fn());
const daemonCallMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
  convertFileSrc: convertFileSrcMock,
}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openPath: openPathMock }));
vi.mock("../ipc/rpc", () => ({
  daemonCall: (method: string, params?: unknown) => daemonCallMock(method, params),
}));

import ImageViewer from "./ImageViewer";
import { resetWorktreeAssetsAllowedCacheForTests } from "../ipc/assets";

function setNaturalSize(img: HTMLImageElement, width: number, height: number) {
  Object.defineProperty(img, "naturalWidth", { value: width, configurable: true });
  Object.defineProperty(img, "naturalHeight", { value: height, configurable: true });
}

// The <img> element is mounted immediately (see ImageViewer's "always mounted" comment), but its
// onload/onerror handlers are only bound once the async asset-protocol grant resolves. Firing a
// load/error event before that point hits no handler at all, so every test waits for the real
// asset URL to land on the element first.
async function waitForAssetSrc(img: HTMLImageElement, absolutePath: string) {
  await waitFor(() => expect(img.src).toContain(encodeURIComponent(absolutePath)));
}

class ResizeObserverMock {
  static instances: ResizeObserverMock[] = [];
  callback: ResizeObserverCallback;
  observed: Element[] = [];
  constructor(callback: ResizeObserverCallback) {
    this.callback = callback;
    ResizeObserverMock.instances.push(this);
  }
  observe(el: Element) {
    this.observed.push(el);
  }
  unobserve() {}
  disconnect() {
    this.observed = [];
  }
  trigger() {
    this.callback([] as ResizeObserverEntry[], this as unknown as ResizeObserver);
  }
}

beforeEach(() => {
  invokeMock.mockReset().mockResolvedValue(undefined);
  convertFileSrcMock.mockClear();
  openPathMock.mockReset().mockResolvedValue(undefined);
  daemonCallMock.mockReset();
  resetWorktreeAssetsAllowedCacheForTests();
  ResizeObserverMock.instances = [];
  vi.stubGlobal("ResizeObserver", ResizeObserverMock);
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("ImageViewer", () => {
  it("streams the image through the asset protocol and renders its URL", async () => {
    render(() => <ImageViewer worktreeRoot="/repo/lane" laneId={1} path="assets/logo.png" size={2048} />);

    await waitFor(() => expect(convertFileSrcMock).toHaveBeenCalledWith("/repo/lane/assets/logo.png"));
    const img = screen.getByTestId("image-viewer-img") as HTMLImageElement;
    expect(img.src).toContain(encodeURIComponent("/repo/lane/assets/logo.png"));
  });

  it("changes the scale and the zoom label from the zoom buttons and the keyboard", async () => {
    render(() => <ImageViewer worktreeRoot="/repo/lane" laneId={1} path="assets/logo.png" />);

    const img = (await screen.findByTestId("image-viewer-img")) as HTMLImageElement;
    await waitForAssetSrc(img, "/repo/lane/assets/logo.png");
    setNaturalSize(img, 400, 300);
    fireEvent.load(img);

    const zoomLabel = () => screen.getByTitle("Reset zoom to fit");
    await waitFor(() => expect(zoomLabel()).toBeInTheDocument());

    const root = screen.getByTestId("image-viewer-root");
    fireEvent.keyDown(root, { key: "1", metaKey: true });
    expect(zoomLabel()).toHaveTextContent("100%");

    fireEvent.click(screen.getByLabelText("Zoom in"));
    expect(zoomLabel()).toHaveTextContent("125%");

    fireEvent.click(screen.getByLabelText("Zoom out"));
    fireEvent.click(screen.getByLabelText("Zoom out"));
    expect(zoomLabel()).toHaveTextContent("80%");

    fireEvent.keyDown(root, { key: "0", metaKey: true });
    expect(screen.getByLabelText("Fit to pane").className).toContain("bg-signal/15");
  });

  it("recomputes the fit scale on a debounced stage resize while in fit mode", async () => {
    render(() => <ImageViewer worktreeRoot="/repo/lane" laneId={1} path="assets/logo.png" />);

    const img = (await screen.findByTestId("image-viewer-img")) as HTMLImageElement;
    await waitForAssetSrc(img, "/repo/lane/assets/logo.png");
    setNaturalSize(img, 800, 400);
    fireEvent.load(img);
    await screen.findByTitle("Reset zoom to fit");

    const stage = screen.getByTestId("image-stage");
    Object.defineProperty(stage, "clientWidth", { value: 432, configurable: true });
    Object.defineProperty(stage, "clientHeight", { value: 300, configurable: true });

    vi.useFakeTimers();
    ResizeObserverMock.instances[0]?.trigger();
    await vi.advanceTimersByTimeAsync(150);
    vi.useRealTimers();

    // (432 - 32 gutter) / 800 width = 0.5; (300 - 32 gutter) / 400 height = 0.67 -> min is 50%.
    await waitFor(() => expect(screen.getByTitle("Reset zoom to fit")).toHaveTextContent("50%"));
  });

  it("toggles between fit and actual size on double-click", async () => {
    render(() => <ImageViewer worktreeRoot="/repo/lane" laneId={1} path="assets/logo.png" />);

    const img = (await screen.findByTestId("image-viewer-img")) as HTMLImageElement;
    await waitForAssetSrc(img, "/repo/lane/assets/logo.png");
    setNaturalSize(img, 400, 300);
    fireEvent.load(img);
    await screen.findByTitle("Reset zoom to fit");

    const stage = screen.getByTestId("image-stage");
    const fitButton = screen.getByLabelText("Fit to pane");
    const actualButton = screen.getByLabelText("Actual size");
    expect(fitButton.className).toContain("bg-signal/15");

    fireEvent.dblClick(stage);
    await waitFor(() => expect(actualButton.className).toContain("bg-signal/15"));
    expect(screen.getByTitle("Reset zoom to fit")).toHaveTextContent("100%");

    fireEvent.dblClick(stage);
    await waitFor(() => expect(fitButton.className).toContain("bg-signal/15"));
  });

  it("shows the primary open-externally action on the error state", async () => {
    daemonCallMock.mockRejectedValue(new Error("not found"));

    render(() => <ImageViewer worktreeRoot="/repo/lane" laneId={1} path="assets/broken.png" />);
    const img = (await screen.findByTestId("image-viewer-img")) as HTMLImageElement;
    await waitForAssetSrc(img, "/repo/lane/assets/broken.png");

    // The primary asset-protocol load fails, falling back to file.read_raw, which also fails here.
    fireEvent.error(img);

    expect(await screen.findByText("Couldn't load preview")).toBeInTheDocument();
    const button = screen.getByRole("button", { name: "Open in system viewer" });
    expect(button.className).toContain("bg-signal");

    fireEvent.click(button);
    await waitFor(() => expect(openPathMock).toHaveBeenCalledWith("/repo/lane/assets/broken.png"));
  });

  it("shows the too-large guard above the 50 MiB decode threshold without attempting a load", async () => {
    render(() => (
      <ImageViewer worktreeRoot="/repo/lane" laneId={1} path="assets/huge.png" size={60 * 1024 * 1024} />
    ));

    expect(await screen.findByText("Too large to preview here")).toBeInTheDocument();
    expect(convertFileSrcMock).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Open in system viewer" }).className).toContain("bg-signal");
  });

  it("ignores a stale fallback load after the path changes", async () => {
    let resolveFallback!: (value: { base64: string; mime: string; size: number }) => void;
    daemonCallMock.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveFallback = resolve;
        }),
    );

    const [path, setPath] = createSignal("assets/old.png");
    render(() => <ImageViewer worktreeRoot="/repo/lane" laneId={1} path={path()} />);

    const img = (await screen.findByTestId("image-viewer-img")) as HTMLImageElement;
    await waitForAssetSrc(img, "/repo/lane/assets/old.png");
    // The primary asset-protocol load for the old path fails; its fallback fetch is left pending.
    fireEvent.error(img);
    await waitFor(() => expect(daemonCallMock).toHaveBeenCalledTimes(1));

    setPath("assets/new.png");
    await waitFor(() => expect(convertFileSrcMock).toHaveBeenCalledWith("/repo/lane/assets/new.png"));
    const freshSrc = img.src;
    expect(freshSrc).toContain(encodeURIComponent("/repo/lane/assets/new.png"));

    // The stale fallback for the old path now resolves - it must not clobber the current image.
    resolveFallback({ base64: "AAAA", mime: "image/png", size: 4 });
    await Promise.resolve();
    await Promise.resolve();

    expect(img.src).toBe(freshSrc);
  });
});
