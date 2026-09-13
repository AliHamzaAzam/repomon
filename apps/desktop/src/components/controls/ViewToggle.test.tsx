import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";

import type { AgentView } from "../../stores/agentViews";
import ViewToggle from "./ViewToggle";

afterEach(() => {
  cleanup();
});

describe("ViewToggle liquid switch", () => {
  it("moves both blobs across a full segment when the operator picks the other view", () => {
    const [view, setView] = createSignal<AgentView>("terminal");
    const { container } = render(() => <ViewToggle value={view()} onChange={setView} />);
    const group = screen.getByRole("group", { name: "Agent view" });

    expect(group.style.getPropertyValue("--view-toggle-travel")).toBe("0%");
    fireEvent.click(screen.getByRole("button", { name: "Chat" }));
    expect(view()).toBe("conversation");
    expect(group.style.getPropertyValue("--view-toggle-travel")).toBe("100%");
    // One travel variable drives both, so the follower can never be left behind at rest.
    expect(container.querySelectorAll(".view-toggle__blob")).toHaveLength(2);
    expect(container.querySelectorAll(".view-toggle__blob.is-follower")).toHaveLength(1);
  });

  it("keeps the blobs out of the accessibility tree and the labels out of the gooey layer", () => {
    const { container } = render(() => <ViewToggle value="conversation" onChange={() => {}} />);

    const liquid = container.querySelector(".view-toggle__liquid");
    expect(liquid).toHaveAttribute("aria-hidden", "true");
    expect(liquid?.querySelector("button")).toBeNull();
    expect(screen.getByRole("button", { name: "Chat" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Terminal" })).toHaveAttribute("aria-pressed", "false");
  });

  it("does not fire onChange from a disabled segment", () => {
    let calls = 0;
    render(() => <ViewToggle value="terminal" disabled label="codex default view" onChange={() => { calls += 1; }} />);

    const chat = screen.getByRole("button", { name: "Chat" });
    expect(screen.getByRole("group", { name: "codex default view" })).toBeInTheDocument();
    expect(chat).toBeDisabled();
    fireEvent.click(chat);
    expect(calls).toBe(0);
  });
});
