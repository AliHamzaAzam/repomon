import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount, type JSX } from "solid-js";

import type { AgentChoice } from "../bindings";
import {
  SOUND_PROFILES,
  readSoundProfile,
  saveSoundProfile,
  type SoundCue,
  type SoundProfile,
} from "../audio/sound";
import { daemonCall, type ConfigView } from "../ipc/rpc";
import { checkForUpdate, type AvailableUpdate, type UpdateProgress } from "../ipc/updater";
import {
  applyAccent,
  applyTheme,
  readTheme,
  readTerminalAppearance,
  saveTerminalAppearance,
  ACCENT_SWATCHES,
  TERMINAL_FONT_FAMILIES,
  THEME_PRESETS,
  type Theme,
  type TerminalAppearance,
} from "../theme";
import {
  readAutoCollapseEmptyLanes,
  saveAutoCollapseEmptyLanes,
} from "../stores/uiSettings";
import AutomationSettings from "./AutomationSettings";
import ColorField from "./controls/ColorField";
import Select from "./controls/Select";
import Switch from "./controls/Switch";
import KeyboardHelp from "./KeyboardHelp";
import { translateError } from "../ipc/errors";
import Modal from "./Modal";
import SystemHealthView from "./SystemHealthView";
import {
  AGENT_ICON_CATALOG,
  AgentIcon,
  resolveAgentIconKey,
  setAgentIconOverrides,
  IconCheck,
  IconClose,
  IconPlus,
  IconRefresh,
  IconSearch,
  IconTrash,
} from "./icons";

export type SettingsTab = "general" | "system" | "agents" | "notifications" | "appearance" | "automation" | "keyboard";

interface SettingsModalProps {
  onClose: () => void;
  initialTab?: SettingsTab;
  onConfigSaved?: (config: ConfigView) => void;
  onPreviewSound?: (cue: SoundCue, volume: number, profile?: SoundProfile) => boolean;
  onUpdateAvailable?: (version: string) => void;
  onReplayOnboarding?: () => void;
  fleet?: import("../stores/fleet").FleetStore;
  actions?: import("../stores/actions").ActionsStore;
}

const TABS: Array<{ id: SettingsTab; label: string }> = [
  { id: "general", label: "General" },
  { id: "system", label: "System" },
  { id: "agents", label: "Agents" },
  { id: "notifications", label: "Notifications" },
  { id: "appearance", label: "Appearance" },
  { id: "automation", label: "Automation" },
  { id: "keyboard", label: "Keyboard" },
];

const NOTIFY_TOGGLES: Array<[keyof ConfigView, string]> = [
  ["notify_needs_you", "Needs you"],
  ["notify_rate_limited", "Rate limited"],
  ["notify_resumed", "Resumed"],
  ["notify_idle", "Idle / ended"],
  ["notify_show_why", "Show why (last message)"],
  ["notify_coalesce", "Coalesce bursts"],
  ["notify_click_focus", "Click to focus"],
  ["notify_desktop_fallback", "System popup when no window is open"],
  ["notify_subagents", "Include subagents"],
];

const SOUND_CUE_CONTROLS: Array<{
  key: keyof ConfigView;
  cue: SoundCue;
  label: string;
}> = [
  { key: "notify_sound_agent_needs_you", cue: "agent-needs-you", label: "Agent needs you" },
  { key: "notify_sound_agent_finished", cue: "agent-finished", label: "Agent finished" },
  { key: "notify_sound_repomind_needs_you", cue: "repomind-needs-you", label: "Repomind needs you" },
  { key: "notify_sound_error_or_stall", cue: "error-or-stall", label: "Error or stall" },
  { key: "notify_sound_incoming_message", cue: "incoming-message", label: "Incoming message" },
  { key: "notify_sound_update_ready", cue: "update-ready", label: "Update ready" },
];

const GENERAL_TOGGLES: Array<[keyof ConfigView, string]> = [
  ["auto_continue", "Auto-continue rate-limited agents"],
  ["spawn_prompt", "Prompt for agent on spawn"],
  ["usage_probe", "Probe account usage"],
  ["expand_agents", "Expand multi-agent lanes"],
  ["embedded_pty", "Embedded terminal renderer"],
];

function agentSelectOptions(agents: AgentChoice[], current: string | null | undefined): Array<{ value: string; label: string }> {
  const options = [{ value: "", label: "Default" }, ...agents.map((choice) => ({ value: choice.name, label: choice.name }))];
  const value = current ?? "";
  if (value && !options.some((option) => option.value === value)) {
    options.push({ value, label: value });
  }
  return options;
}

function TextField(props: {
  label: string;
  value: string;
  onInput: (value: string) => void;
  placeholder?: string;
}) {
  return (
    <label class="block">
      <span class="section-label">{props.label}</span>
      <input
        class="focus-ring mt-1.5 h-8 w-full rounded-lg border border-line bg-surface px-3 text-xs text-foreground outline-none placeholder:text-muted/60"
        value={props.value}
        placeholder={props.placeholder}
        onInput={(event) => props.onInput(event.currentTarget.value)}
      />
    </label>
  );
}

