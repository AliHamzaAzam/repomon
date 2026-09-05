import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import RepomindDuties from "./RepomindDuties";

const daemonCall = vi.fn();

vi.mock("../ipc/rpc", () => ({
  daemonCall: (...args: unknown[]) => daemonCall(...args),
}));

afterEach(() => {
  cleanup();
  daemonCall.mockReset();
});

const NIGHTLY = {
  id: 4,
  spec: "daily 09:00",
  prompt: "sweep every lane for stalled agents",
  max_actions: 6,
  created_at: "2026-09-01T00:00:00Z",
  last_run_at: "2026-09-05T09:00:00Z",
  next_run: "2026-09-06T09:00:00Z",
};

function mockDaemon(schedules: unknown[], overrides: Record<string, unknown> = {}) {
  daemonCall.mockImplementation((method: string) => {
    if (method in overrides) {
      const value = overrides[method];
      return value instanceof Error ? Promise.reject(value) : Promise.resolve(value);
    }
    if (method === "schedule.list") return Promise.resolve({ schedules });
    return Promise.resolve(null);
  });
}

describe("the standing duties section", () => {
  it("states the spec, the goal, the cap and both run times", async () => {
    mockDaemon([NIGHTLY]);
    render(() => <RepomindDuties />);

    await waitFor(() => expect(screen.getByText("daily 09:00")).toBeInTheDocument());
    expect(screen.getByText("sweep every lane for stalled agents")).toBeInTheDocument();
    expect(screen.getByText(/cap 6/)).toBeInTheDocument();
    expect(screen.getByText(/ran /)).toBeInTheDocument();
    expect(screen.getByText(/next /)).toBeInTheDocument();
  });

  it("asks before removing a duty, and removes it once confirmed", async () => {
    mockDaemon([NIGHTLY]);
    render(() => <RepomindDuties />);

    await waitFor(() => expect(screen.getByText("Remove")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Remove"));
    expect(daemonCall).not.toHaveBeenCalledWith("schedule.remove", expect.anything());

    fireEvent.click(screen.getByText("Confirm"));
    await waitFor(() => expect(daemonCall).toHaveBeenCalledWith("schedule.remove", { id: 4 }));
  });

  it("lets a confirmation be backed out of", async () => {
    mockDaemon([NIGHTLY]);
    render(() => <RepomindDuties />);

    await waitFor(() => expect(screen.getByText("Remove")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Remove"));
    fireEvent.click(screen.getByText("Keep"));

    await waitFor(() => expect(screen.getByText("Remove")).toBeInTheDocument());
    expect(daemonCall).not.toHaveBeenCalledWith("schedule.remove", expect.anything());
  });

  it("adds a duty from the section itself, without a trip through settings", async () => {
    mockDaemon([]);
    render(() => <RepomindDuties />);

    await waitFor(() => expect(screen.getByText("Add")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Add"));

    const form = screen.getByRole("form", { name: "Add a standing duty" });
    expect(form).toBeInTheDocument();
    expect(screen.getByText("Add duty")).toBeDisabled();

    fireEvent.input(screen.getByLabelText("Schedule"), { target: { value: "weekdays 09:00" } });
    fireEvent.input(screen.getByLabelText("Goal"), { target: { value: "Brief the fleet" } });
    fireEvent.input(screen.getByLabelText("Action cap"), { target: { value: "12" } });
    fireEvent.submit(form);

    await waitFor(() =>
      expect(daemonCall).toHaveBeenCalledWith("schedule.add", {
        spec: "weekdays 09:00",
        prompt: "Brief the fleet",
        max_actions: 12,
      }),
    );
    await waitFor(() =>
      expect(screen.queryByRole("form", { name: "Add a standing duty" })).toBeNull(),
    );
  });

  it("will not add a duty with no schedule or no goal", async () => {
    mockDaemon([]);
    render(() => <RepomindDuties />);

    await waitFor(() => expect(screen.getByText("Add")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Add"));
    fireEvent.input(screen.getByLabelText("Schedule"), { target: { value: "daily 09:00" } });
    fireEvent.submit(screen.getByRole("form", { name: "Add a standing duty" }));

    expect(daemonCall).not.toHaveBeenCalledWith("schedule.add", expect.anything());
  });
});
