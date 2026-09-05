import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ConnectionSnapshot } from "../ipc/connection";
import ConnectionTrouble from "./ConnectionTrouble";

afterEach(() => {
  cleanup();
});

const PIPE = "\\\\.\\pipe\\repomon-azama";

function snapshot(overrides: Partial<ConnectionSnapshot> = {}): ConnectionSnapshot {
  return {
    phase: "retrying",
    endpoint: PIPE,
    message: "the daemon started and exited immediately (exit code -1073741515 / 0xC0000135)",
    hint: "The Visual C++ runtime is missing or is the wrong architecture, so the daemon dies before it starts.",
    log_path: "C:\\Users\\azama\\AppData\\Roaming\\repomon\\data\\logs\\repomond.out.log",
    daemon: null,
    ...overrides,
  };
}

describe("the connection rail's trouble row", () => {
  it("names the endpoint and the fix, not just the failure", () => {
    render(() => (
      <ConnectionTrouble
        snapshot={snapshot()}
        onShowLog={async () => undefined}
        collectDiagnostics={async () => ""}
      />
    ));

    expect(screen.getByText(/Visual C\+\+ runtime is missing/)).toBeInTheDocument();
    expect(screen.getByText(PIPE)).toBeInTheDocument();
  });

  it("offers the daemon log even when the failure carried no hint", () => {
    const showLog = vi.fn(async () => undefined);
    render(() => (
      <ConnectionTrouble
        snapshot={snapshot({ hint: null })}
        onShowLog={showLog}
        collectDiagnostics={async () => ""}
      />
    ));

    fireEvent.click(screen.getByRole("button", { name: "Show log" }));
    expect(showLog).toHaveBeenCalledOnce();
  });

  it("puts the collected diagnostics on the clipboard and says so", async () => {
    const written: string[] = [];
    render(() => (
      <ConnectionTrouble
        snapshot={snapshot()}
        onShowLog={async () => undefined}
        collectDiagnostics={async () => "Repomon diagnostics\nendpoint: " + PIPE}
        writeClipboard={async (text) => {
          written.push(text);
        }}
      />
    ));

    fireEvent.click(screen.getByRole("button", { name: "Copy diagnostics" }));

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
    });
    expect(written).toEqual(["Repomon diagnostics\nendpoint: " + PIPE]);
  });

  it("reports a failed copy instead of pretending it worked", async () => {
    render(() => (
      <ConnectionTrouble
        snapshot={snapshot()}
        onShowLog={async () => undefined}
        collectDiagnostics={async () => {
          throw new Error("the daemon log could not be read");
        }}
        writeClipboard={async () => undefined}
      />
    ));

    fireEvent.click(screen.getByRole("button", { name: "Copy diagnostics" }));

    expect(await screen.findByText("the daemon log could not be read")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Copied" })).not.toBeInTheDocument();
  });

  it("reports a log that would not open", async () => {
    render(() => (
      <ConnectionTrouble
        snapshot={snapshot()}
        onShowLog={async () => {
          throw new Error("no application is registered for .log");
        }}
        collectDiagnostics={async () => ""}
      />
    ));

    fireEvent.click(screen.getByRole("button", { name: "Show log" }));
    expect(await screen.findByText("no application is registered for .log")).toBeInTheDocument();
  });
});