export default function SettingsModal(props: SettingsModalProps) {
  const [tab, setTab] = createSignal<SettingsTab>(props.initialTab ?? "general");

  createEffect(() => {
    const requested = props.initialTab;
    if (requested) setTab(requested);
  });
  const [config, setConfig] = createSignal<ConfigView | null>(null);
  const [agents, setAgents] = createSignal<AgentChoice[]>([]);
  const [error, setError] = createSignal<string | null>(null);
  const [status, setStatus] = createSignal<string | null>(null);
  const [checking, setChecking] = createSignal(false);
  const [progress, setProgress] = createSignal<UpdateProgress | null>(null);
  const [availableUpdate, setAvailableUpdate] = createSignal<AvailableUpdate | null>(null);

  // Icon customization state
  const [pickerAgent, setPickerAgent] = createSignal<string | null>(null);
  const [iconFilter, setIconFilter] = createSignal("");

  // Custom Agent registration state
  const [customAgentName, setCustomAgentName] = createSignal("");
  const [customAgentCommand, setCustomAgentCommand] = createSignal("");
  const [customAgentBusy, setCustomAgentBusy] = createSignal(false);
  const [customAgentError, setCustomAgentError] = createSignal<string | null>(null);
  const [customAgentSuccess, setCustomAgentSuccess] = createSignal<string | null>(null);

  onMount(() => {
    void daemonCall("config.get").then((cfg) => {
      setConfig(cfg);
      if (cfg.agent_icons) setAgentIconOverrides(cfg.agent_icons);
    }).catch((cause: unknown) => {
      setError(cause instanceof Error ? cause.message : String(cause));
    });
    void daemonCall("agent.detect").then(setAgents).catch(() => undefined);
  });

  async function addCustomAgent(e?: Event) {
    e?.preventDefault();
    const name = customAgentName().trim();
    const command = customAgentCommand().trim();
    if (!name || !command) {
      setCustomAgentError("Agent name and launch command are both required.");
      return;
    }
    setCustomAgentError(null);
    setCustomAgentBusy(true);
    try {
      await daemonCall("agent.add", { name, command });
      const [nextConfig, nextAgents] = await Promise.all([
        daemonCall("config.get"),
        daemonCall("agent.detect"),
      ]);
      setConfig(nextConfig);
      setAgents(nextAgents);
      props.onConfigSaved?.(nextConfig);
      void props.fleet?.refresh();
      setCustomAgentName("");
      setCustomAgentCommand("");
      setCustomAgentSuccess(`Registered custom agent '${name}'`);
      setTimeout(() => setCustomAgentSuccess(null), 3000);
    } catch (cause) {
      const trans = translateError(cause);
      setCustomAgentError(trans.friendly || String(cause));
    } finally {
      setCustomAgentBusy(false);
    }
  }

  function removeCustomAgent(name: string) {
    if (props.actions) {
      props.actions.confirm({
        title: `Remove agent '${name}'?`,
        message: `Remove custom agent '${name}' from Repomon. Any active sessions will not be affected.`,
        confirmLabel: "Remove Agent",
        danger: true,
        onConfirm: () => executeRemoveAgent(name),
      });
    } else {
      void executeRemoveAgent(name);
    }
  }

  async function executeRemoveAgent(name: string) {
    setError(null);
    setCustomAgentError(null);
    try {
      await daemonCall("agent.remove", { name });
      const [nextConfig, nextAgents] = await Promise.all([
        daemonCall("config.get"),
        daemonCall("agent.detect"),
      ]);
      setConfig(nextConfig);
      setAgents(nextAgents);
      props.onConfigSaved?.(nextConfig);
      void props.fleet?.refresh();
    } catch (cause) {
      const trans = translateError(cause);
      setCustomAgentError(trans.friendly || String(cause));
    }
  }

  async function setDefaultAgent(name: string | null) {
    setError(null);
    setCustomAgentError(null);
    try {
      await daemonCall("agent.set_default", { name });
      const [nextConfig, nextAgents] = await Promise.all([
        daemonCall("config.get"),
        daemonCall("agent.detect"),
      ]);
      setConfig(nextConfig);
      setAgents(nextAgents);
      props.onConfigSaved?.(nextConfig);
      void props.fleet?.refresh();
    } catch (cause) {
      const trans = translateError(cause);
      setCustomAgentError(trans.friendly || String(cause));
    }
  }

  // Daemon & Recovery state
  const [daemonInfo, setDaemonInfo] = createSignal<{ service_managed: boolean; status: string } | null>(null);
  const [daemonBusy, setDaemonBusy] = createSignal(false);
  const [restoreBusy, setRestoreBusy] = createSignal(false);
  const [restoreResult, setRestoreResult] = createSignal<string | null>(null);
  const [confirmRestart, setConfirmRestart] = createSignal(false);

  // Load daemon service info on mount when fleet prop is available
  createEffect(() => {
    if (tab() === "general" && props.fleet) {
      import("../ipc/daemonControl").then(({ getDaemonServiceInfo }) => {
        getDaemonServiceInfo().then(setDaemonInfo).catch(() => undefined);
      });
    }
  });

  // Count orphaned/external sessions across all lanes
  const orphanCount = () => {
    if (!props.fleet) return 0;
    let count = 0;
    for (const lane of props.fleet.lanes()) {
      for (const sess of lane.agent_sessions) {
        if (sess.external || (!sess.tmux_window && sess.session_id)) count++;
      }
    }
    return count;
  };

  const [saveStatus, setSaveStatus] = createSignal<"idle" | "saving" | "saved" | "error">("idle");
  let saveTimer: ReturnType<typeof setTimeout> | undefined;
  let debounceTimer: ReturnType<typeof setTimeout> | undefined;

  onCleanup(() => {
    if (saveTimer) clearTimeout(saveTimer);
    if (debounceTimer) clearTimeout(debounceTimer);
  });

  async function persistConfig(nextConfig: ConfigView) {
    setSaveStatus("saving");
    setError(null);
    try {
      const saved = await daemonCall("config.set", nextConfig);
      setConfig(saved);
      props.onConfigSaved?.(saved);
      if (saved.accent) applyAccent(saved.accent);
      if (saved.agent_icons) setAgentIconOverrides(saved.agent_icons);
      setSaveStatus("saved");
      if (saveTimer) clearTimeout(saveTimer);
      saveTimer = setTimeout(() => setSaveStatus("idle"), 2000);
    } catch (cause) {
      setSaveStatus("error");
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  const [currentTheme, setCurrentTheme] = createSignal<Theme>(readTheme());
  const [activeSoundProfile, setActiveSoundProfile] = createSignal<SoundProfile>(readSoundProfile());
  const [terminalApp, setTerminalApp] = createSignal<TerminalAppearance>(readTerminalAppearance());
  const [autoCollapseEmpty, setAutoCollapseEmpty] = createSignal<boolean>(readAutoCollapseEmptyLanes());

  function selectTheme(themeId: Theme) {
    setCurrentTheme(themeId);
    applyTheme(themeId);
    patch({ theme: themeId });
  }

  function selectAccent(accentKey: string) {
    patch({ accent: accentKey });
    applyAccent(accentKey);
  }

  function patch(next: Partial<ConfigView>, debounce = false) {
    const current = config();
    if (!current) return;
    const merged = { ...current, ...next };
    setConfig(merged);
    if (next.accent) applyAccent(next.accent);
    if (next.agent_icons) setAgentIconOverrides(next.agent_icons);

    if (debounce) {
      if (debounceTimer) clearTimeout(debounceTimer);
      setSaveStatus("saving");
      debounceTimer = setTimeout(() => {
        void persistConfig(merged);
      }, 400);
    } else {
      if (debounceTimer) clearTimeout(debounceTimer);
      void persistConfig(merged);
    }
  }

  function updateAgentIcon(agentName: string, iconId: string | null) {
    const current = config();
    if (!current) return;
    const lower = agentName.toLowerCase().trim();
    const nextIcons: Record<string, string> = { ...(current.agent_icons ?? {}) };
    if (iconId === null) {
      delete nextIcons[lower];
      delete nextIcons[agentName];
    } else {
      nextIcons[lower] = iconId;
    }
    patch({ agent_icons: nextIcons });
  }

  const knownAgentsList = () => {
    const cfg = config();
    const set = new Map<string, { name: string; custom: boolean; command?: string }>();

    const BUILTINS = [
      "claude-code",
      "cursor",
      "aider",
      "codex",
      "antigravity",
      "opencode",
    ];
    for (const b of BUILTINS) {
      set.set(b, { name: b, custom: false });
    }

    for (const a of agents()) {
      const lower = a.name.toLowerCase().trim();
      if (!set.has(lower)) {
        set.set(lower, { name: a.name, custom: a.custom, command: a.command });
      }
    }

    if (cfg?.agents && typeof cfg.agents === "object") {
      for (const [name, cmd] of Object.entries(cfg.agents as Record<string, string>)) {
        const lower = name.toLowerCase().trim();
        if (!set.has(lower)) {
          set.set(lower, { name, custom: true, command: cmd });
        }
      }
    }

    if (cfg?.agent_icons && typeof cfg.agent_icons === "object") {
      for (const name of Object.keys(cfg.agent_icons)) {
        const lower = name.toLowerCase().trim();
        if (!set.has(lower)) {
          set.set(lower, { name, custom: true });
        }
      }
    }

    return Array.from(set.values());
  };

  const customAgentsList = createMemo(() => {
    const list: Array<{ name: string; command: string; isDefault: boolean; detected: boolean }> = [];
    const cfg = config();
    const detectMap = new Map(agents().map((a) => [a.name, a]));

    if (cfg?.agents && typeof cfg.agents === "object") {
      for (const [name, cmd] of Object.entries(cfg.agents as Record<string, string>)) {
        const detectedInfo = detectMap.get(name);
        const isDef = Boolean(cfg.default_agent === name || detectedInfo?.default);
        list.push({
          name,
          command: cmd,
          isDefault: isDef,
          detected: detectedInfo ? detectedInfo.detected : true,
        });
      }
    }

    for (const a of agents()) {
      if (a.custom && !list.some((item) => item.name === a.name)) {
        list.push({
          name: a.name,
          command: a.command,
          isDefault: Boolean(a.default || cfg?.default_agent === a.name),
          detected: a.detected,
        });
      }
    }

    return list;
  });

  const filteredIcons = () => {
    const q = iconFilter().toLowerCase().trim();
    if (!q) return AGENT_ICON_CATALOG;
    return AGENT_ICON_CATALOG.filter(
      (entry) => entry.id.toLowerCase().includes(q) || entry.label.toLowerCase().includes(q) || entry.category.toLowerCase().includes(q)
    );
  };

  async function checkForUpdates() {
    setChecking(true);
    setError(null);
    setProgress(null);
    setAvailableUpdate(null);
    setStatus("Checking for updates…");
    try {
      const update = await checkForUpdate();
      setAvailableUpdate(update);
      if (update) props.onUpdateAvailable?.(update.version);
      setStatus(update ? `Repomon ${update.version} is available.` : "Repomon is up to date.");
    } catch (cause) {
      setStatus(null);
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setChecking(false);
    }
  }

  async function installUpdate() {
    const update = availableUpdate();
    if (!update) return;
    setChecking(true);
    setError(null);
    setStatus(`Downloading Repomon ${update.version}…`);
    try {
      await update.install((value) => {
        setProgress(value);
        setStatus(`Downloading Repomon ${value.version}…`);
      });
    } catch (cause) {
      setStatus(null);
      setError(cause instanceof Error ? cause.message : String(cause));
      setChecking(false);
    }
  }

  const footer = (): JSX.Element => (
    <>
      <div class="mr-auto flex items-center gap-2">
        <Show when={saveStatus() === "saving"}>
          <span class="flex items-center gap-1.5 font-mono text-[11px] text-muted">
            <IconRefresh size={11} class="animate-spin text-signal" />
            <span>Saving…</span>
          </span>
        </Show>
        <Show when={saveStatus() === "saved"}>
          <span class="flex items-center gap-1.5 font-mono text-[11px] text-signal transition-opacity">
            <IconCheck size={12} strokeWidth={2.5} />
            <span>Saved</span>
          </span>
        </Show>
        <Show when={status()}>
          <span class="text-xs text-muted">{status()}</span>
        </Show>
      </div>
      <button
        type="button"
        class="focus-ring rounded-lg border border-line bg-surface px-4 py-1.5 text-xs font-medium text-foreground transition-colors hover:bg-raised"
        onClick={props.onClose}
      >
        Done
      </button>
    </>
  );

  return (
    <Modal title="Settings" subtitle="Preferences are stored by the daemon and shared with the TUI." width="min(46rem, 95vw)" onClose={props.onClose} footer={footer()}>
      <div class="sticky -top-4 z-10 -mx-5 -mt-4 mb-5 border-b border-line bg-surface/95 px-5 pt-3 pb-2.5 backdrop-blur">
        <div class="flex items-center gap-1 rounded-lg border border-line bg-raised/50 p-0.5" role="tablist" aria-label="Settings sections">
          <For each={TABS}>
            {(item) => (
              <button
                type="button"
                role="tab"
                aria-selected={tab() === item.id}
                class={`focus-ring flex-1 rounded-md py-1 text-center text-xs font-medium transition-colors ${
                  tab() === item.id ? "bg-surface text-foreground shadow-xs font-semibold" : "text-muted hover:text-foreground"
                }`}
                onClick={() => {
                  setTab(item.id);
                  setPickerAgent(null);
                }}
              >
                {item.label}
              </button>
            )}
          </For>
        </div>
      </div>

      <Show when={error()}>
        <p class="mb-4 rounded-xl border border-fault/30 bg-fault/8 p-3 text-xs text-fault">{error()}</p>
      </Show>
      <Show when={config()} fallback={<p class="text-xs text-muted">Loading settings…</p>}>
        {(settings) => (
          <div class="space-y-6">
            <Show when={tab() === "general"}>
              <section class="space-y-4">
                <p class="section-label">General Configuration</p>
                <Select
                  label="Default Agent Runtime"
                  value={String(settings().default_agent ?? "")}
                  options={agentSelectOptions(agents(), settings().default_agent)}
                  onChange={(value) => patch({ default_agent: value || null })}
                />
                <TextField label="Worktree Template" value={settings().worktree_template} onInput={(value) => patch({ worktree_template: value }, true)} />
                <TextField label="Auto-continue Message" value={settings().auto_continue_message} onInput={(value) => patch({ auto_continue_message: value }, true)} />
                <div class="grid gap-2 sm:grid-cols-2">
                  <Switch
                    label="Auto-collapse lanes with no active agent"
                    checked={autoCollapseEmpty()}
                    onChange={(value) => {
                      setAutoCollapseEmpty(value);
                      saveAutoCollapseEmptyLanes(value);
                    }}
                  />
                  <For each={GENERAL_TOGGLES}>
                    {([key, label]) => <Switch label={label} checked={Boolean(settings()[key])} onChange={(value) => patch({ [key]: value } as Partial<ConfigView>)} />}
                  </For>
                </div>
              </section>

              <section class="space-y-3 border-t border-line/70 pt-5">
                <p class="section-label">Repomind Orchestration</p>
                <Select
                  label="Repomind Orchestrator Runtime"
                  value={String(settings().orchestrator_agent ?? "")}
                  options={agentSelectOptions(agents(), settings().orchestrator_agent)}
                  onChange={(value) => patch({ orchestrator_agent: value || null })}
                />
                <TextField
                  label="Repomind Model"
                  value={String(settings().orchestrator_model ?? "")}
                  placeholder="opus / sonnet"
                  onInput={(value) => patch({ orchestrator_model: value || null }, true)}
                />
              </section>

              <section class="space-y-3 border-t border-line/70 pt-5">
                <p class="section-label">Software Updates</p>
                <div class="flex items-center gap-3">
                  <button
                    type="button"
                    class="focus-ring rounded-lg border border-line bg-surface px-3.5 py-1.5 text-xs font-medium text-foreground transition-colors hover:bg-raised disabled:opacity-50"
                    disabled={checking()}
                    onClick={() => void checkForUpdates()}
                  >
                    {checking() ? "Checking…" : "Check for updates"}
                  </button>
                  <Show when={availableUpdate()}>
                    {(update) => (
                      <button
                        type="button"
                        class="focus-ring rounded-lg bg-signal px-3.5 py-1.5 text-xs font-semibold text-background transition-colors hover:bg-signal/90 disabled:opacity-50"
                        disabled={checking()}
                        onClick={() => void installUpdate()}
                      >
                        Install {update().version} & restart
                      </button>
                    )}
                  </Show>
                  <Show when={progress()?.total}>
                    <progress class="h-1.5 flex-1 accent-signal" max={progress()!.total} value={progress()!.downloaded} />
                  </Show>
                </div>
              </section>

              <section class="space-y-3 border-t border-line/70 pt-5">
                <p class="section-label">Onboarding & Setup</p>
                <div class="flex items-center justify-between gap-3">
                  <div class="min-w-0">
                    <p class="text-xs font-medium text-foreground">Welcome Guide</p>
                    <p class="mt-0.5 text-[11px] text-muted leading-snug">
                      Replay the first-run onboarding wizard to review system setup, add repositories, and explore Repomon workflows.
                    </p>
                  </div>
                  <button
                    type="button"
                    class="focus-ring shrink-0 rounded-lg border border-line bg-surface px-3.5 py-1.5 text-xs font-medium text-foreground transition-colors hover:bg-raised cursor-pointer"
                    onClick={() => {
                      props.onClose();
                      props.onReplayOnboarding?.();
                    }}
                  >
                    Replay Onboarding
                  </button>
                </div>
              </section>

              <Show when={props.fleet}>
                <section class="space-y-4 border-t border-line/70 pt-5">
                  <p class="section-label">Daemon & Recovery</p>

                  {/* Stop / Start Daemon */}
                  <div class="rounded-xl border border-line/80 bg-surface/50 p-4 space-y-3">
                    <div class="flex items-center justify-between gap-3">
                      <div class="min-w-0">
                        <p class="text-xs font-medium text-foreground">Background Daemon</p>
                        <p class="mt-0.5 text-[11px] text-muted leading-snug">
                          The daemon streams terminal state and manages agent sessions.
                          {daemonInfo()?.service_managed
                            ? " Running as a system service (launchd/systemd)."
                            : " Running as an unmanaged background process."}
                        </p>
                      </div>
                      <div class="flex items-center gap-2 shrink-0">
                        <Show when={daemonInfo()}>
                          <span class={`inline-flex items-center gap-1.5 rounded-md px-2 py-0.5 text-[10px] font-medium ${
                            daemonInfo()!.service_managed
                              ? "bg-signal/10 text-signal"
                              : "bg-muted/10 text-muted"
                          }`}>
                            {daemonInfo()!.service_managed ? "service" : "process"}
                          </span>
                        </Show>
                        <button
                          type="button"
                          class="focus-ring rounded-lg border border-line bg-surface px-3.5 py-1.5 text-xs font-medium text-foreground transition-colors hover:bg-raised disabled:opacity-50"
                          disabled={daemonBusy()}
                          onClick={async () => {
                            setDaemonBusy(true);
                            try {
                              const { stopDaemon } = await import("../ipc/daemonControl");
                              // TODO: determine current phase from connection store;
                              // for now always offer stop since modal is only reachable when connected
                              await stopDaemon();
                              setDaemonInfo((prev) => prev ? { ...prev, status: "stopped" } : prev);
                            } catch (e) {
                              setError(e instanceof Error ? e.message : String(e));
                            } finally {
                              setDaemonBusy(false);
                            }
                          }}
                        >
                          {daemonBusy() ? "Working…" : "Stop Daemon"}
                        </button>
                        <button
                          type="button"
                          class="focus-ring rounded-lg border border-signal/40 bg-signal/10 px-3.5 py-1.5 text-xs font-medium text-signal transition-colors hover:bg-signal/20 disabled:opacity-50"
                          disabled={daemonBusy()}
                          onClick={async () => {
                            setDaemonBusy(true);
                            try {
                              const { startDaemon } = await import("../ipc/daemonControl");
                              await startDaemon();
                              setDaemonInfo((prev) => prev ? { ...prev, status: "running" } : prev);
                            } catch (e) {
                              setError(e instanceof Error ? e.message : String(e));
                            } finally {
                              setDaemonBusy(false);
                            }
                          }}
                        >
                          Start Daemon
                        </button>
                      </div>
                    </div>
                  </div>

                  {/* Reset Daemon */}
                  <div class="rounded-xl border border-line/80 bg-surface/50 p-4 space-y-3">
                    <div class="flex items-center justify-between gap-3">
                      <div class="min-w-0">
                        <p class="text-xs font-medium text-foreground">Reset Daemon</p>
                        <p class="mt-0.5 text-[11px] text-muted leading-snug">
                          Restart the background daemon and clear all in-memory caches (overlay state, prompt caches,
                          known sessions). Use when agents stop showing up or the sidebar looks stale.
                        </p>
                      </div>
                      <Show when={!confirmRestart()} fallback={
                        <div class="flex items-center gap-2 shrink-0">
                          <span class="text-[11px] text-attention">Lanes will briefly disconnect.</span>
                          <button
                            type="button"
                            class="focus-ring rounded-lg border border-attention/40 bg-attention/10 px-3.5 py-1.5 text-xs font-semibold text-attention transition-colors hover:bg-attention/20 disabled:opacity-50"
                            disabled={daemonBusy()}
                            onClick={async () => {
                              setDaemonBusy(true);
                              setConfirmRestart(false);
                              try {
                                const { restartDaemon } = await import("../ipc/daemonControl");
                                await restartDaemon();
                              } catch (e) {
                                setError(e instanceof Error ? e.message : String(e));
                              } finally {
                                setDaemonBusy(false);
                              }
                            }}
                          >
                            {daemonBusy() ? "Restarting…" : "Confirm Reset"}
                          </button>
                          <button
                            type="button"
                            class="focus-ring rounded-lg border border-line bg-surface px-3 py-1.5 text-xs font-medium text-muted transition-colors hover:bg-raised"
                            onClick={() => setConfirmRestart(false)}
                          >
                            Cancel
                          </button>
                        </div>
                      }>
                        <button
                          type="button"
                          class="focus-ring rounded-lg border border-attention/40 bg-attention/10 px-3.5 py-1.5 text-xs font-medium text-attention transition-colors hover:bg-attention/20 disabled:opacity-50 shrink-0"
                          disabled={daemonBusy()}
                          onClick={() => setConfirmRestart(true)}
                        >
                          Reset Daemon
                        </button>
                      </Show>
                    </div>
                  </div>

                  {/* Restore Orphaned Agents */}
                  <div class="rounded-xl border border-line/80 bg-surface/50 p-4 space-y-3">
                    <div class="flex items-center justify-between gap-3">
                      <div class="min-w-0">
                        <p class="text-xs font-medium text-foreground">Restore Orphaned Agents</p>
                        <p class="mt-0.5 text-[11px] text-muted leading-snug">
                          Re-adopt external or disconnected agent sessions back into tmux management.
                          Use after a tmux crash or when agents disappear from the sidebar.
                        </p>
                      </div>
                      <div class="flex items-center gap-2 shrink-0">
                        <Show when={orphanCount() > 0}>
                          <span class="inline-flex items-center rounded-md bg-attention/10 px-2 py-0.5 text-[10px] font-medium text-attention">
                            {orphanCount()} orphaned
                          </span>
                        </Show>
                        <Show when={restoreResult()}>
                          <span class="text-[11px] text-signal">{restoreResult()}</span>
                        </Show>
                        <button
                          type="button"
                          class="focus-ring rounded-lg border border-signal/40 bg-signal/10 px-3.5 py-1.5 text-xs font-medium text-signal transition-colors hover:bg-signal/20 disabled:opacity-50"
                          disabled={restoreBusy() || orphanCount() === 0}
                          onClick={async () => {
                            if (!props.actions) return;
                            setRestoreBusy(true);
                            setRestoreResult(null);
                            try {
                              const restored = await props.actions.restoreAllAgents();
                              setRestoreResult(`Restored ${restored} session${restored !== 1 ? "s" : ""}`);
                              setTimeout(() => setRestoreResult(null), 5000);
                            } catch (e) {
                              setError(e instanceof Error ? e.message : String(e));
                            } finally {
                              setRestoreBusy(false);
                            }
                          }}
                        >
                          {restoreBusy()
                            ? "Restoring…"
                            : orphanCount() === 0
                              ? "No orphans detected"
                              : `Restore ${orphanCount()} Agent${orphanCount() !== 1 ? "s" : ""}`}
                        </button>
                      </div>
                    </div>
                  </div>
                </section>
              </Show>
            </Show>

            <Show when={tab() === "system"}>
              <SystemHealthView onConfigureCustomAgents={() => setTab("agents")} />
            </Show>

            <Show when={tab() === "agents"}>
              <div class="space-y-6">
                {/* SECTION 1: CUSTOM AGENTS */}
                <section class="space-y-4">
                  <div>
                    <p class="section-label">Custom Agent Registrations</p>
                    <p class="mt-0.5 text-xs text-muted">
                      Register custom coding agent CLIs with arbitrary arguments to spawn in git worktree lanes.
                    </p>
                  </div>

                  {/* Add Agent Form */}
                  <form
                    class="rounded-xl border border-line bg-surface/50 p-4 space-y-3"
                    onSubmit={addCustomAgent}
                    aria-label="Register custom agent"
                  >
                    <div class="flex items-center justify-between">
                      <span class="text-xs font-semibold text-foreground">Register New Agent</span>
                      <Show when={customAgentSuccess()}>
                        {(msg) => (
                          <span class="text-xs text-emerald-500 font-medium animate-in fade-in flex items-center gap-1">
                            <IconCheck size={12} />
                            {msg()}
                          </span>
                        )}
                      </Show>
                    </div>

                    <div class="grid gap-3 sm:grid-cols-2">
                      <label class="block">
                        <span class="text-[11px] font-medium text-muted block mb-1">Agent Name / Kind</span>
                        <input
                          type="text"
                          class="focus-ring h-8 w-full rounded-lg border border-line bg-surface px-3 text-xs text-foreground outline-none placeholder:text-muted/60"
                          placeholder="e.g. 'devin', 'gemini-cli', 'deepseek'"
                          value={customAgentName()}
                          onInput={(e) => {
                            setCustomAgentName(e.currentTarget.value);
                            if (customAgentError()) setCustomAgentError(null);
                          }}
                        />
                      </label>

                      <label class="block">
                        <span class="text-[11px] font-medium text-muted block mb-1">Launch Command</span>
                        <input
                          type="text"
                          class="focus-ring h-8 w-full rounded-lg border border-line bg-surface px-3 text-xs font-mono text-foreground outline-none placeholder:text-muted/60"
                          placeholder="e.g. 'gemini --repomon', 'python run.py'"
                          value={customAgentCommand()}
                          onInput={(e) => {
                            setCustomAgentCommand(e.currentTarget.value);
                            if (customAgentError()) setCustomAgentError(null);
                          }}
                        />
                      </label>
                    </div>

                    {/* Inline Validation Feedback */}
                    <Show when={customAgentError()}>
                      {(errMsg) => (
                        <div
                          role="alert"
                          class="flex items-start justify-between gap-2 rounded-lg border border-fault/30 bg-fault/10 p-2.5 text-xs text-fault animate-in fade-in"
                        >
                          <div class="flex items-center gap-1.5">
                            <span class="font-medium">{errMsg()}</span>
                          </div>
                          <button
                            type="button"
                            class="text-fault/70 hover:text-fault p-0.5 cursor-pointer"
                            onClick={() => setCustomAgentError(null)}
                            aria-label="Dismiss error"
                          >
                            <IconClose size={12} />
                          </button>
                        </div>
                      )}
                    </Show>

                    <div class="flex items-center justify-end pt-1">
                      <button
                        type="submit"
                        class="focus-ring inline-flex items-center gap-1.5 rounded-lg bg-signal px-4 py-1.5 text-xs font-semibold text-background transition-colors hover:bg-signal/90 disabled:opacity-50 cursor-pointer"
                        disabled={customAgentBusy() || !customAgentName().trim() || !customAgentCommand().trim()}
                      >
                        <IconPlus size={13} />
                        <span>{customAgentBusy() ? "Registering…" : "Register Agent"}</span>
                      </button>
                    </div>
                  </form>

                  {/* Custom Agents List */}
                  <div class="space-y-2.5">
                    <Show
                      when={customAgentsList().length > 0}
                      fallback={
                        <div class="rounded-xl border border-dashed border-line/80 bg-surface/20 p-5 text-center space-y-1">
                          <p class="text-xs font-medium text-foreground">No custom agents registered</p>
                          <p class="text-[11px] text-muted max-w-md mx-auto">
                            Add a custom CLI above to launch specialized scripts, local models, or proprietary agent binaries alongside built-in runtimes.
                          </p>
                        </div>
                      }
                    >
                      <For each={customAgentsList()}>
                        {(agent) => {
                          const currentKey = () =>
                            settings().agent_icons?.[agent.name.toLowerCase()] ??
                            settings().agent_icons?.[agent.name] ??
                            resolveAgentIconKey(agent.name);
                          const isDefault = () =>
                            Boolean(settings().default_agent === agent.name || agent.isDefault);

                          return (
                            <div class="flex flex-col sm:flex-row sm:items-center justify-between gap-3 rounded-xl border border-line/80 bg-surface/50 p-3.5 transition-colors hover:bg-surface/80">
                              <div class="flex items-center gap-3 min-w-0">
                                <div class="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-line bg-raised/70 text-foreground">
                                  <AgentIcon agent={agent.name} iconKey={currentKey()} size={18} />
                                </div>
                                <div class="min-w-0 space-y-1">
                                  <div class="flex items-center gap-2 flex-wrap">
                                    <span class="truncate font-semibold text-xs text-foreground">{agent.name}</span>
                                    <span class="rounded px-1.5 py-0.2 text-[9.5px] font-mono uppercase tracking-wider bg-amber-500/10 text-amber-400 border border-amber-500/20">
                                      Custom
                                    </span>
                                    <Show when={isDefault()}>
                                      <span class="flex items-center gap-1 rounded bg-signal/15 border border-signal/30 px-1.5 py-0.2 text-[9.5px] font-mono text-signal font-medium">
                                        <span class="size-1 rounded-full bg-signal" />
                                        Default Agent
                                      </span>
                                    </Show>
                                  </div>
                                  <div class="flex items-center gap-1.5">
                                    <span class="text-[10.5px] text-muted font-medium">Command:</span>
                                    <code class="font-mono text-[10.5px] text-foreground bg-surface px-1.5 py-0.2 rounded border border-line select-all truncate max-w-xs">
                                      {agent.command}
                                    </code>
                                  </div>
                                </div>
                              </div>

                              <div class="flex items-center gap-1.5 shrink-0 self-end sm:self-center">
                                <Show when={!isDefault()}>
                                  <button
                                    type="button"
                                    class="focus-ring inline-flex items-center gap-1 rounded-lg border border-line bg-surface px-2.5 py-1 text-xs text-muted transition-colors hover:bg-raised hover:text-foreground cursor-pointer"
                                    onClick={() => setDefaultAgent(agent.name)}
                                    title="Set as default agent for new lanes and spawns"
                                    aria-label={`Set ${agent.name} as default agent`}
                                  >
                                    <span>Set as Default</span>
                                  </button>
                                </Show>
                                <button
                                  type="button"
                                  class="focus-ring inline-flex items-center gap-1 rounded-lg border border-line bg-surface px-2.5 py-1 text-xs font-medium text-foreground transition-colors hover:bg-raised cursor-pointer"
                                  onClick={() => {
                                    setPickerAgent(agent.name);
                                    setIconFilter("");
                                  }}
                                  aria-label={`Change icon for ${agent.name}`}
                                >
                                  <span>Change Icon…</span>
                                </button>
                                <button
                                  type="button"
                                  class="focus-ring inline-flex items-center gap-1 rounded-lg border border-fault/30 bg-fault/5 px-2.5 py-1 text-xs text-fault transition-colors hover:bg-fault/15 hover:border-fault/50 cursor-pointer"
                                  onClick={() => removeCustomAgent(agent.name)}
                                  title={`Remove custom agent ${agent.name}`}
                                  aria-label={`Remove agent ${agent.name}`}
                                >
                                  <IconTrash size={12} />
                                  <span>Remove</span>
                                </button>
                              </div>
                            </div>
                          );
                        }}
                      </For>
                    </Show>
                  </div>
                </section>

                {/* SECTION 2: BUILT-IN RUNTIMES & ICON OVERRIDES */}
                <section class="space-y-4 border-t border-line/70 pt-5">
                  <div class="flex items-center justify-between">
                    <div>
                      <p class="section-label">Built-in Runtime Icons & Overrides</p>
                      <p class="mt-0.5 text-xs text-muted">
                        Customize geometric visual identities for built-in runtimes (Claude Code, Cursor, Antigravity, OpenCode, Codex, Aider).
                      </p>
                    </div>
                  </div>

                  <div class="space-y-2">
                    <For each={knownAgentsList().filter((a) => !a.custom)}>
                      {(agent) => {
                        const currentKey = () =>
                          settings().agent_icons?.[agent.name.toLowerCase()] ??
                          settings().agent_icons?.[agent.name] ??
                          resolveAgentIconKey(agent.name);
                        const hasOverride = () =>
                          Boolean(
                            settings().agent_icons?.[agent.name.toLowerCase()] ||
                              settings().agent_icons?.[agent.name]
                          );
                        const currentEntry = () =>
                          AGENT_ICON_CATALOG.find((c) => c.id === currentKey());
                        const isDefault = () =>
                          Boolean(settings().default_agent === agent.name);

                        return (
                          <div class="flex items-center justify-between rounded-xl border border-line/80 bg-surface/50 p-3 transition-colors hover:bg-surface/80">
                            <div class="flex items-center gap-3 min-w-0">
                              <div class="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-line bg-raised/70 text-foreground">
                                <AgentIcon agent={agent.name} iconKey={currentKey()} size={18} />
                              </div>
                              <div class="min-w-0">
                                <div class="flex items-center gap-2">
                                  <span class="truncate font-medium text-xs text-foreground">{agent.name}</span>
                                  <span class="rounded bg-surface px-1.5 py-0.2 text-[9.5px] font-mono uppercase tracking-wider text-muted border border-line">
                                    Built-in
                                  </span>
                                  <Show when={isDefault()}>
                                    <span class="flex items-center gap-1 rounded bg-signal/15 border border-signal/30 px-1.5 py-0.2 text-[9.5px] font-mono text-signal font-medium">
                                      <span class="size-1 rounded-full bg-signal" />
                                      Default
                                    </span>
                                  </Show>
                                </div>
                                <p class="truncate text-[11px] text-muted">
                                  {hasOverride() ? (
                                    <span class="text-signal font-medium">
                                      Custom icon: {currentEntry()?.label ?? currentKey()}
                                    </span>
                                  ) : (
                                    <span>Default: {currentEntry()?.label ?? currentKey()}</span>
                                  )}
                                </p>
                              </div>
                            </div>

                            <div class="flex items-center gap-2 shrink-0">
                              <Show when={!isDefault()}>
                                <button
                                  type="button"
                                  class="focus-ring inline-flex items-center gap-1 rounded-lg border border-line bg-surface px-2.5 py-1 text-xs text-muted transition-colors hover:bg-raised hover:text-foreground cursor-pointer"
                                  onClick={() => setDefaultAgent(agent.name)}
                                  title="Set as default agent for new lanes and spawns"
                                  aria-label={`Set ${agent.name} as default agent`}
                                >
                                  <span>Set as Default</span>
                                </button>
                              </Show>
                              <Show when={hasOverride()}>
                                <button
                                  type="button"
                                  class="focus-ring inline-flex items-center gap-1 rounded-lg border border-line/60 bg-surface px-2.5 py-1 text-xs text-muted transition-colors hover:bg-raised hover:text-foreground cursor-pointer"
                                  onClick={() => updateAgentIcon(agent.name, null)}
                                  title="Reset to default icon"
                                >
                                  <IconRefresh size={12} />
                                  <span>Reset</span>
                                </button>
                              </Show>
                              <button
                                type="button"
                                class="focus-ring inline-flex items-center gap-1.5 rounded-lg border border-line bg-surface px-3 py-1 text-xs font-medium text-foreground transition-colors hover:bg-raised cursor-pointer"
                                onClick={() => {
                                  setPickerAgent(agent.name);
                                  setIconFilter("");
                                }}
                                aria-label={`Change icon for ${agent.name}`}
                              >
                                <span>Change Icon…</span>
                              </button>
                            </div>
                          </div>
                        );
                      }}
                    </For>
                  </div>
                </section>
              </div>
            </Show>

            <Show when={tab() === "notifications"}>
              <section class="space-y-4">
                <p class="section-label">Alert Notifications</p>
                <Switch label="Enable notifications" checked={settings().notify_enabled} onChange={(value) => patch({ notify_enabled: value })} />
                <div class="grid gap-2 sm:grid-cols-2">
                  <For each={NOTIFY_TOGGLES}>
                    {([key, label]) => <Switch label={label} checked={Boolean(settings()[key])} disabled={!settings().notify_enabled} onChange={(value) => patch({ [key]: value } as Partial<ConfigView>)} />}
                  </For>
                </div>
                <div class="space-y-3 border-t border-line/70 pt-5">
                  <p class="section-label">Audio Cues</p>
                  <div class="grid gap-2 sm:grid-cols-2">
                    <Switch
                      label="Play sound cues"
                      checked={settings().notify_sound}
                      disabled={!settings().notify_enabled}
                      onChange={(value) => patch({ notify_sound: value })}
                    />
                    <Switch
                      label="Only while unfocused"
                      checked={settings().notify_sound_unfocused_only}
                      disabled={!settings().notify_enabled || !settings().notify_sound}
                      onChange={(value) => patch({ notify_sound_unfocused_only: value })}
                    />
                  </div>
                  <label class="block">
                    <span class="section-label">Volume ({Math.round(settings().notify_sound_volume * 100)}%)</span>
                    <input
                      class="mt-2 w-full accent-signal"
                      type="range"
                      min="0"
                      max="1"
                      step="0.05"
                      value={settings().notify_sound_volume}
                      disabled={!settings().notify_enabled || !settings().notify_sound}
                      onInput={(event) => patch({ notify_sound_volume: Number(event.currentTarget.value) })}
                      onChange={(event) => patch({ notify_sound_volume: Number(event.currentTarget.value) })}
                    />
                  </label>
                  <div class="space-y-2 border-t border-line/70 pt-4">
                    <div>
                      <p class="section-label">Sound Profile</p>
                      <p class="mt-0.5 text-xs text-muted">
                        Select the acoustic personality and synthesizer contour for all notification sound cues.
                      </p>
                    </div>
                    <div class="grid grid-cols-1 gap-2 sm:grid-cols-2 lg:grid-cols-3">
                      <For each={SOUND_PROFILES}>
                        {(prof) => {
                          const isSelected = () => activeSoundProfile() === prof.id;
                          return (
                            <button
                              type="button"
                              class={`focus-ring flex flex-col rounded-xl border p-2.5 text-left transition-all ${
                                isSelected()
                                  ? "border-signal bg-signal/5 ring-1 ring-signal/30"
                                  : "border-line bg-surface hover:bg-raised/50"
                              }`}
                              onClick={() => {
                                setActiveSoundProfile(prof.id);
                                saveSoundProfile(prof.id);
                                props.onPreviewSound?.("agent-finished", settings().notify_sound_volume, prof.id);
                              }}
                            >
                              <div class="flex items-center justify-between">
                                <span class="text-xs font-semibold text-foreground">{prof.name}</span>
                                <Show when={isSelected()}>
                                  <span class="text-signal"><IconCheck size={13} /></span>
                                </Show>
                              </div>
                              <span class="mt-1 text-[11px] text-muted leading-snug">{prof.description}</span>
                            </button>
                          );
                        }}
                      </For>
                    </div>
                  </div>

                  <div class="grid gap-2 sm:grid-cols-2">
                    <For each={SOUND_CUE_CONTROLS}>
                      {(item) => (
                        <div class="flex items-center gap-2 rounded-lg border border-line bg-surface/50 p-2">
                          <div class="min-w-0 flex-1">
                            <Switch
                              label={item.label}
                              checked={Boolean(settings()[item.key])}
                              disabled={!settings().notify_enabled || !settings().notify_sound}
                              onChange={(value) => patch({ [item.key]: value } as Partial<ConfigView>)}
                            />
                          </div>
                          <button
                            type="button"
                            class="focus-ring rounded-md border border-line bg-surface px-2 py-1 text-[11px] font-medium text-muted transition-colors hover:bg-raised hover:text-foreground"
                            aria-label={`Preview ${item.label}`}
                            onClick={() => props.onPreviewSound?.(item.cue, settings().notify_sound_volume, activeSoundProfile())}
                          >
                            Play
                          </button>
                        </div>
                      )}
                    </For>
                  </div>
                </div>
              </section>
            </Show>

            <Show when={tab() === "appearance"}>
              <div class="space-y-6">
                {/* 1. Theme Presets */}
                <section class="space-y-3">
                  <div>
                    <p class="section-label">Color Themes & Presets</p>
                    <p class="mt-0.5 text-xs text-muted">
                      Select a theme palette for the application, terminal panes, and interactive chrome.
                    </p>
                  </div>

                  <div class="grid grid-cols-1 gap-2.5 sm:grid-cols-2 lg:grid-cols-3">
                    <For each={THEME_PRESETS}>
                      {(preset) => {
                        const isSelected = () => currentTheme() === preset.id;
                        return (
                          <button
                            type="button"
                            class={`focus-ring relative flex flex-col rounded-xl border p-3 text-left transition-all ${
                              isSelected()
                                ? "border-signal bg-signal/5 shadow-xs ring-1 ring-signal/30"
                                : "border-line bg-surface hover:border-line hover:bg-raised/40"
                            }`}
                            onClick={() => selectTheme(preset.id)}
                          >
                            {/* Miniature Color Swatch Preview */}
                            <div
                              class="mb-2.5 flex h-10 w-full items-center justify-between rounded-lg border px-3"
                              style={{
                                "background-color": preset.preview.bg,
                                "border-color": preset.preview.line,
                              }}
                            >
                              <div class="flex items-center gap-1.5">
                                <span
                                  class="size-2.5 rounded-full"
                                  style={{ "background-color": preset.preview.signal }}
                                />
                                <span
                                  class="h-2 w-8 rounded-sm"
                                  style={{ "background-color": preset.preview.surface }}
                                />
                              </div>
                              <span
                                class="h-2 w-12 rounded-sm"
                                style={{ "background-color": preset.preview.line }}
                              />
                            </div>

                            <div class="flex items-center justify-between">
                              <span class="text-xs font-semibold text-foreground">
                                {preset.name}
                              </span>
                              <Show when={isSelected()}>
                                <span class="text-signal">
                                  <IconCheck size={14} />
                                </span>
                              </Show>
                            </div>
                            <span class="mt-0.5 text-[11px] leading-snug text-muted line-clamp-2">
                              {preset.description}
                            </span>
                          </button>
                        );
                      }}
                    </For>
                  </div>
                </section>

                {/* 2. Accent Color Choice */}
                <section class="space-y-3 border-t border-line/70 pt-5">
                  <div>
                    <p class="section-label">Brand & Accent Color</p>
                    <p class="mt-0.5 text-xs text-muted">
                      Customizes the primary highlight color used for badges, buttons, active focus rings, and terminal cursors.
                    </p>
                  </div>

                  <div class="flex flex-wrap items-center gap-2">
                    <For each={ACCENT_SWATCHES}>
                      {(swatch) => {
                        const isSelected = () => (settings().accent?.toLowerCase() ?? "cyan") === swatch.id;
                        return (
                          <button
                            type="button"
                            class={`focus-ring relative flex items-center gap-1.5 rounded-lg border px-2.5 py-1.5 text-xs font-medium transition-all ${
                              isSelected()
                                ? "border-signal bg-raised text-foreground shadow-xs font-semibold"
                                : "border-line bg-surface text-muted hover:bg-raised hover:text-foreground"
                            }`}
                            onClick={() => selectAccent(swatch.id)}
                            aria-pressed={isSelected()}
                          >
                            <span
                              class="size-3 shrink-0 rounded-full border border-black/10 dark:border-white/10"
                              style={{ "background-color": swatch.color }}
                            />
                            <span>{swatch.label}</span>
                            <Show when={isSelected()}>
                              <span class="ml-0.5 text-signal">
                                <IconCheck size={12} />
                              </span>
                            </Show>
                          </button>
                        );
                      }}
                    </For>
                  </div>

                  <div class="mt-2 max-w-xs">
                    <ColorField
                      label="Custom Accent Color (Hex)"
                      value={String(settings().accent ?? "")}
                      onChange={(value) => {
                        patch({ accent: value });
                        applyAccent(value);
                      }}
                    />
                  </div>
                </section>

                {/* 3. Terminal Pane Customization & Background Tint */}
                <section class="space-y-3 border-t border-line/70 pt-5">
                  <div>
                    <p class="section-label">Terminal Pane Appearance</p>
                    <p class="mt-0.5 text-xs text-muted">
                      Optionally tint agent terminal backgrounds using the theme accent, and configure font styling.
                    </p>
                  </div>

                  <div class="grid gap-3 sm:grid-cols-2">
                    <Switch
                      label="Tint terminal background with accent"
                      checked={terminalApp().tintEnabled}
                      onChange={(value) => {
                        const next = { ...terminalApp(), tintEnabled: value };
                        setTerminalApp(next);
                        saveTerminalAppearance(next);
                      }}
                    />
                    <label class="block">
                      <span class="text-xs text-muted">Tint Intensity ({Math.round(terminalApp().tintOpacity * 100)}%)</span>
                      <input
                        class="mt-1.5 w-full accent-signal"
                        type="range"
                        min="0.01"
                        max="0.25"
                        step="0.01"
                        value={terminalApp().tintOpacity}
                        disabled={!terminalApp().tintEnabled}
                        onInput={(e) => {
                          const next = { ...terminalApp(), tintOpacity: Number(e.currentTarget.value) };
                          setTerminalApp(next);
                          saveTerminalAppearance(next);
                        }}
                      />
                    </label>
                  </div>

                  <div class="grid gap-3 sm:grid-cols-2">
                    <label class="block">
                      <span class="text-xs text-muted">Terminal Font Family</span>
                      <select
                        class="mt-1 w-full rounded-lg border border-line bg-surface px-3 py-1.5 text-xs text-foreground focus:border-signal outline-none"
                        value={terminalApp().fontFamily}
                        onChange={(e) => {
                          const next = { ...terminalApp(), fontFamily: e.currentTarget.value };
                          setTerminalApp(next);
                          saveTerminalAppearance(next);
                        }}
                      >
                        <For each={TERMINAL_FONT_FAMILIES}>
                          {(font) => <option value={font.id}>{font.label}</option>}
                        </For>
                      </select>
                    </label>

                    <label class="block">
                      <span class="text-xs text-muted">Terminal Font Size ({terminalApp().fontSize}px)</span>
                      <input
                        class="mt-1.5 w-full accent-signal"
                        type="range"
                        min="10"
                        max="18"
                        step="1"
                        value={terminalApp().fontSize}
                        onInput={(e) => {
                          const next = { ...terminalApp(), fontSize: Number(e.currentTarget.value) };
                          setTerminalApp(next);
                          saveTerminalAppearance(next);
                        }}
                      />
                    </label>
                  </div>
                </section>

                {/* 4. Layout & Visual Preferences */}
                <section class="space-y-3 border-t border-line/70 pt-5">
                  <p class="section-label">Sidebar & Layout</p>
                  <Switch
                    label="Sort projects by activity"
                    checked={Boolean(settings().sort_repos_by_activity)}
                    onChange={(value) => patch({ sort_repos_by_activity: value })}
                  />
                </section>
              </div>
            </Show>

            <Show when={tab() === "automation"}>
              <AutomationSettings />
            </Show>

            <Show when={tab() === "keyboard"}>
              <section class="space-y-3">
                <p class="section-label">Keyboard Shortcuts</p>
                <KeyboardHelp />
              </section>
            </Show>
          </div>
        )}
      </Show>

      {/* Interactive Icon Picker Modal */}
      <Show when={pickerAgent()}>
        {(targetAgent) => {
          const currentKey = () =>
            config()?.agent_icons?.[targetAgent().toLowerCase()] ??
            config()?.agent_icons?.[targetAgent()] ??
            resolveAgentIconKey(targetAgent());

          return (
            <div
              class="fixed inset-0 z-50 flex items-center justify-center bg-background/80 p-4 backdrop-blur-xs"
              onClick={(e) => {
                if (e.target === e.currentTarget) setPickerAgent(null);
              }}
            >
              <div class="focus-ring flex max-h-[85vh] w-full max-w-lg flex-col rounded-2xl border border-line bg-surface shadow-2xl overflow-hidden">
                <div class="flex items-center justify-between border-b border-line px-5 py-3.5">
                  <div class="flex items-center gap-2.5">
                    <div class="flex h-7 w-7 items-center justify-center rounded-md border border-line bg-raised text-foreground">
                      <AgentIcon agent={targetAgent()} iconKey={currentKey()} size={16} />
                    </div>
                    <div>
                      <h3 class="text-xs font-semibold text-foreground">
                        Icon for <span class="text-signal">{targetAgent()}</span>
                      </h3>
                      <p class="text-[11px] text-muted">Choose any abstract geometric symbol from the library.</p>
                    </div>
                  </div>
                  <button
                    type="button"
                    class="focus-ring rounded-lg p-1.5 text-muted transition-colors hover:bg-raised hover:text-foreground"
                    onClick={() => setPickerAgent(null)}
                    aria-label="Close icon picker"
                  >
                    <IconClose size={14} />
                  </button>
                </div>

                <div class="border-b border-line bg-raised/30 px-4 py-2.5">
                  <div class="flex items-center gap-2 rounded-lg border border-line bg-surface px-2.5 py-1 text-xs">
                    <IconSearch size={13} class="text-muted" />
                    <input
                      type="text"
                      placeholder="Search 26 geometric icons…"
                      class="w-full bg-transparent text-xs text-foreground outline-none placeholder:text-muted/60"
                      value={iconFilter()}
                      onInput={(e) => setIconFilter(e.currentTarget.value)}
                    />
                    <Show when={iconFilter()}>
                      <button
                        type="button"
                        class="text-muted hover:text-foreground"
                        onClick={() => setIconFilter("")}
                      >
                        <IconClose size={12} />
                      </button>
                    </Show>
                  </div>
                </div>

                <div class="flex-1 overflow-y-auto p-4">
                  <div class="grid grid-cols-3 gap-2.5 sm:grid-cols-4 md:grid-cols-6">
                    <For each={filteredIcons()}>
                      {(entry) => {
                        const isSelected = () => currentKey() === entry.id;
                        const EntryIcon = entry.Icon;
                        return (
                          <button
                            type="button"
                            class={`focus-ring flex flex-col items-center justify-center rounded-xl border p-2.5 text-center transition-all ${
                              isSelected()
                                ? "border-signal bg-signal/10 text-signal shadow-xs"
                                : "border-line bg-surface/70 text-muted hover:border-line hover:bg-raised hover:text-foreground"
                            }`}
                            onClick={() => {
                              updateAgentIcon(targetAgent(), entry.id);
                              setPickerAgent(null);
                            }}
                          >
                            <div class="mb-1.5 flex h-8 w-8 items-center justify-center">
                              <EntryIcon size={20} />
                            </div>
                            <span class="line-clamp-1 w-full text-[10px] font-medium leading-tight">
                              {entry.label}
                            </span>
                            <Show when={isSelected()}>
                              <span class="mt-1 inline-flex items-center gap-0.5 text-[9px] font-semibold text-signal">
                                <IconCheck size={10} /> Active
                              </span>
                            </Show>
                          </button>
                        );
                      }}
                    </For>
                  </div>
                  <Show when={!filteredIcons().length}>
                    <p class="py-8 text-center text-xs text-muted">No matching icons found.</p>
                  </Show>
                </div>

                <div class="flex items-center justify-between border-t border-line bg-raised/40 px-4 py-3">
                  <button
                    type="button"
                    class="focus-ring rounded-lg border border-line bg-surface px-3 py-1.5 text-xs text-muted transition-colors hover:bg-raised hover:text-foreground"
                    onClick={() => {
                      updateAgentIcon(targetAgent(), null);
                      setPickerAgent(null);
                    }}
                  >
                    Reset to Default
                  </button>
                  <button
                    type="button"
                    class="focus-ring rounded-lg bg-surface border border-line px-4 py-1.5 text-xs font-medium text-foreground transition-colors hover:bg-raised"
                    onClick={() => setPickerAgent(null)}
                  >
                    Done
                  </button>
                </div>
              </div>
            </div>
          );
        }}
      </Show>
    </Modal>
  );
}
