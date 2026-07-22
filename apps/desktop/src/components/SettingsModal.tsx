import { For, Show, createSignal, onMount, type JSX } from "solid-js";

import type { AgentChoice } from "../bindings";
import { daemonCall, type ConfigView } from "../ipc/rpc";
import { checkForUpdate, type AvailableUpdate, type UpdateProgress } from "../ipc/updater";
import { applyAccent } from "../theme";
import Modal from "./Modal";

interface SettingsModalProps {
  onClose: () => void;
}

const NOTIFY_TOGGLES: Array<[keyof ConfigView, string]> = [
  ["notify_needs_you", "Needs you"],
  ["notify_rate_limited", "Rate limited"],
  ["notify_resumed", "Resumed"],
  ["notify_idle", "Idle / ended"],
  ["notify_sound", "Play sound"],
  ["notify_show_why", "Show why (last message)"],
  ["notify_coalesce", "Coalesce bursts"],
  ["notify_click_focus", "Click to focus"],
  ["notify_subagents", "Include subagents"],
];

const GENERAL_TOGGLES: Array<[keyof ConfigView, string]> = [
  ["auto_continue", "Auto-continue rate-limited agents"],
  ["spawn_prompt", "Prompt for agent on spawn"],
  ["usage_probe", "Probe account usage"],
  ["expand_agents", "Expand multi-agent lanes"],
  ["embedded_pty", "Embedded terminal renderer"],
];

function Toggle(props: { label: string; checked: boolean; disabled?: boolean; onChange: (value: boolean) => void }) {
  return (
    <label class="flex items-center justify-between rounded border border-line px-3 py-2 text-xs" classList={{ "opacity-50": props.disabled }}>
      <span>{props.label}</span>
      <input
        type="checkbox"
        class="accent-signal"
        checked={props.checked}
        disabled={props.disabled}
        onChange={(event) => props.onChange(event.currentTarget.checked)}
      />
    </label>
  );
}

function TextField(props: {
  label: string;
  value: string;
  onInput: (value: string) => void;
  placeholder?: string;
  list?: string;
}) {
  return (
    <label class="block">
      <span class="section-label">{props.label}</span>
      <input
        class="settings-input"
        value={props.value}
        placeholder={props.placeholder}
        list={props.list}
        onInput={(event) => props.onInput(event.currentTarget.value)}
      />
    </label>
  );
}

export default function SettingsModal(props: SettingsModalProps) {
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
      <button type="button" class="focus-ring rounded border border-line px-3 py-2 text-xs text-muted" onClick={props.onClose}>
        Close
      </button>
      <button
        type="button"
        class="focus-ring rounded bg-signal px-4 py-2 font-mono text-[0.6rem] font-semibold uppercase text-background disabled:opacity-50"
        disabled={saving() || !config()}
        onClick={() => void save()}
      >
        {saving() ? "Saving…" : "Save"}
      </button>
    </>
  );

  return (
    <Modal title="Settings" subtitle="Preferences are stored by the daemon and shared with the TUI." width="min(44rem, 95vw)" onClose={props.onClose} footer={footer()}>
      <Show when={error()}>
        <p class="mb-4 rounded-md border border-fault/40 bg-fault/8 p-2 text-xs text-fault">{error()}</p>
      </Show>
      <Show when={config()} fallback={<p class="text-sm text-muted">Loading settings…</p>}>
        {(settings) => (
          <div class="space-y-6">
            <datalist id="agent-choices">
              <For each={agents()}>{(choice) => <option value={choice.name} />}</For>
            </datalist>

            <section class="space-y-3">
              <p class="section-label text-signal">General</p>
              <div class="grid gap-3 sm:grid-cols-2">
                <TextField label="Accent" value={String(settings().accent ?? "")} placeholder="cyan or #rrggbb or mono" onInput={(value) => patch({ accent: value })} />
                <TextField label="Default agent" value={String(settings().default_agent ?? "")} list="agent-choices" placeholder="claude-code" onInput={(value) => patch({ default_agent: value || null })} />
              </div>
              <TextField label="Worktree template" value={settings().worktree_template} onInput={(value) => patch({ worktree_template: value })} />
              <TextField label="Auto-continue message" value={settings().auto_continue_message} onInput={(value) => patch({ auto_continue_message: value })} />
              <div class="grid gap-2 sm:grid-cols-2">
                <For each={GENERAL_TOGGLES}>
                  {([key, label]) => <Toggle label={label} checked={Boolean(settings()[key])} onChange={(value) => patch({ [key]: value } as Partial<ConfigView>)} />}
                </For>
              </div>
            </section>

            <section class="space-y-3">
              <p class="section-label text-signal">Notifications</p>
              <Toggle label="Enable notifications" checked={settings().notify_enabled} onChange={(value) => patch({ notify_enabled: value })} />
              <div class="grid gap-2 sm:grid-cols-2">
                <For each={NOTIFY_TOGGLES}>
                  {([key, label]) => <Toggle label={label} checked={Boolean(settings()[key])} disabled={!settings().notify_enabled} onChange={(value) => patch({ [key]: value } as Partial<ConfigView>)} />}
                </For>
              </div>
            </section>

            <section class="space-y-3">
              <p class="section-label text-signal">Orchestrator</p>
              <div class="grid gap-3 sm:grid-cols-2">
                <TextField label="Repomind agent" value={String(settings().orchestrator_agent ?? "")} list="agent-choices" placeholder="claude" onInput={(value) => patch({ orchestrator_agent: value || null })} />
                <TextField label="Repomind model" value={String(settings().orchestrator_model ?? "")} placeholder="opus / sonnet" onInput={(value) => patch({ orchestrator_model: value || null })} />
              </div>
            </section>

            <section class="space-y-3 border-t border-line pt-4">
              <p class="section-label text-signal">Updates</p>
              <div class="flex items-center gap-3">
                <button type="button" class="focus-ring rounded border border-line px-3 py-2 font-mono text-[0.58rem] uppercase text-muted disabled:opacity-50" disabled={checking()} onClick={() => void checkForUpdates()}>
                  {checking() ? "Checking…" : "Check for updates"}
                </button>
                <Show when={availableUpdate()}>
                  {(update) => (
                    <button type="button" class="focus-ring rounded bg-signal px-3 py-2 font-mono text-[0.58rem] font-semibold uppercase text-background disabled:opacity-50" disabled={checking()} onClick={() => void installUpdate()}>
                      Install {update().version} and restart
                    </button>
                  )}
                </Show>
                <Show when={progress()?.total}>
                  <progress class="h-1.5 flex-1 accent-signal" max={progress()!.total} value={progress()!.downloaded} />
                </Show>
              </div>
            </section>
          </div>
        )}
      </Show>
    </Modal>
  );
}
