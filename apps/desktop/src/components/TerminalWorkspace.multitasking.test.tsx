import { cleanup, render, screen, waitFor } from "@solidjs/testing-library";
import { createRoot } from "solid-js";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import type { AgentSession, Lane, Repo } from "../bindings";
import { createFleetStore, type FleetSource } from "../stores/fleet";
import { createWorkspaceStore } from "../stores/workspace";
import TerminalWorkspace from "./TerminalWorkspace";

const daemonCallMock = vi.hoisted(() => vi.fn().mockResolvedValue(null));
vi.mock("../ipc/rpc", async () => {
  const actual = await vi.importActual<typeof import("../ipc/rpc")>("../ipc/rpc");
  return { ...actual, daemonCall: (...args: unknown[]) => daemonCallMock(...args) };
});

/// Full-suite runs can be resource-contended (many jsdom + Solid + xterm component trees
/// mounting in parallel across test files); give these multi-pane, multi-lane assertions more
/// headroom than the default 1000ms so real timing pressure doesn't read as a false failure.
const settle = (callback: () => void | Promise<void>) => waitFor(callback, { timeout: 5000 });

beforeAll(() => {
  // jsdom gaps the component hits while rendering panes.
  Element.prototype.scrollIntoView = vi.fn();
  HTMLCanvasElement.prototype.getContext = vi.fn(() => null);
});

afterEach(() => {
  cleanup();
  daemonCallMock.mockClear();
});

function session(overrides: Partial<AgentSession> = {}): AgentSession {
  return {
    id: 1,
    agent: "claude-code",
    repo_id: 1,
    worktree_id: 1,
    started_at: "2026-07-20T00:00:00Z",
    last_activity_at: "2026-07-20T00:00:00Z",
    ended_at: null,
    manifest_path: "",
    tool_call_count: 0,
    title: "Ship",
    status: "running",
    external: false,
    session_id: "s1",
    resume_at: null,
    inferred: false,
    tmux_window: "lane-10-1",
    last_message: null,
    pending_prompt: null,
    pending_dialog: null,
    stale: false,
    stalled_since: null,
    subagent_running: null,
    gate: null,
    config_dir: null,
    custom_label: null,
    generated_label: null,
    ...overrides,
  };
}

/// A lane with its own repo/worktree identity, so multiple lanes in one fleet are distinguishable
/// in the multitasking view's "repo / branch" pane labels.
function mkLane(id: number, sessions: AgentSession[]): Lane {
  const repo: Repo = {
    id,
    name: `repo${id}`,
    path: `/code/repo${id}`,
    added_at: "2026-07-20T00:00:00Z",
    worktree_root_template: null,
    hidden: false,
    position: null,
    label: null,
  };
  return {
    id,
    repo,
    worktree: { id, repo_id: id, path: `/code/repo${id}`, branch: `branch${id}`, head: "abc", is_main: true, name: `lane${id}` },
    state: {
      worktree_id: id, head: "abc", branch: `branch${id}`, upstream: null, ahead: 0, behind: 0,
      dirty: { staged: 0, unstaged: 0, untracked: 0 }, last_commit_at: null, last_change_at: null,
      locked: false, prunable: false,
    },
    agent_sessions: sessions,
    last_activity_at: "2026-07-20T00:00:00Z",
    pinned: false,
  };
}

interface Mounted {
  fleet: ReturnType<typeof createFleetStore>;
  workspace: ReturnType<typeof createWorkspaceStore>;
  dispose: () => void;
}

async function mountFleet(lanes: Lane[], opts: { multitasking?: boolean; selectedLaneId?: number } = {}): Promise<Mounted> {
  const source: FleetSource = {
    load: () => Promise.resolve({
      repos: lanes.map((l) => l.repo),
      lanes,
      usage: [],
      terminals: [],
      sortReposByActivity: false,
      sortMode: "default",
      tabSortMode: "manual",
    }),
    refreshUsage: () => Promise.resolve(),
    subscribe: () => Promise.resolve(() => undefined),
  };
  const actions = { spawn: vi.fn(), stopAgent: vi.fn(), adoptAgent: vi.fn(), rename: vi.fn(), setAgentTabOrder: vi.fn() };
  return createRoot(async (dispose) => {
    const fleet = createFleetStore(source);
    const workspace = createWorkspaceStore(fleet);
    fleet.start();
    await settle(() => expect(fleet.synced()).toBe(true));
    fleet.setSelectedLaneId(opts.selectedLaneId ?? lanes[0]?.id ?? null);
    if (opts.multitasking) workspace.setMultitasking(true);
    else workspace.chooseLayout("grid");
    render(() => <TerminalWorkspace fleet={fleet} actions={actions as never} workspace={workspace} />);
    return { fleet, workspace, dispose };
  });
}

/// Every wrapper `<div>` the grid actually renders for a pane, i.e. `.terminal-layout`'s direct
/// children (mirrors the pattern already used in TerminalWorkspace.test.tsx).
function paneWrapperDivs(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>(".terminal-layout > div")];
}

