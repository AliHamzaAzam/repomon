import { render, screen, waitFor } from "@solidjs/testing-library";
import { describe, expect, it } from "vitest";

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
      expect(screen.getByText("Version 0.5.0")).toBeInTheDocument();
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
});
