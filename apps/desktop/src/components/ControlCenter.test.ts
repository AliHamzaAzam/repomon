import { describe, expect, it } from "vitest";

import { DaemonRpcError } from "../ipc/rpc";
import type { ApprovalRule, Playbook } from "../bindings";
import { groupApprovalRules, journalQueryParams, playbookState, replacementDialog, scheduleAddParams } from "./ControlCenter";

describe("safe dialog answers", () => {
  it("extracts the replacement after DIALOG_CHANGED", () => {
    const dialog = { title: "Question", question: "Continue?", body: [], options: [], selected: null };
    const error = new DaemonRpcError({ code: -32010, message: "dialog changed", data: { dialog } });
    expect(replacementDialog(error)).toEqual(dialog);
  });

  it("does not treat unrelated failures as replacement dialogs", () => {
    expect(replacementDialog(new Error("offline"))).toBeUndefined();
  });
});

describe("journal query", () => {
  it("asks for the recent tail when the box is empty", () => {
    // Not `query: ""`: that is a substring search matching every row, ranked by relevance rather
    // than recency, which is the opposite of what the tab should show on open.
    expect(journalQueryParams("")).toEqual({ limit: 200 });
    expect(journalQueryParams("   ")).toEqual({ limit: 200 });
  });

  it("trims a real search before sending it", () => {
    expect(journalQueryParams("  merge_lane  ")).toEqual({ query: "merge_lane", limit: 200 });
  });
});

describe("playbook approval state", () => {
  const base: Playbook = {
    name: "release-all", content: "steps", status: "draft", draft_content: null,
    created_at: "2026-07-27T00:00:00Z", updated_at: "2026-07-27T00:00:00Z", approved_at: null,
  };

  it("treats a fresh draft as awaiting approval", () => {
    expect(playbookState(base)).toEqual({ label: "draft", awaitingApproval: true });
  });

  it("treats an approved playbook with no pending revision as settled", () => {
    const approved = { ...base, status: "approved", approved_at: "2026-07-27T01:00:00Z" };
    expect(playbookState(approved)).toEqual({ label: "approved", awaitingApproval: false });
  });

  // The case worth being precise about: the old approved text is still what repomind follows,
  // but a revision is queued. Reporting plain "approved" would hide that.
  it("flags a re-drafted playbook as pending without calling it a draft", () => {
    const revised = { ...base, status: "approved", draft_content: "new steps" };
    expect(playbookState(revised)).toEqual({
      label: "approved · revision pending",
      awaitingApproval: true,
    });
  });
});

describe("schedule add params", () => {
  it("trims the spec and goal", () => {
    expect(scheduleAddParams("  weekdays 09:00 ", " briefing ", "")).toEqual({
      spec: "weekdays 09:00",
      prompt: "briefing",
    });
  });

  // Sending 0 would cap an unattended run at no actions: a schedule that fires and does nothing,
  // which reads as a broken scheduler rather than a bad cap. Omit instead and let the daemon
  // apply its own conservative default.
  it("omits a blank or nonsense cap instead of sending zero", () => {
    for (const cap of ["", "   ", "abc", "0", "-4"]) {
      expect(scheduleAddParams("daily 09:00", "g", cap)).toEqual({ spec: "daily 09:00", prompt: "g" });
    }
  });

  it("passes a real cap through", () => {
    expect(scheduleAddParams("every 30m", "g", " 20 ")).toEqual({
      spec: "every 30m",
      prompt: "g",
      max_actions: 20,
    });
  });
});

describe("approval rules grouping", () => {
  const rule = (repo: string, pattern: string): ApprovalRule =>
    ({ repo, pattern, created_at: "2026-07-27T00:00:00Z" });

  it("groups by repo and sorts both levels", () => {
    const grouped = groupApprovalRules([
      rule("zeta", "cargo test"),
      rule("alpha", "pnpm test"),
      rule("alpha", "cargo build"),
    ]);
    expect(grouped.map((g) => g.repo)).toEqual(["alpha", "zeta"]);
    expect(grouped[0].rules.map((r) => r.pattern)).toEqual(["cargo build", "pnpm test"]);
  });

  // A rule is scoped to one repo, so the same pattern approved in two repos is two independent
  // rules. Collapsing them would imply revoking one revokes both.
  it("keeps the same pattern in two repos as two rules", () => {
    const grouped = groupApprovalRules([rule("a", "cargo test"), rule("b", "cargo test")]);
    expect(grouped).toHaveLength(2);
    expect(grouped.every((g) => g.rules.length === 1)).toBe(true);
  });

  it("returns nothing for no rules", () => {
    expect(groupApprovalRules([])).toEqual([]);
  });
});
