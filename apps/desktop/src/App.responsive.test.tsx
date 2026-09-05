import { cleanup, render, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";

import App from "./App";
import type { ConnectionSnapshot, ConnectionSource } from "./ipc/connection";

// jsdom has no layout engine and does not evaluate media queries, so a width cannot be "set" and
// observed here. What can be asserted is the structural contract the medium breakpoint relies on:
// every toolbar word is wrapped as a `.toolbar-label` (the one thing the stylesheet hides), and
// every button carries an accessible name that does not depend on that word being visible.

function sourceFor(snapshot: ConnectionSnapshot): ConnectionSource {
  return {
    current: async () => snapshot,
    subscribe: async () => () => undefined,
  };
}

const starting: ConnectionSnapshot = {
  phase: "starting",
  endpoint: "Resolving local daemon endpoint",
  message: null,
  hint: null,
  log_path: null,
  daemon: null,
};

const TOOLBAR = ["Git", "Editor", "Usage", "Control", "Multitasking", "Extensions", "Supervision", "Repomail", "Repomind"];

afterEach(cleanup);

describe("header toolbar at the medium breakpoint", () => {
  it("wraps every toolbar word as a collapsible label inside one toolbar", () => {
    const { container } = render(() => <App connectionSource={sourceFor(starting)} />);
    const toolbar = within(container).getByRole("toolbar", { name: "Panels" });
    expect(toolbar.className).toContain("header-toolbar");

    const labels = [...toolbar.querySelectorAll(".toolbar-label")].map((el) => el.textContent);
    expect(labels).toEqual(TOOLBAR);
  });

  it("names every toolbar button independently of its visible label", () => {
    const { container } = render(() => <App connectionSource={sourceFor(starting)} />);
    const toolbar = within(container).getByRole("toolbar", { name: "Panels" });
    for (const name of TOOLBAR) {
      const button = within(toolbar).getByRole("button", { name: name === "Control" ? "Command Palette" : name });
      expect(button.getAttribute("aria-label")).toBeTruthy();
      expect(button.getAttribute("title")).toBeTruthy();
    }
  });

  it("keeps the toolbar itself shrinkable so the lockup and the settings button never overlap", () => {
    const { container } = render(() => <App connectionSource={sourceFor(starting)} />);
    const toolbar = within(container).getByRole("toolbar", { name: "Panels" });
    expect(toolbar.className).toContain("min-w-0");
  });
});
