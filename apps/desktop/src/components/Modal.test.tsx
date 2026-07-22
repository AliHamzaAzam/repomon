import { fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { Show, createSignal } from "solid-js";
import { describe, expect, it } from "vitest";

import Modal from "./Modal";

describe("Modal focus management", () => {
  it("traps focus and restores the opener after close", async () => {
    const [open, setOpen] = createSignal(false);
    render(() => (
      <>
        <button type="button" onClick={() => setOpen(true)}>Open dialog</button>
        <Show when={open()}>
          <Modal
            title="Test dialog"
            onClose={() => setOpen(false)}
            footer={<button type="button">Last action</button>}
          >
            <button type="button">First action</button>
          </Modal>
        </Show>
      </>
    ));

    const opener = screen.getByRole("button", { name: "Open dialog" });
    opener.focus();
    fireEvent.click(opener);

    const close = await screen.findByRole("button", { name: "Close Test dialog" });
    await waitFor(() => expect(close).toHaveFocus());
    fireEvent.keyDown(close, { key: "Tab", shiftKey: true });
    expect(screen.getByRole("button", { name: "Last action" })).toHaveFocus();

    fireEvent.click(close);
    await waitFor(() => expect(opener).toHaveFocus());
  });
});