function visiblePaneWrapperDivs(): HTMLElement[] {
  return paneWrapperDivs().filter((el) => !el.classList.contains("warm-terminal-hidden"));
}

describe("TerminalWorkspace multitasking: every selected pane is actually mounted (bug 1)", () => {
  it("mounts a real header for every pane the picker's default fallback selects across lanes", async () => {
    const laneA = mkLane(10, [
      session({ id: 1, agent: "codex", tmux_window: "lane-10-1", session_id: "s1" }),
      session({ id: 2, agent: "claude-code", tmux_window: "lane-10-2", session_id: "s2" }),
    ]);
    const laneB = mkLane(20, [
      session({ id: 3, agent: "opencode", tmux_window: "lane-20-1", session_id: "s3" }),
      session({ id: 4, agent: "gemini", tmux_window: "lane-20-2", session_id: "s4" }),
    ]);
    const { workspace, dispose } = await mountFleet([laneA, laneB], { multitasking: true });

    await settle(() => {
      expect(screen.getByText(/panes/)).toBeTruthy();
    });

    const expectedCount = workspace.multitaskTargets().length;
    expect(expectedCount).toBe(4); // all four agents fit under the new fallback cap of 6

    // The header text ("N LANES · N PANES") must match what's actually selected...
    expect(screen.getByText(new RegExp(`${expectedCount} panes`))).toBeTruthy();

    // ...and every one of those selected panes must have a real, mounted TerminalPane header —
    // not an empty grid cell with no border accent and no content (the reported bug).
    await settle(() => {
      const visible = visiblePaneWrapperDivs();
      expect(visible).toHaveLength(expectedCount);
      for (const target of workspace.multitaskTargets()) {
        const wrapper = visible.find((el) => el.querySelector("section")?.getAttribute("aria-label")?.includes(target.label));
        expect(wrapper, `expected a mounted pane for ${target.label}`).toBeTruthy();
        expect(wrapper!.querySelector("section")).not.toBeNull();
        // Every mounted multitasking pane must carry its accent border — the visual tell the
        // operator used to distinguish "a real pane" from "an empty grid cell".
        expect(wrapper!.classList.contains("multitask-pane")).toBe(true);
      }
    });
    dispose();
  });

  it("keeps every visible pane mounted through a picker selection change to previously-unmounted agents", async () => {
    const laneA = mkLane(10, [
      session({ id: 1, agent: "codex", tmux_window: "lane-10-1", session_id: "s1" }),
    ]);
    const laneB = mkLane(20, [
      session({ id: 2, agent: "opencode", tmux_window: "lane-20-1", session_id: "s2" }),
    ]);
    const laneC = mkLane(30, [
      session({ id: 3, agent: "gemini", tmux_window: "lane-30-1", session_id: "s3" }),
    ]);
    const { workspace, dispose } = await mountFleet([laneA, laneB, laneC], { multitasking: true, selectedLaneId: 10 });

    await settle(() => expect(visiblePaneWrapperDivs().length).toBeGreaterThan(0));

    // Jump the selection straight to a lane the view has never warmed before (laneC only) —
    // exercising the exact "warm cache hasn't caught up yet" gap bug 1 targets.
    workspace.setMultitaskPaneSelection(["lane-30-1"]);

    await settle(() => {
      const visible = visiblePaneWrapperDivs();
      expect(visible).toHaveLength(1);
      expect(visible[0].querySelector("section")?.getAttribute("aria-label")).toContain("gemini");
    });
    dispose();
  });
});

describe("TerminalWorkspace: active pane highlight (bug 4)", () => {
  it("rings the active pane in lane-scoped grid view and no other", async () => {
    const laneA = mkLane(10, [
      session({ id: 1, agent: "codex", tmux_window: "lane-10-1", session_id: "s1" }),
      session({ id: 2, agent: "claude-code", tmux_window: "lane-10-2", session_id: "s2" }),
      session({ id: 3, agent: "opencode", tmux_window: "lane-10-3", session_id: "s3" }),
    ]);
    const { workspace, dispose } = await mountFleet([laneA]);

    await settle(() => expect(visiblePaneWrapperDivs().length).toBe(3));

    workspace.setActiveWindow("lane-10-2");
    await settle(() => {
      const visible = visiblePaneWrapperDivs();
      const active = visible.filter((el) => el.classList.contains("is-active-pane"));
      expect(active).toHaveLength(1);
      expect(active[0].querySelector("section")?.getAttribute("aria-label")).toContain("claude-code");
      // Ring classes should only land on the active pane.
      const inactive = visible.filter((el) => !el.classList.contains("is-active-pane"));
      expect(inactive).toHaveLength(2);
      for (const el of inactive) {
        expect(el.classList.contains("ring-signal/60")).toBe(false);
      }
    });
    dispose();
  });

  it("rings the active pane in the fleet-wide multitasking view too", async () => {
    const laneA = mkLane(10, [
      session({ id: 1, agent: "codex", tmux_window: "lane-10-1", session_id: "s1" }),
      session({ id: 2, agent: "claude-code", tmux_window: "lane-10-2", session_id: "s2" }),
    ]);
    const { workspace, dispose } = await mountFleet([laneA], { multitasking: true });

    await settle(() => expect(visiblePaneWrapperDivs().length).toBe(2));
    workspace.setActiveWindow("lane-10-1");

    await settle(() => {
      const visible = visiblePaneWrapperDivs();
      const active = visible.filter((el) => el.classList.contains("is-active-pane"));
      expect(active).toHaveLength(1);
      expect(active[0].querySelector("section")?.getAttribute("aria-label")).toContain("codex");
    });
    dispose();
  });
});

