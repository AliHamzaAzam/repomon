import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
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
  it("exposes search options and disclosure state to keyboard and assistive technology", () => {
    const editor = { selectedLane: () => null, activeFile: () => null } as unknown as EditorStore;
    render(() => <ProjectSearchPanel editor={editor} />);
    expect(screen.getByRole("textbox", { name: "Search in project" })).toBeInTheDocument();
    for (const name of ["Match case", "Use regular expression"]) {
      const toggle = screen.getByRole("button", { name });
      expect(toggle).toHaveAttribute("aria-pressed", "false");
      fireEvent.click(toggle);
      expect(toggle).toHaveAttribute("aria-pressed", "true");
    }
    for (const name of ["Replace in file", "Filter paths"]) {
      const toggle = screen.getByRole("button", { name });
      expect(toggle).toHaveAttribute("aria-expanded", "false");
      fireEvent.click(toggle);
      expect(toggle).toHaveAttribute("aria-expanded", "true");
    }
  });

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

    fireEvent.input(input, { target: { value: "first" } });

    await waitFor(() => {
      expect(searchResolvers.some((r) => r.query === "first")).toBe(true);
    });

    fireEvent.input(input, { target: { value: "second" } });

    await waitFor(() => {
      expect(searchResolvers.some((r) => r.query === "second")).toBe(true);
    });

    const firstResolver = searchResolvers.find((r) => r.query === "first")!;
    const secondResolver = searchResolvers.find((r) => r.query === "second")!;

    secondResolver.resolve({
      hits: [{ path: "second.txt", line: 2, column: 1, line_text: "second hit" }],
      truncated: false,
    });

    await waitFor(() => {
      expect(container.textContent).toContain("second.txt");
    });

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
