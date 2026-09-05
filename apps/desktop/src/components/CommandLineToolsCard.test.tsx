import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { CliStatus } from "../ipc/cli";
import CommandLineToolsCard from "./CommandLineToolsCard";

afterEach(() => {
  cleanup();
});

function notInstalled(): CliStatus {
  return {
    installed: false,
    dir: "/Users/pat/.local/bin",
    on_path: true,
    version: null,
    tools: [],
    missing: ["repomon", "repomond"],
    path_hint: null,
  };
}

function installedAndOnPath(): CliStatus {
  return {
    installed: true,
    dir: "/Users/pat/.local/bin",
    on_path: true,
    version: "repomon 0.8.1",
    tools: ["repomon", "repomond"],
    missing: [],
    path_hint: null,
  };
}

function installedButUnreachable(): CliStatus {
  return {
    installed: true,
    dir: "/Users/pat/.local/bin",
    on_path: false,
    version: "repomon 0.8.1",
    tools: ["repomon", "repomond"],
    missing: [],
    path_hint: 'Add this line to your shell rc (~/.zshrc or ~/.bashrc): export PATH="/Users/pat/.local/bin:$PATH"',
  };
}

describe("the command-line tools card", () => {
  it("offers Install when nothing is there yet", async () => {
    render(() => <CommandLineToolsCard read={async () => notInstalled()} />);

    expect(await screen.findByText("Not installed")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Install" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove" })).not.toBeInTheDocument();
  });

  it("reports the installed version and that a shell can reach it", async () => {
    render(() => <CommandLineToolsCard read={async () => installedAndOnPath()} />);

    expect(await screen.findByText("On your PATH")).toBeInTheDocument();
    expect(screen.getByText("repomon 0.8.1")).toBeInTheDocument();
    expect(screen.getByText("/Users/pat/.local/bin")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove" })).toBeInTheDocument();
  });

  it("does not call an install a success when the shell still cannot find it", async () => {
    render(() => (
      <CommandLineToolsCard
        read={async () => notInstalled()}
        install={async () => installedButUnreachable()}
      />
    ));

    fireEvent.click(await screen.findByRole("button", { name: "Install" }));

    expect(await screen.findByText("Installed, not on PATH")).toBeInTheDocument();
    expect(screen.getByText(/export PATH="\/Users\/pat\/\.local\/bin:\$PATH"/)).toBeInTheDocument();
  });

  it("copies the exact line to add to a shell rc", async () => {
    const written: string[] = [];
    render(() => (
      <CommandLineToolsCard
        read={async () => installedButUnreachable()}
        writeClipboard={async (text) => {
          written.push(text);
        }}
      />
    ));

    fireEvent.click(await screen.findByRole("button", { name: "Copy the PATH line" }));

    await waitFor(() => {
      expect(screen.getByText("Copied")).toBeInTheDocument();
    });
    expect(written[0]).toContain('export PATH="/Users/pat/.local/bin:$PATH"');
  });

  it("goes back to Not installed after Remove", async () => {
    const uninstall = vi.fn(async () => notInstalled());
    render(() => (
      <CommandLineToolsCard read={async () => installedAndOnPath()} uninstall={uninstall} />
    ));

    fireEvent.click(await screen.findByRole("button", { name: "Remove" }));

    expect(await screen.findByText("Not installed")).toBeInTheDocument();
    expect(uninstall).toHaveBeenCalledOnce();
  });

  it("shows why an install failed rather than silently doing nothing", async () => {
    render(() => (
      <CommandLineToolsCard
        read={async () => notInstalled()}
        install={async () => {
          throw new Error("this build does not carry repomon next to the app");
        }}
      />
    ));

    fireEvent.click(await screen.findByRole("button", { name: "Install" }));

    expect(
      await screen.findByText("this build does not carry repomon next to the app"),
    ).toBeInTheDocument();
  });

  it("renders nothing outside the Tauri shell", () => {
    const { container } = render(() => <CommandLineToolsCard />);
    expect(container.querySelector("[data-testid='command-line-tools']")).toBeNull();
  });
});
