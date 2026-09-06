import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ExtSnapshot } from "../bindings";
import { createExtensionsStore, type ExtSource } from "../stores/extensions";
import type { FleetStore } from "../stores/fleet";
import ExtensionsView from "./ExtensionsView";

const snapshot: ExtSnapshot = {
  cli_version: "fixture", marketplaces: [],
  plugins: [{ id: "review@fixture", name: "review", marketplace: "fixture", version: null, enabled: true, enabled_source: "global", provides: null, installed: true }],
  skills: [], accounts: [{ key: "default", label: "Fixture", claude: true, agent_kind: null }], account: "default",
};

function mount() {
  const source = { list: vi.fn().mockResolvedValue(snapshot), subscribe: async () => () => undefined } as unknown as ExtSource;
  const fleet = { visibleRepos: () => [{ id: 2, name: "workspace" }] } as unknown as FleetStore;
  render(() => {
    const store = createExtensionsStore(source);
    return <ExtensionsView store={store} fleet={fleet} />;
  });
}

afterEach(cleanup);

describe("ExtensionsView", () => {
  it("announces selected account, scope and filter while changing the visible results", async () => {
    mount();
    expect(await screen.findByRole("button", { name: "Fixture" })).toHaveAttribute("aria-pressed", "true");
    const scope = screen.getByRole("button", { name: "workspace" });
    fireEvent.click(scope);
    expect(scope).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Global" })).toHaveAttribute("aria-pressed", "false");
    const filter = screen.getByRole("button", { name: "skills" });
    fireEvent.click(filter);
    expect(filter).toHaveAttribute("aria-pressed", "true");
    expect(await screen.findByText("No extensions or skills in this scope.")).toHaveAttribute("role", "status");
  });

  it("names search and install fields and explains an empty search", async () => {
    mount();
    await screen.findByRole("button", { name: "Fixture" });
    fireEvent.input(screen.getByRole("textbox", { name: "Search extensions and skills" }), { target: { value: "missing" } });
    expect(screen.getByRole("status")).toHaveTextContent("No extensions match your search.");
    const install = screen.getByRole("button", { name: "Install" });
    fireEvent.click(install);
    expect(install).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("textbox", { name: "Plugin reference" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "New Skill" }));
    expect(screen.getByRole("textbox", { name: "Skill name" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Skill description (optional)" })).toBeInTheDocument();
  });
});
