import { describe, expect, it } from "vitest";

import { DaemonRpcError } from "../ipc/rpc";
import type { Playbook } from "../bindings";
import { journalQueryParams, playbookState, replacementDialog } from "./ControlCenter";

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
