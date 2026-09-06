import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());
const convertFileSrcMock = vi.hoisted(() =>
  vi.fn((path: string) => `asset://localhost/${encodeURIComponent(path)}`),
);
const openPathMock = vi.hoisted(() => vi.fn());
const getDocumentMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
  convertFileSrc: convertFileSrcMock,
}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openPath: openPathMock }));
vi.mock("pdfjs-dist", () => ({
  getDocument: getDocumentMock,
  GlobalWorkerOptions: { workerSrc: "" },
}));

import PdfViewer from "./PdfViewer";
import { resetWorktreeAssetsAllowedCacheForTests } from "../ipc/assets";

// -- Fake pdf.js surface ----------------------------------------------------------------------
//
// A minimal stand-in for pdf.js's document/page objects: three uniform 600x800 pages, each with
// one text item. Page 1 and page 3 both contain the word "needle" so find-in-page has more than
// one hit to cycle through.

const PAGE_WIDTH = 600;
const PAGE_HEIGHT = 800;

function baseViewportTransform(scale: number): number[] {
  return [scale, 0, 0, -scale, 0, PAGE_HEIGHT * scale];
}

function pageText(pageNumber: number): string {
  if (pageNumber === 2) return "middle page, nothing special here";
  return `page ${pageNumber} has a needle in it`;
}

function makeFakePage(pageNumber: number) {
  return {
    getViewport: ({ scale }: { scale: number }) => ({
      width: PAGE_WIDTH * scale,
      height: PAGE_HEIGHT * scale,
      scale,
      transform: baseViewportTransform(scale),
    }),
    render: vi.fn(() => ({ promise: Promise.resolve(), cancel: vi.fn() })),
    getTextContent: () =>
      Promise.resolve({
        items: [{ str: pageText(pageNumber), transform: [12, 0, 0, 12, 50, 700], width: 6 * pageText(pageNumber).length, hasEOL: false }],
      }),
    cleanup: vi.fn(),
  };
}

