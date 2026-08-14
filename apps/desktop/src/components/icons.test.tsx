import { cleanup, render } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";

import {
  AGENT_ICON_CATALOG,
  AgentIcon,
  agentIconOverrides,
  resolveAgentIconKey,
  setAgentIconOverrides,
} from "./icons";

describe("Agent icon resolution and catalog", () => {
  afterEach(() => {
    cleanup();
    setAgentIconOverrides({});
  });

  it("provides 20+ unique curated icons in catalog", () => {
    expect(AGENT_ICON_CATALOG.length).toBeGreaterThanOrEqual(20);
    const ids = new Set(AGENT_ICON_CATALOG.map((c) => c.id));
    expect(ids.size).toBe(AGENT_ICON_CATALOG.length);
    for (const entry of AGENT_ICON_CATALOG) {
      expect(entry.id).toBeTruthy();
      expect(entry.label).toBeTruthy();
      expect(typeof entry.Icon).toBe("function");
    }
  });

  it("resolves default icons for known built-in agents", () => {
    expect(resolveAgentIconKey("claude-code")).toBe("sparkle");
    expect(resolveAgentIconKey("claude")).toBe("sparkle");
    expect(resolveAgentIconKey("cursor")).toBe("cursor");
    expect(resolveAgentIconKey("aider")).toBe("binary-orbit");
    expect(resolveAgentIconKey("codex")).toBe("code-brackets");
    expect(resolveAgentIconKey("antigravity")).toBe("antigravity");
    expect(resolveAgentIconKey("agy")).toBe("antigravity");
    expect(resolveAgentIconKey("opencode")).toBe("brackets");
  });

  it("falls back to bot for unknown custom agents without overrides", () => {
    expect(resolveAgentIconKey("my-unknown-agent")).toBe("bot");
    expect(resolveAgentIconKey("custom-cli-tool")).toBe("bot");
    expect(resolveAgentIconKey("")).toBe("bot");
    expect(resolveAgentIconKey(null)).toBe("bot");
  });

  it("respects dynamic icon overrides for custom and built-in agents", () => {
    setAgentIconOverrides({
      "custom-cli-tool": "bolt",
      "data-analyzer": "radar",
      codex: "compass",
    });

    expect(agentIconOverrides()["custom-cli-tool"]).toBe("bolt");
    expect(resolveAgentIconKey("custom-cli-tool")).toBe("bolt");
    expect(resolveAgentIconKey("data-analyzer")).toBe("radar");
    expect(resolveAgentIconKey("codex")).toBe("compass");
    // Other built-ins remain default
    expect(resolveAgentIconKey("claude-code")).toBe("sparkle");
  });

  it("renders AgentIcon with custom key or shell", () => {
    const { container: c1 } = render(() => <AgentIcon shell />);
    expect(c1.querySelector("svg")).toBeTruthy();

    const { container: c2 } = render(() => <AgentIcon agent="codex" />);
    expect(c2.querySelector("svg")).toBeTruthy();

    const { container: c3 } = render(() => <AgentIcon agent="my-bot" iconKey="shield" />);
    expect(c3.querySelector("svg")).toBeTruthy();
  });
});
