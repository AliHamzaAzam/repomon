import { afterEach, describe, expect, it, vi } from "vitest";

import type { ConfigView, DaemonEvent } from "../ipc/rpc";
import type { SoundCue, SoundPlayer } from "../audio/sound";
import {
  DEFAULT_SOUND_PREFERENCES,
  arbitrateSound,
  createNotificationStore,
  cueForFleetNotification,
  showNativeNotification,
  type FleetNotification,
  type NativeAlert,
} from "./notifications";

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    unminimize: () => Promise.resolve(),
    setFocus: () => Promise.resolve(),
  }),
}));

const permissions = vi.hoisted(() => ({ granted: true, requested: 0 }));

vi.mock("@tauri-apps/plugin-notification", () => ({
  isPermissionGranted: () => Promise.resolve(permissions.granted),
  requestPermission: () => {
    permissions.requested += 1;
    return Promise.resolve("granted");
  },
  sendNotification: vi.fn(),
}));

vi.mock("../ipc/rpc", () => ({
  daemonCall: vi.fn(),
  subscribeDaemon: vi.fn(),
}));

const config = (): ConfigView => ({
  ...DEFAULT_SOUND_PREFERENCES,
  accent: null,
  worktree_template: "{repo}-{branch}",
  default_agent: null,
  auto_continue: true,
  auto_continue_message: "continue",
  spawn_prompt: true,
  notify_needs_you: true,
  notify_rate_limited: true,
  notify_resumed: true,
  notify_idle: false,
  notify_show_why: true,
  notify_coalesce: true,
  notify_click_focus: true,
  notify_desktop_fallback: true,
  notify_subagents: false,
  usage_probe: false,
  expand_agents: false,
  sort_repos_by_activity: false,
  embedded_pty: true,
  orchestrator_agent: null,
  orchestrator_model: null,
});

function fakeSound(result = true) {
  const play = vi.fn<(cue: SoundCue, volume: number) => boolean>(() => result);
  return { player: { play } satisfies SoundPlayer, play };
}

function harness(overrides: Partial<ConfigView> = {}, soundResult = true) {
  const sound = fakeSound(soundResult);
  const native = vi.fn<(alert: NativeAlert, silent: boolean) => void>();
  let callback: ((event: DaemonEvent) => void) | undefined;
  const store = createNotificationStore(undefined, {
    sound: sound.player,
    hasFocus: () => false,
    showNative: native,
    loadConfig: async () => ({ ...config(), ...overrides }),
    subscribe: async (next) => {
      callback = next;
      return () => { callback = undefined; };
    },
    now: () => 123,
  });
  return { store, sound: sound.play, native, event: (event: DaemonEvent) => callback?.(event) };
}

afterEach(() => {
  vi.unstubAllGlobals();
  permissions.granted = true;
  permissions.requested = 0;
});

describe("sound mapping", () => {
  it.each([
    ["needs_you", "permission", "agent-needs-you"],
    ["needs_you", "decision", "agent-needs-you"],
    ["needs_you", "end_of_turn", "agent-finished"],
    ["needs_you", "done_candidate", "agent-finished"],
    ["idle", "none", "agent-finished"],
    ["stalled", "none", "error-or-stall"],
    ["rate_limited", "none", "error-or-stall"],
    ["resumed", "none", null],
  ] as const)("maps %s/%s to %s", (kind, attention, expected) => {
    expect(cueForFleetNotification({ kind, attention })).toBe(expected);
  });

  it("applies focus policy, cue toggles, and native/custom arbitration", () => {
    const available = fakeSound(true);
    expect(arbitrateSound("agent-needs-you", DEFAULT_SOUND_PREFERENCES, available.player, false)).toEqual({
      eligible: true,
      customScheduled: true,
      nativeSilent: true,
    });

    const unavailable = fakeSound(false);
    expect(arbitrateSound("agent-needs-you", DEFAULT_SOUND_PREFERENCES, unavailable.player, false)).toEqual({
      eligible: true,
      customScheduled: false,
      nativeSilent: false,
    });

    expect(arbitrateSound("agent-needs-you", DEFAULT_SOUND_PREFERENCES, available.player, true).nativeSilent).toBe(true);
    expect(available.play).toHaveBeenCalledTimes(1);

    expect(arbitrateSound("agent-needs-you", {
      ...DEFAULT_SOUND_PREFERENCES,
      notify_sound_agent_needs_you: false,
    }, available.player, false).eligible).toBe(false);
  });
});

