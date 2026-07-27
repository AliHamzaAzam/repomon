import { describe, expect, it } from "vitest";

import { DaemonRpcError } from "../ipc/rpc";
import { journalQueryParams, replacementDialog } from "./ControlCenter";

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
