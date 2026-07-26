import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { beforeAll, describe, expect, it } from "vitest";

import App from "./App";
import type { ConnectionSnapshot, ConnectionSource } from "./ipc/connection";
import type { FleetSource } from "./stores/fleet";

function sourceFor(snapshot: ConnectionSnapshot): ConnectionSource {
  return {
    current: async () => snapshot,
    subscribe: async () => () => undefined,
  };
}

describe("Repomon desktop shell", () => {
  beforeAll(() => {
    // App.tsx's shortcut handler resolves "mod" from navigator.platform when no explicit
    // platform is passed in (the real, unmocked path the app uses at runtime). jsdom reports an
    // empty platform string, so pin it to macOS here: the fixtures below fire metaKey to mean
    // "mod", matching how the app actually runs on macOS.
    Object.defineProperty(navigator, "platform", { value: "MacIntel", configurable: true });
  });


  it("renders the mission control frame and connection rail", () => {
    render(() => <App connectionSource={sourceFor({
      phase: "starting",
      endpoint: "Resolving local daemon endpoint",
      message: null,
      daemon: null,
    })} />);

    expect(screen.getByRole("heading", { name: "Repomon" })).toBeInTheDocument();
    expect(screen.getByRole("navigation", { name: "Fleet" })).toBeInTheDocument();
    expect(screen.getByRole("main", { name: "Terminal bay" })).toBeInTheDocument();
    expect(screen.getByRole("complementary", { name: "Repomind" })).toBeInTheDocument();
    expect(screen.getByRole("status", { name: "Daemon connection" })).toBeInTheDocument();
  });

  it("shows live daemon metrics when the host connects", async () => {
    render(() => <App connectionSource={sourceFor({
      phase: "connected",
      endpoint: "/tmp/repomon.sock",
      message: null,
      daemon: {
        uptime_secs: 3661,
        repos: 3,
        lanes: 5,
        db_size_bytes: 4096,
        version: "0.5.0",
      },
    })} />);

    await waitFor(() => {
      expect(screen.getByText("Connected")).toBeInTheDocument();
      expect(screen.getByText(/daemon 0\.5\.0/)).toBeInTheDocument();
      expect(screen.getByText("3 repos / 5 lanes")).toBeInTheDocument();
      expect(screen.getByText("Uptime 1h 01m")).toBeInTheDocument();
    });
  });

  it("makes a lost connection actionable", async () => {
    render(() => <App connectionSource={sourceFor({
      phase: "retrying",
      endpoint: "/tmp/repomon.sock",
      message: "daemon connection closed",
      daemon: null,
    })} />);

    await waitFor(() => {
      expect(screen.getByText("Retrying")).toBeInTheDocument();
      expect(screen.getByText("daemon connection closed")).toBeInTheDocument();
    });
  });

  it("surfaces fleet loading errors instead of failing silently", async () => {
    const fleetSource: FleetSource = {
      load: async () => { throw new Error("fleet sync failed"); },
      subscribe: async () => () => undefined,
    };
    render(() => <App connectionSource={sourceFor({
      phase: "connected",
      endpoint: "/tmp/repomon.sock",
      message: null,
      daemon: null,
    })} fleetSource={fleetSource} />);

    expect(await screen.findByText("fleet sync failed")).toBeInTheDocument();
  });

  it("drives the Extensions panel from the keymap table, not a bare digit", async () => {
    // Scoped to this render's container: earlier tests in this file leave their DOM mounted
    // (no cleanup wired up), so an unscoped query would see every prior "Extensions" button too.
    const { container } = render(() => <App connectionSource={sourceFor({
      phase: "starting",
      endpoint: "Resolving local daemon endpoint",
      message: null,
      daemon: null,
    })} />);

    const extensions = within(container).getByRole("button", { name: "Extensions" });
    expect(extensions).toHaveAttribute("aria-pressed", "false");

    // The old ad-hoc listener toggled Extensions on a bare "6"; that would steal a keystroke
    // meant for a focused agent terminal. The keymap-driven handler ignores unmodified keys.
    fireEvent.keyDown(window, { key: "6", code: "Digit6" });
    expect(extensions).toHaveAttribute("aria-pressed", "false");

    fireEvent.keyDown(window, { key: "4", code: "Digit4", metaKey: true });
    await waitFor(() => expect(extensions).toHaveAttribute("aria-pressed", "true"));

    fireEvent.keyDown(window, { key: "4", code: "Digit4", metaKey: true });
    await waitFor(() => expect(extensions).toHaveAttribute("aria-pressed", "false"));
  });
});