function makeFakeDoc(numPages: number) {
  const pages = new Map<number, ReturnType<typeof makeFakePage>>();
  return {
    numPages,
    getPage: (n: number) => {
      if (!pages.has(n)) pages.set(n, makeFakePage(n));
      return Promise.resolve(pages.get(n));
    },
    destroy: vi.fn().mockResolvedValue(undefined),
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Queues up loading-task results for successive `getDocument(...)` calls. */
function queueLoadingTasks(count: number) {
  const tasks = Array.from({ length: count }, () => ({
    ...deferred<unknown>(),
    destroy: vi.fn().mockResolvedValue(undefined),
  }));
  let call = 0;
  getDocumentMock.mockImplementation(() => {
    const task = tasks[Math.min(call, tasks.length - 1)];
    call += 1;
    return { promise: task.promise, destroy: task.destroy };
  });
  return tasks;
}

// -- IntersectionObserver / ResizeObserver mocks -----------------------------------------------

class IntersectionObserverMock {
  static instances: IntersectionObserverMock[] = [];
  callback: IntersectionObserverCallback;
  observed: Element[] = [];
  constructor(callback: IntersectionObserverCallback) {
    this.callback = callback;
    IntersectionObserverMock.instances.push(this);
  }
  observe(el: Element) {
    this.observed.push(el);
  }
  unobserve(el: Element) {
    this.observed = this.observed.filter((e) => e !== el);
  }
  disconnect() {
    this.observed = [];
  }
  takeRecords() {
    return [];
  }
}

function fireIntersection(el: Element, isIntersecting: boolean, ratio = isIntersecting ? 1 : 0) {
  const instance = IntersectionObserverMock.instances.find((i) => i.observed.includes(el));
  instance?.callback(
    [{ target: el, isIntersecting, intersectionRatio: ratio } as IntersectionObserverEntry],
    instance as unknown as IntersectionObserver,
  );
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

async function makeAllPagesVisible() {
  for (let n = 1; n <= 3; n++) {
    const el = await screen.findByTestId(`pdf-page-${n}`);
    fireIntersection(el, true);
  }
}

beforeEach(() => {
  invokeMock.mockReset().mockResolvedValue(undefined);
  convertFileSrcMock.mockClear();
  openPathMock.mockReset().mockResolvedValue(undefined);
  resetWorktreeAssetsAllowedCacheForTests();
  getDocumentMock.mockReset();
  getDocumentMock.mockImplementation(() => ({
    promise: Promise.resolve(makeFakeDoc(3)),
    destroy: vi.fn().mockResolvedValue(undefined),
  }));
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue({ arrayBuffer: () => Promise.resolve(new ArrayBuffer(8)) }),
  );
  IntersectionObserverMock.instances = [];
  vi.stubGlobal("IntersectionObserver", IntersectionObserverMock);
  ResizeObserverMock.instances = [];
  vi.stubGlobal("ResizeObserver", ResizeObserverMock);
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("PdfViewer", () => {
  it("exposes fit modes as mutually exclusive pressed controls", async () => {
    render(() => <PdfViewer worktreeRoot="/repo/lane" path="docs/report.pdf" />);
    await screen.findByTestId("pdf-page-1");
    const width = screen.getByRole("button", { name: "Fit width" });
    const page = screen.getByRole("button", { name: "Fit page" });
    expect(width).toHaveAttribute("aria-pressed", "true");
    expect(page).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(page);
    expect(page).toHaveAttribute("aria-pressed", "true");
    expect(width).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(screen.getByRole("button", { name: "Zoom in" }));
    expect(page).toHaveAttribute("aria-pressed", "false");
    expect(width).toHaveAttribute("aria-pressed", "false");
  });

  it("renders three page slots and only the visible canvases", async () => {
    render(() => <PdfViewer worktreeRoot="/repo/lane" path="docs/report.pdf" size={2048} />);

    expect(await screen.findByTestId("pdf-page-1")).toBeInTheDocument();
    expect(screen.getByTestId("pdf-page-2")).toBeInTheDocument();
    expect(screen.getByTestId("pdf-page-3")).toBeInTheDocument();
    expect(document.querySelectorAll("canvas")).toHaveLength(0);

    fireIntersection(screen.getByTestId("pdf-page-1"), true);
    await waitFor(() => expect(document.querySelectorAll("canvas")).toHaveLength(1));

    fireIntersection(screen.getByTestId("pdf-page-2"), true);
    await waitFor(() => expect(document.querySelectorAll("canvas")).toHaveLength(2));

    // Page 1 scrolls back out of the virtualization margin: its canvas is released.
    fireIntersection(screen.getByTestId("pdf-page-1"), false);
    await waitFor(() => expect(document.querySelectorAll("canvas")).toHaveLength(1));
  });

  it("recomputes fit-width scale on a debounced resize", async () => {
    render(() => <PdfViewer worktreeRoot="/repo/lane" path="docs/report.pdf" />);
    await screen.findByTestId("pdf-page-1");

    const container = screen.getByTestId("pdf-scroll-container");
    Object.defineProperty(container, "clientWidth", { value: 632, configurable: true });
    Object.defineProperty(container, "clientHeight", { value: 900, configurable: true });

    vi.useFakeTimers();
    ResizeObserverMock.instances[0]?.trigger();
    await vi.advanceTimersByTimeAsync(150);
    vi.useRealTimers();

    // (632 - 32 gutter) / 600 page width = 1.0 -> 100%.
    await waitFor(() => expect(screen.getByTitle("Reset zoom to 100%")).toHaveTextContent("100%"));
  });

  it("navigates via the page input", async () => {
    render(() => <PdfViewer worktreeRoot="/repo/lane" path="docs/report.pdf" />);
    await screen.findByTestId("pdf-page-1");

    const input = screen.getByLabelText("Page number") as HTMLInputElement;
    fireEvent.input(input, { target: { value: "3" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => expect(screen.getByText("of 3")).toBeInTheDocument());
    expect(input.value).toBe("3");
  });

  it("changes the scale and the zoom label from the zoom buttons", async () => {
    render(() => <PdfViewer worktreeRoot="/repo/lane" path="docs/report.pdf" />);
    await screen.findByTestId("pdf-page-1");

    const zoomLabel = () => screen.getByTitle("Reset zoom to 100%");
    expect(zoomLabel()).toHaveTextContent("100%");

    fireEvent.click(screen.getByLabelText("Zoom in"));
    expect(zoomLabel()).toHaveTextContent("115%");

    fireEvent.click(screen.getByLabelText("Zoom out"));
    fireEvent.click(screen.getByLabelText("Zoom out"));
    expect(zoomLabel()).toHaveTextContent("87%");

    fireEvent.click(zoomLabel());
    expect(zoomLabel()).toHaveTextContent("100%");
  });

  it("finds matches, highlights them, and cycles with the next/previous buttons", async () => {
    render(() => <PdfViewer worktreeRoot="/repo/lane" path="docs/report.pdf" />);
    await makeAllPagesVisible();

    fireEvent.keyDown(screen.getByTestId("pdf-viewer-root"), { key: "f", metaKey: true });

    const input = await screen.findByPlaceholderText("Find in document");
    fireEvent.input(input, { target: { value: "needle" } });

    await waitFor(() => expect(screen.getByText("1 of 2")).toBeInTheDocument());
    await waitFor(() => expect(document.querySelectorAll(".pdf-find-mark")).toHaveLength(2));
    expect(document.querySelectorAll(".pdf-find-mark.is-active")).toHaveLength(1);

    fireEvent.click(screen.getByLabelText("Next match"));
    expect(screen.getByText("2 of 2")).toBeInTheDocument();

    fireEvent.click(screen.getByLabelText("Next match"));
    expect(screen.getByText("1 of 2")).toBeInTheDocument();
  });

  it("shows the primary open-externally action on the error state", async () => {
    getDocumentMock.mockImplementation(() => ({
      promise: Promise.reject(new Error("boom")),
      destroy: vi.fn().mockResolvedValue(undefined),
    }));

    render(() => <PdfViewer worktreeRoot="/repo/lane" path="docs/report.pdf" />);

    expect(await screen.findByText("Couldn't load preview")).toBeInTheDocument();
    const button = screen.getByRole("button", { name: "Open in system viewer" });
    expect(button.className).toContain("bg-signal");

    fireEvent.click(button);
    await waitFor(() => expect(openPathMock).toHaveBeenCalledWith("/repo/lane/docs/report.pdf"));
  });

  it("shows the password state without a prompt", async () => {
    const err = Object.assign(new Error("needs password"), { name: "PasswordException" });
    getDocumentMock.mockImplementation(() => ({
      promise: Promise.reject(err),
      destroy: vi.fn().mockResolvedValue(undefined),
    }));

    render(() => <PdfViewer worktreeRoot="/repo/lane" path="docs/secret.pdf" />);

    expect(await screen.findByText("This PDF is password protected")).toBeInTheDocument();
    expect(screen.queryByLabelText("Page number")).not.toBeInTheDocument();
  });

  it("ignores a stale load result after the path changes", async () => {
    const tasks = queueLoadingTasks(2);
    const [path, setPath] = createSignal("docs/old.pdf");

    render(() => <PdfViewer worktreeRoot="/repo/lane" path={path()} />);
    await waitFor(() => expect(getDocumentMock).toHaveBeenCalledTimes(1));

    setPath("docs/new.pdf");
    await waitFor(() => expect(getDocumentMock).toHaveBeenCalledTimes(2));

    const oldDoc = makeFakeDoc(1);
    const newDoc = makeFakeDoc(3);
    // The new load's document resolves first...
    tasks[1].resolve(newDoc);
    await screen.findByText("of 3");
    // ...then the stale one resolves after the fact and must be discarded, not swap the view back.
    tasks[0].resolve(oldDoc);

    await waitFor(() => expect(oldDoc.destroy).toHaveBeenCalled());
    expect(screen.getByText("of 3")).toBeInTheDocument();
  });
});
