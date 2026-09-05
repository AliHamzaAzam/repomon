import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { RepomindStatus } from "../bindings";
import type { RepomindStore } from "../stores/repomind";
import { status } from "../test/repomindFixtures";
import RepomindMemory from "./RepomindMemory";

const daemonCall = vi.fn();

vi.mock("../ipc/rpc", () => ({
  daemonCall: (...args: unknown[]) => daemonCall(...args),
}));

afterEach(() => {
  cleanup();
  daemonCall.mockReset();
});

function repomindStub(value: RepomindStatus | null, extra: Partial<RepomindStore> = {}) {
  return {
    status: () => value,
    busy: () => null,
    regenerateBoot: vi.fn().mockResolvedValue(undefined),
    runExport: vi.fn().mockResolvedValue(undefined),
    ...extra,
  } as unknown as RepomindStore;
}

function entry(name: string, path: string) {
  return { name, path, is_dir: false, size: 20, ignored: false };
}

const DIGESTS: Record<string, string> = {
  "journal/2026-09-05.md": "# Journal\n\n## 09:12 spawn_agent (ok)\n\n- Repo: repomon\n",
  "journal/2026-09-03.md": "# Journal\n\n## 08:01 merge_lane (ok)\n",
  "journal/archive/2026-05.md": "# May\n\n## 05-02 the old thing\n",
};

function mockDaemon(overrides: Record<string, unknown> = {}) {
  daemonCall.mockImplementation((method: string, params?: { path?: string }) => {
    if (method in overrides) {
      const value = overrides[method];
      return value instanceof Error ? Promise.reject(value) : Promise.resolve(value);
    }
    if (method === "file.list") {
      if (params?.path === "journal/archive") {
        return Promise.resolve({
          entries: [entry("2026-05.md", "journal/archive/2026-05.md")],
          truncated: false,
        });
      }
      return Promise.resolve({
        entries: [
          entry("2026-09-03.md", "journal/2026-09-03.md"),
          entry("2026-09-05.md", "journal/2026-09-05.md"),
        ],
        truncated: false,
      });
    }
    if (method === "file.read") {
      const content = DIGESTS[params?.path ?? ""];
      return content
        ? Promise.resolve({ content, mtime_ms: 0, size: 20, truncated: false, kind: "text", large: false })
        : Promise.reject(new Error("no such file"));
    }
    return Promise.resolve(null);
  });
}

describe("the memory section", () => {
  it("reports the boot context and regenerates it on demand", async () => {
    mockDaemon();
    const regenerateBoot = vi.fn().mockResolvedValue(undefined);
    render(() => (
      <RepomindMemory
        laneId={90}
        onOpen={vi.fn()}
        repomind={repomindStub(
          status({
            boot: {
              generated_at: "2026-09-04T00:00:00Z",
              tokens_estimate: 8400,
              trimmed: ["journal/2026-09-01.md"],
            },
          }),
          { regenerateBoot },
        )}
      />
    ));

    await waitFor(() => expect(screen.getByText("8400 tokens")).toBeInTheDocument());
    expect(screen.getByText(/The token budget left out: journal\/2026-09-01\.md/)).toBeInTheDocument();
    fireEvent.click(screen.getByText("Regenerate"));
    expect(regenerateBoot).toHaveBeenCalled();
  });

  it("opens the boot document in the editor", async () => {
    mockDaemon();
    const onOpen = vi.fn();
    render(() => <RepomindMemory laneId={90} onOpen={onOpen} repomind={repomindStub(status())} />);

    await waitFor(() => expect(screen.getByTitle("Open .repomind/boot.md")).toBeInTheDocument());
    fireEvent.click(screen.getByTitle("Open .repomind/boot.md"));
    expect(onOpen).toHaveBeenCalledWith(".repomind/boot.md");
  });

  it("shows the export state, its error, and runs one on demand", async () => {
    mockDaemon();
    const runExport = vi.fn().mockResolvedValue(undefined);
    render(() => (
      <RepomindMemory
        laneId={90}
        onOpen={vi.fn()}
        repomind={repomindStub(
          status({ export: { last_run: null, pending: true, last_error: "home is not a git repo" } }),
          { runExport },
        )}
      />
    ));

    await waitFor(() => expect(screen.getByText(/Export ran never/)).toBeInTheDocument());
    expect(screen.getByText("one pending")).toBeInTheDocument();
    expect(screen.getByText("home is not a git repo")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Export now"));
    expect(runExport).toHaveBeenCalled();
  });

  it("opens on the newest day and shows that day's entries", async () => {
    mockDaemon();
    render(() => <RepomindMemory laneId={90} onOpen={vi.fn()} repomind={repomindStub(status())} />);

    await waitFor(() => expect(screen.getByText(/spawn_agent \(ok\)/)).toBeInTheDocument());
    // The digest's own title heading is not an entry.
    expect(screen.queryByText(/^# Journal$/)).toBeNull();
    expect(screen.getByLabelText("Journal day")).toHaveTextContent("2026-09-05");
  });

  it("keeps archived months behind a disclosure, and offers them once opened", async () => {
    mockDaemon();
    render(() => <RepomindMemory laneId={90} onOpen={vi.fn()} repomind={repomindStub(status())} />);

    await waitFor(() => expect(screen.getByText("1 archived month")).toBeInTheDocument());
    const disclosure = screen.getByText("1 archived month").closest("button");
    expect(disclosure).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(disclosure as HTMLButtonElement);
    expect(disclosure).toHaveAttribute("aria-expanded", "true");
  });

  it("opens the selected day in the editor", async () => {
    mockDaemon();
    const onOpen = vi.fn();
    render(() => <RepomindMemory laneId={90} onOpen={onOpen} repomind={repomindStub(status())} />);

    await waitFor(() =>
      expect(screen.getByTitle("Open journal/2026-09-05.md")).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByTitle("Open journal/2026-09-05.md"));
    expect(onOpen).toHaveBeenCalledWith("journal/2026-09-05.md");
  });

  it("says the journal has not started rather than showing an empty list", async () => {
    mockDaemon({ "file.list": { entries: [], truncated: false } });
    render(() => <RepomindMemory laneId={90} onOpen={vi.fn()} repomind={repomindStub(status())} />);

    await waitFor(() => expect(screen.getByText(/No journal yet/)).toBeInTheDocument());
  });

  it("opens the daemon's activity journal from the memory section", async () => {
    mockDaemon({
      "journal.query": {
        entries: [
          {
            action: "merge_lane",
            outcome: "ok",
            at: "2026-09-05T10:00:00Z",
            repo: "repomon",
            lane_id: 1,
            params: "lane 1",
            detail: "merged cleanly",
          },
        ],
      },
    });
    render(() => <RepomindMemory laneId={90} onOpen={vi.fn()} repomind={repomindStub(status())} />);

    await waitFor(() => expect(screen.getByText("Activity")).toBeInTheDocument());
    expect(screen.queryByText("merge_lane")).toBeNull();

    fireEvent.click(screen.getByText("Activity"));
    await waitFor(() => expect(screen.getByText(/merge_lane/)).toBeInTheDocument());
    expect(screen.getByText("Activity journal")).toBeInTheDocument();
  });
});
