import { createRoot } from "solid-js";
import { describe, expect, it, vi } from "vitest";

import type { ExtSnapshot } from "../bindings";
import { createExtensionsStore, type ExtSource } from "./extensions";

const snapshot: ExtSnapshot = {
  cli_version: null,
  marketplaces: [{ name: "official", kind: "github", reference: "a/b", last_updated: null }],
  plugins: [
    { id: "superpowers@official", name: "superpowers", marketplace: "official", version: "6.1.1", enabled: true, enabled_source: "global", provides: null, installed: true },
    { id: "github@official", name: "github", marketplace: "official", version: null, enabled: false, enabled_source: "default", provides: null, installed: true },
  ],
  skills: [{ name: "verify", description: "checks things", source: "project", path: "/r/.claude/skills/verify" }],
};

function source(overrides: Partial<ExtSource> = {}): ExtSource {
  return {
    list: vi.fn().mockResolvedValue(snapshot),
    setEnabled: vi.fn().mockResolvedValue({ ok: true, fanout: null }),
    ...overrides,
  };
}

async function flush() {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe("extensions store", () => {
  it("loads a snapshot and exposes unified filtered rows", async () => {
    await createRoot(async (dispose) => {
      const store = createExtensionsStore(source());
      await flush();
      expect(store.rows().length).toBe(3); // 2 plugins + 1 skill, marketplaces excluded from rows
      store.setQuery("verify");
      expect(store.rows().length).toBe(1);
      expect(store.rows()[0].kind).toBe("skill");
      store.setQuery("");
      store.setFilter("plugins");
      expect(store.rows().every((r) => r.kind === "plugin")).toBe(true);
      dispose();
    });
  });

  it("toggling calls the daemon with the active scope and refreshes", async () => {
    await createRoot(async (dispose) => {
      const src = source();
      const store = createExtensionsStore(src);
      store.setScope({ scope: "repo", repo_id: 7 });
      await flush();
      await store.setEnabled("github@official", true);
      expect(src.setEnabled).toHaveBeenCalledWith("github@official", true, { scope: "repo", repo_id: 7 });
      expect(src.list).toHaveBeenCalledTimes(3); // initial + scope change + post-toggle refresh
      dispose();
    });
  });

  it("surfaces toggle failures without wedging busy", async () => {
    await createRoot(async (dispose) => {
      const src = source({ setEnabled: vi.fn().mockRejectedValue(new Error("nope")) });
      const store = createExtensionsStore(src);
      await flush();
      await store.setEnabled("github@official", true);
      expect(store.error()).toContain("nope");
      expect(store.busy()).toBe(false);
      dispose();
    });
  });
});