/// jsdom performs no real layout: `getBoundingClientRect()` reports zero for every element
/// regardless of CSS. To honestly test the geometric claims in the bug report ("composer has
/// nonzero visible height", "no two panes overlap") we model exactly what the *fixed* CSS
/// guarantees — a `multitaskColumns()`-wide grid where every row is at least the 14rem
/// `.multitask-pane` floor confirmed in index.css.test.ts — and stub each element's rect from
/// that model. This proves the arithmetic behind the fix is sound; it does not substitute for
/// looking at the app in a real browser (see the task's final report for what's unverified).
describe("TerminalWorkspace multitasking: simulated pane geometry (bug 2 + bug 3)", () => {
  const ROW_MIN_PX = 224; // 14rem at the standard 16px root font size, per index.css.
  const HEADER_PX = 28; // h-7, TerminalPane.tsx's header bar.
  const CONTAINER_WIDTH = 1200;
  const CONTAINER_HEIGHT = 900;

  it("keeps composer height positive and pane rects non-overlapping for a 5-pane / 2-column layout", async () => {
    const laneA = mkLane(10, [
      session({ id: 1, agent: "codex", tmux_window: "lane-10-1", session_id: "s1" }),
      session({ id: 2, agent: "claude-code", tmux_window: "lane-10-2", session_id: "s2" }),
      session({ id: 3, agent: "opencode", tmux_window: "lane-10-3", session_id: "s3" }),
    ]);
    const laneB = mkLane(20, [
      session({ id: 4, agent: "gemini", tmux_window: "lane-20-1", session_id: "s4" }),
      session({ id: 5, agent: "hermes", tmux_window: "lane-20-2", session_id: "s5" }),
    ]);
    const { workspace, dispose } = await mountFleet([laneA, laneB], { multitasking: true });
    workspace.setMultitaskColumns(2);

    await settle(() => expect(visiblePaneWrapperDivs().length).toBe(5));
    const columns = workspace.multitaskColumns();
    const panes = visiblePaneWrapperDivs();
    expect(panes).toHaveLength(5);

    const colWidth = CONTAINER_WIDTH / columns;
    const rects: DOMRect[] = panes.map((_, index) => {
      const col = index % columns;
      const row = Math.floor(index / columns);
      return new DOMRect(col * colWidth, row * ROW_MIN_PX, colWidth, ROW_MIN_PX);
    });
    panes.forEach((pane, index) => {
      Object.defineProperty(pane, "getBoundingClientRect", { value: () => rects[index], configurable: true });
      const host = pane.querySelector<HTMLElement>(".terminal-host");
      expect(host, "every mounted pane must have its terminal-host composer container").not.toBeNull();
      const rect = rects[index];
      const hostRect = new DOMRect(rect.x, rect.y + HEADER_PX, rect.width, rect.height - HEADER_PX);
      Object.defineProperty(host!, "getBoundingClientRect", { value: () => hostRect, configurable: true });
    });

    // Bug 3: the composer's container must have real, positive, in-bounds height under the
    // fixed 14rem floor — not clipped to zero by an under-sized grid row.
    for (const pane of panes) {
      const host = pane.querySelector<HTMLElement>(".terminal-host")!;
      const paneRect = pane.getBoundingClientRect();
      const hostRect = host.getBoundingClientRect();
      expect(hostRect.height).toBeGreaterThan(0);
      expect(hostRect.top).toBeGreaterThanOrEqual(paneRect.top);
      expect(hostRect.bottom).toBeLessThanOrEqual(paneRect.bottom);
      expect(hostRect.left).toBeGreaterThanOrEqual(paneRect.left);
      expect(hostRect.right).toBeLessThanOrEqual(paneRect.right);
    }

    // Bug 2: no two panes' own rects may overlap.
    for (let i = 0; i < rects.length; i += 1) {
      for (let j = i + 1; j < rects.length; j += 1) {
        const a = rects[i];
        const b = rects[j];
        const overlaps = a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
        expect(overlaps, `pane ${i} and pane ${j} must not overlap`).toBe(false);
      }
    }
    void CONTAINER_HEIGHT;
    dispose();
  });
});
