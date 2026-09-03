import { cleanup, fireEvent, render, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import ProjectSearchPanel from "./ProjectSearchPanel";
import type { EditorStore } from "../stores/editor";

afterEach(cleanup);

let searchResolvers: Array<{ query: string; resolve: (res: any) => void }> = [];

vi.mock("../ipc/rpc", () => ({
  daemonCall: vi.fn((method: string, params: any) => {
    if (method === "file.search") {
      return new Promise((resolve) => {
        searchResolvers.push({ query: params.query, resolve });
      });
    }
    return Promise.resolve({});
  }),
}));

describe("ProjectSearchPanel request guarding", () => {
  it("two searches resolving out of order; the newer query's hits win", async () => {
    searchResolvers = [];

    const mockEditor = {
      selectedLane: () => ({ id: 1, name: "lane-1", repo: "test", worktree: { path: "/tmp" } }),
      currentLaneId: () => 1,
      activeFile: () => null,
      openAt: vi.fn(),
    } as unknown as EditorStore;

    const { container } = render(() => (
      <ProjectSearchPanel editor={mockEditor} compact={false} />
    ));

    const input = container.querySelector("input[placeholder='Search in project...']") as HTMLInputElement;
    expect(input).toBeTruthy();

    // Type first query
    fireEvent.input(input, { target: { value: "first" } });

    // Wait for debounced search to trigger
    await waitFor(() => {
      expect(searchResolvers.some((r) => r.query === "first")).toBe(true);
    });

    // Type second query
    fireEvent.input(input, { target: { value: "second" } });

    // Wait for debounced search to trigger for second query
    await waitFor(() => {
      expect(searchResolvers.some((r) => r.query === "second")).toBe(true);
    });

    const firstResolver = searchResolvers.find((r) => r.query === "first")!;
    const secondResolver = searchResolvers.find((r) => r.query === "second")!;

    // Resolve second query first
    secondResolver.resolve({
      hits: [{ path: "second.txt", line: 2, column: 1, line_text: "second hit" }],
      truncated: false,
    });

    await waitFor(() => {
      expect(container.textContent).toContain("second.txt");
    });

    // Now resolve first query later
    firstResolver.resolve({
      hits: [{ path: "first.txt", line: 1, column: 1, line_text: "first hit" }],
      truncated: false,
    });

    // Wait a tick; the first query's stale result must NOT overwrite the second query
    await new Promise((r) => setTimeout(r, 50));
    expect(container.textContent).toContain("second.txt");
    expect(container.textContent).not.toContain("first.txt");
  });
});
