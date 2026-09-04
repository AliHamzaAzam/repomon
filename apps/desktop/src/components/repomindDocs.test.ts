import { describe, expect, it } from "vitest";

import { journalPathFor, journalTail, planSlug, readActivePlan } from "./repomindDocs";

describe("readActivePlan", () => {
  it("reads the title and next step out of frontmatter, the way the daemon writes plans", () => {
    const plan = readActivePlan(
      "plans/active/ship-r3.md",
      "---\ntitle: Ship R3\nstatus: in flight\nowner: lane-1/1\n---\n\nNext step: land the boot document\n",
    );
    expect(plan.title).toBe("Ship R3");
    expect(plan.nextStep).toBe("land the boot document");
  });

  it("falls back to the first heading, then to the file name", () => {
    expect(readActivePlan("plans/active/ship.md", "# Ship the panel\n\nbody\n").title).toBe("Ship the panel");
    expect(readActivePlan("plans/active/ship.md", "just prose\n").title).toBe("ship");
  });

  it("takes the next step from a Next step section when there is no frontmatter field", () => {
    const plan = readActivePlan(
      "plans/active/ship.md",
      "# Ship\n\n## Next step\n\n- ask the operator to approve the draft\n",
    );
    expect(plan.nextStep).toBe("ask the operator to approve the draft");
  });

  it("says nothing rather than guessing when the plan has no next step", () => {
    expect(readActivePlan("plans/active/ship.md", "# Ship\n\nsome background\n").nextStep).toBeNull();
  });

  it("does not read a heading or a next step out of the frontmatter block itself", () => {
    const plan = readActivePlan("plans/active/ship.md", "---\nstatus: paused\n---\n\n# Real title\n");
    expect(plan.title).toBe("Real title");
  });

  it("names a plan by its file stem", () => {
    expect(planSlug("plans/active/ship-r3.md")).toBe("ship-r3");
  });
});

describe("the journal tail", () => {
  const digest = [
    "---",
    "title: Journal 2026-09-04",
    "---",
    "",
    "# Journal 2026-09-04",
    "",
    "Exported one way from the daemon's orchestration journal.",
    "",
    "<!-- repomind:row 1 -->",
    "## 09:12:00 spawn_agent (ok)",
    "",
    "- Repo: repomon",
    "",
    "<!-- repomind:row 2 -->",
    "## 09:31:00 merge_lane (refused)",
    "",
    "- Detail: unattended merge",
    "",
  ].join("\n");

  it("returns the newest entries last, and neither the file title nor its preamble", () => {
    const entries = journalTail(digest);
    expect(entries).toHaveLength(2);
    expect(entries[0]).toContain("spawn_agent (ok)");
    expect(entries[1]).toContain("merge_lane (refused)");
    // The `# Journal ...` heading names the file; only the `##` row sections are entries.
    expect(entries.join("\n")).not.toContain("Exported one way");
    expect(entries.join("\n")).not.toContain("# Journal");
  });

  it("keeps only the last few entries", () => {
    expect(journalTail(digest, 1)).toHaveLength(1);
    expect(journalTail(digest, 1)[0]).toContain("merge_lane");
  });

  it("reads an empty digest as no entries at all", () => {
    expect(journalTail("")).toEqual([]);
    expect(journalTail("---\ntitle: empty\n---\n\nnothing yet\n")).toEqual([]);
  });

  it("names today's digest the way the export does", () => {
    expect(journalPathFor(new Date(2026, 8, 4))).toBe("journal/2026-09-04.md");
    expect(journalPathFor(new Date(2026, 11, 31))).toBe("journal/2026-12-31.md");
  });
});