describe("notification routing", () => {
  it("keeps cue and native arbitration behind the exact isNew ID guard", async () => {
    const test = harness();
    await test.store.start();
    const event: DaemonEvent = {
      jsonrpc: "2.0",
      method: "event.notification",
      params: {
        id: "7:s1:needs_you:1",
        lane_id: 7,
        kind: "needs_you",
        title: "Agent needs you",
        body: "Approve the command",
        attention: "permission",
      },
    };
    test.event(event);
    test.event(event);

    expect(test.store.items()).toHaveLength(1);
    expect(test.sound).toHaveBeenCalledTimes(1);
    expect(test.native).toHaveBeenCalledTimes(1);
    expect(test.native).toHaveBeenCalledWith(expect.objectContaining({ laneId: 7 }), true, undefined);
    test.store.stop();
  });

  it("allows one native system sound when eligible custom audio is unavailable", async () => {
    const test = harness({}, false);
    await test.store.start();
    test.event({
      jsonrpc: "2.0",
      method: "event.notification",
      params: {
        id: "7:s1:stalled:1",
        lane_id: 7,
        kind: "stalled",
        title: "Agent stalled",
        body: "No output",
        attention: "none",
      },
    });
    expect(test.native).toHaveBeenCalledWith(expect.anything(), false, undefined);
  });

  it("deduplicates repomind attention edges", async () => {
    const test = harness();
    await test.store.start();
    const status = (attention: string): DaemonEvent => ({
      jsonrpc: "2.0",
      method: "event.orchestrator.status",
      params: { attention, headline: "Review the plan" },
    });
    test.event(status("none"));
    test.event(status("permission"));
    test.event(status("decision"));
    test.event(status("none"));
    test.event(status("end_of_turn"));
    expect(test.sound.mock.calls.map(([cue]) => cue)).toEqual([
      "repomind-needs-you",
      "repomind-needs-you",
    ]);
  });

  it("deduplicates update versions", async () => {
    const test = harness();
    await test.store.start();
    expect(await test.store.notifyUpdateReady("0.6.0")).toBe(true);
    expect(await test.store.notifyUpdateReady("0.6.0")).toBe(false);
    expect(await test.store.notifyUpdateReady("0.6.1")).toBe(true);
    expect(test.sound.mock.calls.map(([cue]) => cue)).toEqual(["update-ready", "update-ready"]);
  });

  it("deduplicates newly stored messages by message ID", async () => {
    const test = harness();
    await test.store.start();
    const event: DaemonEvent = {
      jsonrpc: "2.0",
      method: "event.message.stored",
      params: { id: "mail-1", lane_id: 7, from: "lane-2/1", body: "Review complete" },
    };
    test.event(event);
    test.event(event);
    expect(test.sound).toHaveBeenCalledTimes(1);
    expect(test.sound).toHaveBeenCalledWith("incoming-message", 0.25);
  });
});

describe("native notification behavior", () => {
  it("activates the matching lane and preserves the silent choice", () => {
    let popup: { onclick: (() => void) | null; close: ReturnType<typeof vi.fn> } | undefined;
    let options: NotificationOptions | undefined;
    class NotificationStub {
      onclick: (() => void) | null = null;
      close = vi.fn();
      constructor(_title: string, next: NotificationOptions) {
        popup = this;
        options = next;
      }
    }
    vi.stubGlobal("Notification", NotificationStub);
    vi.spyOn(window, "focus").mockImplementation(() => undefined);
    const activate = vi.fn();
    const item: FleetNotification = {
      id: "7:s1:needs_you:1",
      lane_id: 7,
      kind: "needs_you",
      title: "Agent needs you",
      body: "Approve the command",
      attention: "permission",
      received_at: 1,
      read: false,
    };

    showNativeNotification(item, activate, true);
    popup?.onclick?.();
    expect(options?.silent).toBe(true);
    expect(activate).toHaveBeenCalledWith(7);
    expect(popup?.close).toHaveBeenCalled();
  });
});

describe("notification permission", () => {
  it("asks once on first run", async () => {
    permissions.granted = false;
    const test = harness();
    await test.store.start();
    expect(permissions.requested).toBe(1);
    expect(test.store.nativeEnabled()).toBe(true);
  });

  it("does not re-prompt when permission exists", async () => {
    const test = harness();
    await test.store.start();
    expect(permissions.requested).toBe(0);
    expect(test.store.nativeEnabled()).toBe(true);
  });
});
