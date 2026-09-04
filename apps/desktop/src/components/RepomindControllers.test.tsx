import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import { session } from "../test/repomindFixtures";
import RepomindControllers from "./RepomindControllers";

afterEach(cleanup);

describe("the controllers section", () => {
  it("lists each controller with its state and the daemon's reason for it", () => {
    render(() => (
      <RepomindControllers
        sessions={[
          session(),
          session({
            id: 2,
            session_id: "c2",
            custom_label: "Second",
            status: "waiting",
            attention_kind: "decision",
            pending_prompt: "Which auth method?",
            status_reason: "asked a question",
          }),
        ]}
        onFocus={vi.fn()}
      />
    ));

    expect(screen.getByText("Primary")).toBeInTheDocument();
    expect(screen.getByText("Second")).toBeInTheDocument();
    expect(screen.getByText("running")).toBeInTheDocument();
    expect(screen.getByText("needs you")).toBeInTheDocument();
    expect(screen.getByText("Second: asked a question")).toBeInTheDocument();
  });

  it("reads a controller between instructions as idle", () => {
    render(() => (
      <RepomindControllers
        sessions={[session({ status: "waiting", attention_kind: "end_of_turn" })]}
        onFocus={vi.fn()}
      />
    ));

    expect(screen.getByText("idle")).toBeInTheDocument();
    expect(screen.queryByText("needs you")).toBeNull();
  });

  it("focuses a controller's pane rather than talking to it here", () => {
    const onFocus = vi.fn();
    const primary = session();
    render(() => <RepomindControllers sessions={[primary]} onFocus={onFocus} />);

    fireEvent.click(screen.getByText("Focus pane"));
    expect(onFocus).toHaveBeenCalledWith(primary);
  });

  it("cannot focus a session with no managed pane", () => {
    render(() => (
      <RepomindControllers sessions={[session({ tmux_window: null })]} onFocus={vi.fn()} />
    ));

    expect(screen.getByText("Focus pane")).toBeDisabled();
  });

  it("says what starting a controller buys when none is running", () => {
    render(() => <RepomindControllers sessions={[]} onFocus={vi.fn()} />);

    expect(screen.getByText(/already knowing your house rules/)).toBeInTheDocument();
  });
});
