import { For, Show, createEffect, createSignal, onMount, type JSX } from "solid-js";

import type { AgentChoice } from "../bindings";
import type { SoundCue } from "../audio/sound";
import { daemonCall, type ConfigView } from "../ipc/rpc";
import { checkForUpdate, type AvailableUpdate, type UpdateProgress } from "../ipc/updater";
import { applyAccent } from "../theme";
import ColorField from "./controls/ColorField";
import Select from "./controls/Select";
import Switch from "./controls/Switch";
import KeyboardHelp from "./KeyboardHelp";
import Modal from "./Modal";

export type SettingsTab = "general" | "notifications" | "appearance" | "keyboard";

interface SettingsModalProps {
  onClose: () => void;
  initialTab?: SettingsTab;
  onConfigSaved?: (config: ConfigView) => void;
  onPreviewSound?: (cue: SoundCue, volume: number) => boolean;
  onUpdateAvailable?: (version: string) => void;
}

const TABS: Array<{ id: SettingsTab; label: string }> = [
  { id: "general", label: "General" },
  { id: "notifications", label: "Notifications" },
  { id: "appearance", label: "Appearance" },
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
  const [saving, setSaving] = createSignal(false);
  const [checking, setChecking] = createSignal(false);
  const [progress, setProgress] = createSignal<UpdateProgress | null>(null);
  const [availableUpdate, setAvailableUpdate] = createSignal<AvailableUpdate | null>(null);

  onMount(() => {
    void daemonCall("config.get").then(setConfig).catch((cause: unknown) => {
      setError(cause instanceof Error ? cause.message : String(cause));
    });
    void daemonCall("agent.detect").then(setAgents).catch(() => undefined);
  });

  function patch(next: Partial<ConfigView>) {
    const current = config();
    if (current) setConfig({ ...current, ...next });
  }

  async function save() {
    const current = config();
    if (!current) return;
    setSaving(true);
    setError(null);
    setStatus(null);
    try {
      const saved = await daemonCall("config.set", current);
      setConfig(saved);
      props.onConfigSaved?.(saved);
      applyAccent(saved.accent);
      setStatus("Settings saved.");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }

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
      <Show when={status()}>
        <span class="mr-auto text-xs text-muted">{status()}</span>
      </Show>
      <button
        type="button"
        class="focus-ring rounded-lg border border-line bg-surface px-3.5 py-1.5 text-xs font-medium text-muted transition-colors hover:bg-raised hover:text-foreground"
        onClick={props.onClose}
      >
        Close
      </button>
      <button
        type="button"
        class="focus-ring rounded-lg bg-signal px-4 py-1.5 text-xs font-semibold text-background transition-colors hover:bg-signal/90 disabled:opacity-50"
        disabled={saving() || !config()}
        onClick={() => void save()}
      >
        {saving() ? "Saving…" : "Save"}
      </button>
    </>
  );

  return (
    <Modal title="Settings" subtitle="Preferences are stored by the daemon and shared with the TUI." width="min(44rem, 95vw)" onClose={props.onClose} footer={footer()}>
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
                onClick={() => setTab(item.id)}
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
                <TextField label="Worktree Template" value={settings().worktree_template} onInput={(value) => patch({ worktree_template: value })} />
                <TextField label="Auto-continue Message" value={settings().auto_continue_message} onInput={(value) => patch({ auto_continue_message: value })} />
                <div class="grid gap-2 sm:grid-cols-2">
                  <For each={GENERAL_TOGGLES}>
                    {([key, label]) => <Switch label={label} checked={Boolean(settings()[key])} onChange={(value) => patch({ [key]: value } as Partial<ConfigView>)} />}
                  </For>
                </div>
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
                    />
                  </label>
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
                            onClick={() => props.onPreviewSound?.(item.cue, settings().notify_sound_volume)}
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
              <section class="space-y-4">
                <p class="section-label">Visual Preferences</p>
                <Switch
                  label="Sort projects by activity"
                  checked={Boolean(settings().sort_repos_by_activity)}
                  onChange={(value) => patch({ sort_repos_by_activity: value })}
                />
                <ColorField label="Accent Color" value={String(settings().accent ?? "")} onChange={(value) => patch({ accent: value })} />
                <Select
                  label="Repomind Orchestrator Runtime"
                  value={String(settings().orchestrator_agent ?? "")}
                  options={agentSelectOptions(agents(), settings().orchestrator_agent)}
                  onChange={(value) => patch({ orchestrator_agent: value || null })}
                />
                <TextField label="Repomind Model" value={String(settings().orchestrator_model ?? "")} placeholder="opus / sonnet" onInput={(value) => patch({ orchestrator_model: value || null })} />
              </section>
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
    </Modal>
  );
}
