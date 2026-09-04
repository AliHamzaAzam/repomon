import { describe, expect, it } from "vitest";

import {
  donePlanDocument,
  journalDays,
  journalEntries,
  journalPathFor,
  journalTail,
  newPlanDocument,
  planSlug,
  planSlugFor,
  readActivePlan,
  readPlanSummary,
} from "./repomindDocs";

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

describe("readPlanSummary", () => {
  it("reads the owner and the last-moved stamp beside the title and next step", () => {
    const plan = readPlanSummary(
      "plans/active/ship-r6.md",
      "---\ntitle: Ship R6\nowner: lane-7/1\nupdated: 2026-09-04T09:00:00Z\n---\n\nNext step: land the board\n",
    );
    expect(plan).toEqual({
      path: "plans/active/ship-r6.md",
      title: "Ship R6",
      nextStep: "land the board",
      owner: "lane-7/1",
      updated: "2026-09-04T09:00:00Z",
    });
  });

  it("falls back to created, and says nothing rather than guessing an owner", () => {
    const plan = readPlanSummary(
      "plans/active/loose.md",
      "---\ntitle: Loose\ncreated: 2026-09-01T00:00:00Z\n---\n\nbody\n",
    );
    expect(plan.owner).toBeNull();
    expect(plan.updated).toBe("2026-09-01T00:00:00Z");
  });

  it("reads an owner written into the body, the way an operator types one", () => {
    const plan = readPlanSummary("plans/active/hand.md", "# Hand written\n\n- Owner: pat\n");
    expect(plan.owner).toBe("pat");
  });
});

describe("planSlugFor", () => {
  it("turns a title into the file name the home uses", () => {
    expect(planSlugFor("Ship R6: the control room")).toBe("ship-r6-the-control-room");
    expect(planSlugFor("  Trailing  ")).toBe("trailing");
    expect(planSlugFor("!!!")).toBe("goal");
    expect(planSlugFor("x".repeat(80))).toHaveLength(60);
  });
});

describe("newPlanDocument", () => {
  const now = new Date("2026-09-05T10:30:00.000Z");

  it("writes the home's frontmatter with the intent as the next step", () => {
    const doc = newPlanDocument("Ship R6", "land the plans board", now);
    expect(doc).toBe(
      [
        "---",
        "title: Ship R6",
        "type: plan",
        "permalink: repomind/plans/active/ship-r6",
        "status: active",
        "owner: unassigned",
        "source: repomon desktop 2026-09-05",
        'created: "2026-09-05T10:30:00.000Z"',
        "---",
        "",
        "# Ship R6",
        "",
        "Next step: land the plans board",
        "",
      ].join("\n"),
    );
  });

  it("quotes a value that would otherwise read as YAML structure", () => {
    const doc = newPlanDocument("Ship R6: the control room", "unblock the gate", now);
    expect(doc).toContain('title: "Ship R6: the control room"');
    expect(doc).toContain("# Ship R6: the control room");
  });

  it("produces a plan the daemon's own reader understands", () => {
    const doc = newPlanDocument("Ship R6", "land the plans board", now);
    expect(readPlanSummary("plans/active/ship-r6.md", doc)).toMatchObject({
      title: "Ship R6",
      nextStep: "land the plans board",
      owner: "unassigned",
    });
  });
});

describe("donePlanDocument", () => {
  const now = new Date("2026-09-05T10:30:00.000Z");

  it("closes the plan and appends the outcome without losing the body", () => {
    const done = donePlanDocument(
      "---\ntitle: Ship R6\nstatus: active\nowner: pat\n---\n\n# Ship R6\n\nNext step: land it\n",
      "shipped on main",
      now,
    );
    expect(done).toContain("status: done\n");
    expect(done).toContain('closed: "2026-09-05T10:30:00.000Z"\n');
    expect(done).toContain("owner: pat\n");
    expect(done).toContain("# Ship R6");
    expect(done).toContain("Next step: land it");
    expect(done.trimEnd().endsWith("Outcome: shipped on main")).toBe(true);
  });

  it("stamps a plan that never carried frontmatter", () => {
    const done = donePlanDocument("# Loose\n\nsome notes\n", "abandoned", now);
    expect(done.startsWith("---\nstatus: done\n")).toBe(true);
    expect(done).toContain("some notes");
    expect(done).toContain("Outcome: abandoned");
  });
});

describe("journalDays", () => {
  it("lists day files newest first and keeps archived months apart", () => {
    const { days, archive } = journalDays([
      "journal/2026-09-03.md",
      "journal/2026-09-05.md",
      "journal/README.md",
      "journal/notes.txt",
      "journal/archive/2026-05.md",
      "journal/archive/2026-06.md",
    ]);
    expect(days.map((day) => day.label)).toEqual(["2026-09-05", "2026-09-03"]);
    expect(days.every((day) => !day.archived)).toBe(true);
    expect(archive.map((month) => month.label)).toEqual(["2026-06", "2026-05"]);
    expect(archive.every((month) => month.archived)).toBe(true);
  });
});

describe("journalEntries", () => {
  it("returns every entry in a day, not just the tail", () => {
    const content = "# Journal\n\n## one\n\nbody\n\n## two\n\n## three\n\n## four\n\n## five\n\n## six\n";
    expect(journalEntries(content)).toHaveLength(6);
    expect(journalTail(content)).toHaveLength(5);
  });
});
