import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { DaemonBootCheck } from "../ipc/boot";
import DaemonBootRow from "./DaemonBootRow";

afterEach(() => {
  cleanup();
});

const WINDOWS_PATH = "C:\\Program Files\\Repomon\\repomond.exe";
const WINDOWS_LOG = "C:\\Users\\azama\\AppData\\Roaming\\repomon\\data\\logs\\repomond.out.log";

function starts(): DaemonBootCheck {
  return {
    ok: true,
    path: "/Applications/Repomon.app/Contents/MacOS/repomond",
    version: "repomond 0.8.1",
    error: null,
    hint: null,
    log_path: "/Users/pat/Library/Application Support/repomon/logs/repomond.out.log",
  };
}

function diesInTheLoader(): DaemonBootCheck {
  return {
    ok: false,
    path: WINDOWS_PATH,
    version: null,
    error: "exit code -1073741515 / 0xC0000135",
    hint: "The Visual C++ runtime is missing or is the wrong architecture, so the daemon dies before it starts.",
    log_path: WINDOWS_LOG,
  };
}

describe("the System check's daemon launch row", () => {
  it("reports the version when the bundled binary starts", async () => {
    render(() => <DaemonBootRow check={async () => starts()} />);

    expect(await screen.findByText("Starts")).toBeInTheDocument();
    expect(screen.getByText("repomond 0.8.1")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Show log" })).not.toBeInTheDocument();
  });

  it("reports the exact error and the fix when it does not", async () => {
    render(() => <DaemonBootRow check={async () => diesInTheLoader()} />);

    expect(await screen.findByText("Does not start")).toBeInTheDocument();
    expect(screen.getByText("exit code -1073741515 / 0xC0000135")).toBeInTheDocument();
    expect(screen.getByText(/Visual C\+\+ runtime is missing/)).toBeInTheDocument();
    expect(screen.getByText(WINDOWS_LOG)).toBeInTheDocument();
  });

  it("opens the daemon log from the failure state", async () => {
    const showLog = vi.fn(async () => undefined);
    render(() => <DaemonBootRow check={async () => diesInTheLoader()} showLog={showLog} />);

    fireEvent.click(await screen.findByRole("button", { name: "Show log" }));
    expect(showLog).toHaveBeenCalledOnce();
  });

  it("re-runs the probe on demand, so a fix can be verified without a restart", async () => {
    let calls = 0;
    render(() => (
      <DaemonBootRow
        check={async () => {
          calls += 1;
          return calls === 1 ? diesInTheLoader() : starts();
        }}
      />
    ));

    expect(await screen.findByText("Does not start")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Re-run the daemon launch check" }));

    await waitFor(() => {
      expect(screen.getByText("Starts")).toBeInTheDocument();
    });
    expect(calls).toBe(2);
  });

  it("says the check could not run rather than blaming the daemon", async () => {
    render(() => (
      <DaemonBootRow
        check={async () => {
          throw new Error("the command bridge is gone");
        }}
      />
    ));

    expect(
      await screen.findByText("The launch check could not be run from this window."),
    ).toBeInTheDocument();
  });

  it("renders nothing outside the Tauri shell, where there is nothing truthful to say", () => {
    const { container } = render(() => <DaemonBootRow />);
    expect(container.querySelector("[data-testid='daemon-boot-row']")).toBeNull();
  });
});
