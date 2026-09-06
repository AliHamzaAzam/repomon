import { For, Show, createSignal, onCleanup, onMount } from "solid-js";

import type { ApprovalRule, PolicyAction, SupervisionConfig } from "../bindings";
import { daemonCall, subscribeDaemon, type ConfigView } from "../ipc/rpc";
import { formatChord } from "../keymap";
import {
  groupApprovalRules,
  SUPERVISION_ACTION_OPTIONS,
  SUPERVISION_DIALOG_CLASSES,
  supervisionClassActionColor,
  updatedSupervisionClasses,
} from "./automation";
import Select from "./controls/Select";
import Switch from "./controls/Switch";
import { IconCheck, IconClose, IconRefresh } from "./icons";

/// The sub-tabs of Settings > Policies. Exported so a caller elsewhere in the app can send the
/// operator straight to one of them instead of describing where to click.
export type PolicySection = "approvals" | "supervision";

interface PolicySettingsProps {
  /// Which sub-tab to open on. Read once, at mount, so a later change does not yank the operator
  /// off whichever one they moved to.
  initialSection?: PolicySection;
}

/** Edits standing permission rules while operational Repomind state remains in its panel. */
export default function PolicySettings(props: PolicySettingsProps) {
  const [activeSubTab, setActiveSubTab] = createSignal<PolicySection>(
    props.initialSection ?? "approvals",
  );
  const [approvals, setApprovals] = createSignal<ApprovalRule[]>([]);
  const [busy, setBusy] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);

  const [supervision, setSupervision] = createSignal<SupervisionConfig | null>(null);
  const [supervisionSaveStatus, setSupervisionSaveStatus] = createSignal<"idle" | "saving" | "saved" | "error">("idle");
  let supervisionSaveTimer: ReturnType<typeof setTimeout> | undefined;
  let supervisionDebounceTimer: ReturnType<typeof setTimeout> | undefined;

  async function loadData() {
    try {
      const [appRes, cfgRes] = await Promise.all([
        daemonCall("approval.list").catch(() => ({ rules: [] })),
        daemonCall("config.get").catch(() => null),
      ]);
      setApprovals(appRes.rules ?? []);
      if (cfgRes) setSupervision(cfgRes.supervision ?? null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function persistSupervision(next: SupervisionConfig) {
    setSupervisionSaveStatus("saving");
    try {
      const saved = await daemonCall("config.set", { supervision: next });
      setSupervision(saved.supervision);
      setSupervisionSaveStatus("saved");
      if (supervisionSaveTimer) clearTimeout(supervisionSaveTimer);
      supervisionSaveTimer = setTimeout(() => setSupervisionSaveStatus("idle"), 2000);
    } catch (cause) {
      setSupervisionSaveStatus("error");
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  // Optimistic patch + debounce, mirroring SettingsModal's patch() for text/number fields;
  // switches and selects go through the instant (non-debounced) path.
  function patchSupervision(next: Partial<SupervisionConfig>, debounce = false) {
    const current = supervision();
    if (!current) return;
    const merged: SupervisionConfig = { ...current, ...next };
    setSupervision(merged);

    if (debounce) {
      if (supervisionDebounceTimer) clearTimeout(supervisionDebounceTimer);
      setSupervisionSaveStatus("saving");
      supervisionDebounceTimer = setTimeout(() => {
        supervisionDebounceTimer = undefined;
        void persistSupervision(merged);
      }, 400);
    } else {
      if (supervisionDebounceTimer) clearTimeout(supervisionDebounceTimer);
      supervisionDebounceTimer = undefined;
      void persistSupervision(merged);
    }
  }

  onMount(() => {
    void loadData();

    let active = true;
    let stopConfig: (() => void) | undefined;
    void subscribeDaemon((event) => {
      if (!active) return;
      if (event.method === "event.config.changed") {
        const params = event.params as Partial<ConfigView> | null;
        // Skip while a local edit is still debouncing so we don't clobber unsaved input.
        if (params?.supervision && !supervisionDebounceTimer) {
          setSupervision(params.supervision);
        }
      }
    })
      .then((unsub) => {
        if (active) stopConfig = unsub;
        else unsub();
      })
      .catch(() => undefined);

    onCleanup(() => {
      active = false;
      stopConfig?.();
      if (supervisionSaveTimer) clearTimeout(supervisionSaveTimer);
      if (supervisionDebounceTimer) clearTimeout(supervisionDebounceTimer);
    });
  });

  async function revokeApproval(rule: ApprovalRule) {
    setBusy(rule.pattern);
    setError(null);
    try {
      await daemonCall("approval.remove", { pattern: rule.pattern, repo: rule.repo });
      await loadData();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div class="space-y-5">
      <div class="flex items-center justify-between">
        <div>
          <p class="section-label">Standing rules</p>
          <p class="text-xs text-muted">
            What Repomon may do on its own: which commands are pre-approved, and what happens to a
            permission prompt nobody is watching.
          </p>
        </div>
        <button
          type="button"
          class="focus-ring flex items-center gap-1.5 rounded-lg border border-line bg-raised/70 px-2.5 py-1 text-xs text-muted hover:bg-raised hover:text-foreground"
          onClick={() => void loadData()}
          title="Refresh policy state"
        >
          <IconRefresh size={12} class={busy() ? "animate-spin" : ""} />
          <span>Refresh</span>
        </button>
      </div>

      <Show when={error()}>
        {(msg) => (
          <div class="flex items-center justify-between rounded-xl border border-fault/40 bg-fault/10 px-3.5 py-2.5 text-xs text-fault">
            <span>{msg()}</span>
            <button type="button" onClick={() => setError(null)} class="text-fault/80 hover:text-fault">
              <IconClose size={14} />
            </button>
          </div>
        )}
      </Show>

      <div class="flex gap-1.5 rounded-xl border border-line bg-raised/30 p-1">
        <button
          type="button"
          class={`focus-ring flex-1 rounded-lg py-1.5 text-xs font-medium transition-colors ${
            activeSubTab() === "approvals"
              ? "bg-surface text-foreground shadow-xs font-semibold"
              : "text-muted hover:text-foreground"
          }`}
          onClick={() => setActiveSubTab("approvals")}
        >
          Approvals ({approvals().length})
        </button>
        <button
          type="button"
          class={`focus-ring flex-1 rounded-lg py-1.5 text-xs font-medium transition-colors ${
            activeSubTab() === "supervision"
              ? "bg-surface text-foreground shadow-xs font-semibold"
              : "text-muted hover:text-foreground"
          }`}
          onClick={() => setActiveSubTab("supervision")}
        >
          Supervision
        </button>
      </div>

      <Show when={activeSubTab() === "approvals"}>
        <div class="space-y-4">
          <p class="text-xs text-muted">
            Command patterns Repomind is allowed to execute automatically. Destructive operations (e.g. force pushes, <code>rm -rf</code>, hard resets) always require manual confirmation.
          </p>

          <For each={groupApprovalRules(approvals())} fallback={<p class="py-6 text-center text-xs text-muted">No auto-approval rules configured.</p>}>
            {(group) => (
              <div class="space-y-2">
                <p class="section-label">{group.repo}</p>
                <div class="space-y-1.5">
                  <For each={group.rules}>
                    {(rule) => (
                      <div class="flex items-center justify-between gap-3 rounded-xl border border-line bg-raised/20 px-3 py-2">
                        <code class="truncate font-mono text-xs text-foreground/90">{rule.pattern}</code>
                        <button
                          type="button"
                          class="focus-ring shrink-0 rounded-lg border border-line bg-surface px-2.5 py-0.5 text-xs text-muted hover:border-fault/40 hover:text-fault"
                          onClick={() => void revokeApproval(rule)}
                        >
                          Revoke
                        </button>
                      </div>
                    )}
                  </For>
                </div>
              </div>
            )}
          </For>
        </div>
      </Show>

      <Show when={activeSubTab() === "supervision"}>
        <Show
          when={supervision()}
          fallback={<p class="py-6 text-center text-xs text-muted">Loading supervision defaults…</p>}
        >
          {(sup) => (
            <div class="space-y-4">
              <div class="flex items-start justify-between gap-3">
                <p class="text-xs text-muted">
                  Global defaults for automated permission handling. Individual lanes can override these in their
                  own Supervision panel.
                </p>
                <Show when={supervisionSaveStatus() === "saving"}>
                  <span class="flex shrink-0 items-center gap-1.5 font-mono text-[11px] text-muted">
                    <IconRefresh size={11} class="animate-spin text-signal" />
                    <span>Saving…</span>
                  </span>
                </Show>
                <Show when={supervisionSaveStatus() === "saved"}>
                  <span class="flex shrink-0 items-center gap-1.5 font-mono text-[11px] text-signal">
                    <IconCheck size={12} strokeWidth={2.5} />
                    <span>Saved</span>
                  </span>
                </Show>
              </div>

              <div class="space-y-1.5">
                <Switch
                  label="Enable supervision"
                  checked={sup().enabled}
                  onChange={(value) => patchSupervision({ enabled: value })}
                />
                <p class="text-[11px] text-muted">
                  Lanes must also opt in individually before Repomon will act on their behalf.
                </p>
              </div>

              <div class="space-y-2">
                <div>
                  <p class="section-label">Default permission policies</p>
                  <p class="mt-0.5 text-[11px] text-muted">Applied to any lane that has not set its own override.</p>
                </div>
                <div class="space-y-1.5 rounded-xl border border-line bg-raised/20 p-2.5">
                  <For each={SUPERVISION_DIALOG_CLASSES}>
                    {(cls) => {
                      const action = () => sup().classes?.[cls.id] ?? "hold";
                      return (
                        <div class="flex items-center justify-between gap-2 rounded-lg bg-surface/60 px-2.5 py-1.5">
                          <span class="truncate text-xs font-medium text-foreground">{cls.label}</span>
                          <div class="shrink-0">
                            <Select
                              ariaLabel={`${cls.label} default policy`}
                              size="sm"
                              options={SUPERVISION_ACTION_OPTIONS}
                              value={action()}
                              class={`w-36 ${supervisionClassActionColor(action())}`}
                              onChange={(val) =>
                                patchSupervision({
                                  classes: updatedSupervisionClasses(sup().classes, cls.id, val as PolicyAction),
                                })
                              }
                            />
                          </div>
                        </div>
                      );
                    }}
                  </For>
                </div>
              </div>

              <div class="space-y-3 rounded-xl border border-line bg-raised/20 p-3">
                <div>
                  <p class="section-label">Delivery and thresholds</p>
                  <p class="mt-0.5 text-[11px] text-muted">
                    Defaults for nudge messaging and stall detection.
                  </p>
                </div>

                <div class="space-y-1.5">
                  <span class="section-label block">Default nudge message text</span>
                  <input
                    type="text"
                    placeholder="Repomon: checking in on this lane."
                    class="focus-ring w-full rounded-lg border border-line bg-surface px-3 py-1.5 text-xs text-foreground outline-none placeholder:text-muted/60"
                    value={sup().nudge_text}
                    onInput={(e) => patchSupervision({ nudge_text: e.currentTarget.value }, true)}
                  />
                </div>

                <div class="grid grid-cols-2 gap-2.5">
                  <div class="space-y-1.5">
                    <span class="section-label block">Default stall threshold (mins)</span>
                    <input
                      type="number"
                      min="1"
                      max="1440"
                      class="focus-ring w-full rounded-lg border border-line bg-surface px-2.5 py-1.5 font-mono text-xs text-foreground outline-none"
                      value={sup().stall_mins}
                      onInput={(e) => patchSupervision({ stall_mins: Number(e.currentTarget.value) }, true)}
                    />
                  </div>

                  <div class="space-y-1.5">
                    <span class="section-label block">Default max nudge retries</span>
                    <input
                      type="number"
                      min="0"
                      max="10"
                      class="focus-ring w-full rounded-lg border border-line bg-surface px-2.5 py-1.5 font-mono text-xs text-foreground outline-none"
                      value={sup().nudge_retries}
                      onInput={(e) => patchSupervision({ nudge_retries: Number(e.currentTarget.value) }, true)}
                    />
                  </div>
                </div>
              </div>

              <p class="text-[11px] text-muted">Per-lane overrides live in the lane's Supervision panel.</p>
            </div>
          )}
        </Show>
      </Show>

      <p class="rounded-xl border border-line bg-raised/20 px-3.5 py-2.5 text-xs text-muted">
        Playbooks, standing duties, and the journal live in the Repomind panel,{" "}
        <span class="font-mono text-foreground">{formatChord("mod+9")}</span>.
      </p>
    </div>
  );
}
