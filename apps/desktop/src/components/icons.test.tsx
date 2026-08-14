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

  it("resolves official brand marks as defaults for Claude, Antigravity, Codex, OpenCode", () => {
    expect(resolveAgentIconKey("claude-code")).toBe("brand-claude");
    expect(resolveAgentIconKey("claude")).toBe("brand-claude");
    expect(resolveAgentIconKey("antigravity")).toBe("brand-antigravity");
    expect(resolveAgentIconKey("agy")).toBe("brand-antigravity");
    expect(resolveAgentIconKey("codex")).toBe("brand-openai");
    expect(resolveAgentIconKey("opencode")).toBe("brand-opencode");

    // Cursor and Aider keep their abstract defaults
    expect(resolveAgentIconKey("cursor")).toBe("cursor");
    expect(resolveAgentIconKey("aider")).toBe("binary-orbit");
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
      "claude-code": "sparkle",
    });

    expect(agentIconOverrides()["custom-cli-tool"]).toBe("bolt");
    expect(resolveAgentIconKey("custom-cli-tool")).toBe("bolt");
    expect(resolveAgentIconKey("data-analyzer")).toBe("radar");
    expect(resolveAgentIconKey("codex")).toBe("compass");
    expect(resolveAgentIconKey("claude-code")).toBe("sparkle");
    // Non-overridden brand remains default
    expect(resolveAgentIconKey("antigravity")).toBe("brand-antigravity");
  });

  it("renders AgentIcon with brand marks, custom key or shell", () => {
    const { container: c1 } = render(() => <AgentIcon shell />);
    expect(c1.querySelector("svg")).toBeTruthy();

    const { container: c2 } = render(() => <AgentIcon agent="claude-code" />);
    expect(c2.querySelector("svg")).toBeTruthy();

    const { container: c3 } = render(() => <AgentIcon agent="antigravity" />);
    expect(c3.querySelector("svg")).toBeTruthy();

    const { container: c4 } = render(() => <AgentIcon agent="codex" />);
    expect(c4.querySelector("svg")).toBeTruthy();

    const { container: c5 } = render(() => <AgentIcon agent="opencode" />);
    expect(c5.querySelector("svg")).toBeTruthy();

    const { container: c6 } = render(() => <AgentIcon agent="my-bot" iconKey="brand-claude" />);
    expect(c6.querySelector("svg")).toBeTruthy();
  });
});
