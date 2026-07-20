import { describe, expect, it } from "vitest";

import { DaemonRpcError } from "../ipc/rpc";
import { replacementDialog } from "./ControlCenter";

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
