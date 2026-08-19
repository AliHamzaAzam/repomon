import { For, Show, createEffect, createSignal, onCleanup, onMount, type JSX } from "solid-js";

import type {
  DialogClass,
  Lane,
  MailDeliveryMode,
  PolicyAction,
  SupervisionConfig,
  SupervisionEntry,
  SupervisionOverrides,
  SupervisionPolicy,
} from "../bindings";
import Select, { type SelectOption } from "./controls/Select";
import Switch from "./controls/Switch";
import { translateError, type TranslatedError } from "../ipc/errors";
import { daemonCall, subscribeDaemon } from "../ipc/rpc";
import type { ActionsStore } from "../stores/actions";
import type { FleetStore } from "../stores/fleet";
import { formatTime } from "./automation";
import {
  IconChevronDown,
  IconGitBranch,
  IconRefresh,
  IconShield,
} from "./icons";

export const DIALOG_CLASSES: Array<{ id: DialogClass; label: string; description: string }> = [
  { id: "command_exec", label: "Command execution", description: "Terminal bash and shell execution" },
  { id: "file_write", label: "File modification", description: "Creating, editing, or replacing files" },
  { id: "deletion", label: "File deletion", description: "Removing files or worktrees" },
  { id: "network_access", label: "Network access", description: "Outbound network requests and fetches" },
  { id: "credential_access", label: "Credential access", description: "Reading secrets, auth tokens, and keys" },
  { id: "push_remote", label: "Push to remote", description: "Git push and remote branch mutations" },
  { id: "install", label: "Package installation", description: "Installing package dependencies" },
  { id: "device_access", label: "Device access", description: "Interacting with hardware or external devices" },
  { id: "unknown", label: "Other dialogs", description: "Unclassified or ambiguous prompts" },
];

const ACTION_OPTIONS: SelectOption[] = [
  { value: "auto_approve", label: "Auto-approve" },
  { value: "auto_deny", label: "Auto-deny" },
  { value: "hold", label: "Hold for human" },
];

const MAIL_MODE_OPTIONS: SelectOption[] = [
  { value: "nudge", label: "Nudge" },
  { value: "full_body", label: "Full body" },
];

export interface SupervisionPanelProps {
  fleet?: FleetStore;
  actions?: ActionsStore;
}

export default function SupervisionPanel(props: SupervisionPanelProps): JSX.Element {
  const lane = () => props.fleet?.selectedLane();

  const [defaults, setDefaults] = createSignal<SupervisionConfig | null>(null);
  const [laneOverrides, setLaneOverrides] = createSignal<SupervisionOverrides | null>(null);
  const [effectivePolicy, setEffectivePolicy] = createSignal<SupervisionPolicy | null>(null);
  const [auditEntries, setAuditEntries] = createSignal<SupervisionEntry[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<TranslatedError | null>(null);
  const [lastActed, setLastActed] = createSignal<SupervisionEntry | null>(null);
  const [actedRecent, setActedRecent] = createSignal(false);
  const [expandedEntries, setExpandedEntries] = createSignal<Set<number>>(new Set());

  const [nudgeDraft, setNudgeDraft] = createSignal("");
  const [stallDraft, setStallDraft] = createSignal<number>(15);
  const [retriesDraft, setRetriesDraft] = createSignal<number>(2);

  let actedTimer: ReturnType<typeof setTimeout> | undefined;

  async function loadData(laneId: number) {
    setLoading(true);
    setError(null);
    try {
      const [getRes, auditRes] = await Promise.all([
        daemonCall("supervision.get", { lane_id: laneId }),
        daemonCall("supervision.audit", { lane_id: laneId, limit: 50 }).catch(() => ({ entries: [] })),
      ]);
      setDefaults(getRes.defaults);
      setLaneOverrides(getRes.lane);
      setEffectivePolicy(getRes.effective);
      setAuditEntries(auditRes.entries ?? []);

      const effective = getRes.effective;
      if (effective) {
        setNudgeDraft(getRes.lane?.nudge_text ?? effective.nudge_text);
        setStallDraft(getRes.lane?.stall_mins ?? effective.stall_mins);
        setRetriesDraft(getRes.lane?.nudge_retries ?? effective.nudge_retries);
      }
    } catch (cause) {
      setError(translateError(cause));
    } finally {
      setLoading(false);
    }
  }

  let lastLaneId: number | null = null;
  createEffect(() => {
    const l = lane();
    const id = l?.id ?? null;
    if (id === lastLaneId) return;
    lastLaneId = id;
    if (id != null) {
      void loadData(id);
    } else {
      setDefaults(null);
      setLaneOverrides(null);
      setEffectivePolicy(null);
      setAuditEntries([]);
    }
  });

  onMount(() => {
    let active = true;
    let stop: (() => void) | undefined;

    void subscribeDaemon((event) => {
      if (!active) return;
      const currentLane = lane();
      if (!currentLane) return;

      if (event.method === "event.supervision.acted") {
        const entry = event.params as SupervisionEntry | null;
        if (entry && entry.lane_id === currentLane.id) {
          setAuditEntries((prev) => {
            if (prev.some((e) => e.id === entry.id)) return prev;
            return [entry, ...prev].slice(0, 200);
          });
          setLastActed(entry);
          setActedRecent(true);
          if (actedTimer) clearTimeout(actedTimer);
          actedTimer = setTimeout(() => {
            if (active) setActedRecent(false);
          }, 4000);
        }
      } else if (event.method === "event.supervision.changed") {
        const p = event.params as { lane_id?: number } | null;
        if (p && p.lane_id === currentLane.id) {
          void loadData(currentLane.id);
        }
      }
    })
      .then((unsub) => {
        if (active) stop = unsub;
        else unsub();
      })
      .catch(() => undefined);

    onCleanup(() => {
      active = false;
      stop?.();
      if (actedTimer) clearTimeout(actedTimer);
    });
  });

  async function toggleLaneEnabled(enabled: boolean) {
    const currentLane = lane();
    if (!currentLane) return;
    try {
      const res = await daemonCall("supervision.set", {
        lane_id: currentLane.id,
        enabled,
      });
      setEffectivePolicy(res.effective);
      setLaneOverrides((prev) =>
        prev
          ? { ...prev, enabled }
          : {
              lane_id: currentLane.id,
              enabled,
              classes: {},
              mail_mode: null,
              nudge_text: null,
              stall_mins: null,
              nudge_retries: null,
              expect_work: false,
              updated_at: new Date().toISOString(),
            },
      );
    } catch (cause) {
      setError(translateError(cause));
    }
  }

  async function updateClassAction(cls: DialogClass, action: PolicyAction) {
    const currentLane = lane();
    if (!currentLane) return;
    const currentClasses = laneOverrides()?.classes ?? {};
    const newClasses: Partial<Record<DialogClass, PolicyAction>> = {
      ...currentClasses,
      [cls]: action,
    };
    try {
      const res = await daemonCall("supervision.set", {
        lane_id: currentLane.id,
        classes: newClasses,
      });
      setEffectivePolicy(res.effective);
      setLaneOverrides((prev) =>
        prev
          ? { ...prev, classes: newClasses }
          : {
              lane_id: currentLane.id,
              enabled: false,
              classes: newClasses,
              mail_mode: null,
              nudge_text: null,
              stall_mins: null,
              nudge_retries: null,
              expect_work: false,
              updated_at: new Date().toISOString(),
            },
      );
    } catch (cause) {
      setError(translateError(cause));
    }
  }

  async function resetClassAction(cls: DialogClass) {
    const currentLane = lane();
    if (!currentLane) return;
    const currentClasses = { ...(laneOverrides()?.classes ?? {}) };
    delete currentClasses[cls];
    try {
      const res = await daemonCall("supervision.set", {
        lane_id: currentLane.id,
        classes: currentClasses,
      });
      setEffectivePolicy(res.effective);
      setLaneOverrides((prev) => (prev ? { ...prev, classes: currentClasses } : null));
    } catch (cause) {
      setError(translateError(cause));
    }
  }

  async function updateMailMode(mode: MailDeliveryMode) {
    const currentLane = lane();
    if (!currentLane) return;
    try {
      const res = await daemonCall("supervision.set", {
        lane_id: currentLane.id,
        mail_mode: mode,
      });
      setEffectivePolicy(res.effective);
      setLaneOverrides((prev) => (prev ? { ...prev, mail_mode: mode } : null));
    } catch (cause) {
      setError(translateError(cause));
    }
  }

  async function saveNudgeText() {
    const currentLane = lane();
    if (!currentLane) return;
    const text = nudgeDraft().trim();
    try {
      const res = await daemonCall("supervision.set", {
        lane_id: currentLane.id,
        nudge_text: text || undefined,
      });
      setEffectivePolicy(res.effective);
      setLaneOverrides((prev) => (prev ? { ...prev, nudge_text: text || null } : null));
    } catch (cause) {
      setError(translateError(cause));
    }
  }

  async function saveStallMins() {
    const currentLane = lane();
    if (!currentLane) return;
    const mins = Math.max(1, Math.min(1440, stallDraft()));
    setStallDraft(mins);
    try {
      const res = await daemonCall("supervision.set", {
        lane_id: currentLane.id,
        stall_mins: mins,
      });
      setEffectivePolicy(res.effective);
      setLaneOverrides((prev) => (prev ? { ...prev, stall_mins: mins } : null));
    } catch (cause) {
      setError(translateError(cause));
    }
  }

  async function saveNudgeRetries() {
    const currentLane = lane();
    if (!currentLane) return;
    const retries = Math.max(0, Math.min(10, retriesDraft()));
    setRetriesDraft(retries);
    try {
      const res = await daemonCall("supervision.set", {
        lane_id: currentLane.id,
        nudge_retries: retries,
      });
      setEffectivePolicy(res.effective);
      setLaneOverrides((prev) => (prev ? { ...prev, nudge_retries: retries } : null));
    } catch (cause) {
      setError(translateError(cause));
    }
  }

  async function toggleExpectWork(expect: boolean) {
    const currentLane = lane();
    if (!currentLane) return;
    try {
      const res = await daemonCall("supervision.set", {
        lane_id: currentLane.id,
        expect_work: expect,
      });
      setEffectivePolicy(res.effective);
      setLaneOverrides((prev) => (prev ? { ...prev, expect_work: expect } : null));
    } catch (cause) {
      setError(translateError(cause));
    }
  }

  function toggleExpanded(id: number) {
    setExpandedEntries((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  function refresh() {
    const currentLane = lane();
    if (currentLane) void loadData(currentLane.id);
  }

  const isMasterOff = () => defaults() !== null && !defaults()!.enabled;

  function classActionColor(action: PolicyAction): string {
    switch (action) {
      case "auto_approve":
        return "text-signal";
      case "auto_deny":
        return "text-fault";
      case "hold":
        return "text-attention";
      default:
        return "text-muted";
    }
  }

  function decisionBadgeClass(decision: string): string {
    switch (decision.toLowerCase()) {
      case "approve":
      case "auto_approve":
        return "bg-signal/15 text-signal";
      case "deny":
      case "auto_deny":
        return "bg-fault/15 text-fault";
      case "hold":
        return "bg-attention/15 text-attention";
      case "nudge":
        return "bg-raised text-muted border border-line";
      default:
        return "bg-raised text-muted";
    }
  }

  return (
    <div class="flex h-full flex-col bg-surface">
      {/* 1. Header row */}
      <div class="flex h-10 shrink-0 items-center justify-between border-b border-line bg-surface/95 px-3.5">
        <div class="flex min-w-0 items-center gap-2">
          <div class="flex items-center gap-1.5">
            <IconShield size={14} class="shrink-0 text-foreground" />
            <span class="text-xs font-semibold text-foreground">Supervision</span>
          </div>

          <Show when={lane()} keyed>
            {(l: Lane) => (
              <>
                <span class="h-3 w-px shrink-0 bg-line/60" aria-hidden="true" />
                <span class="flex min-w-0 items-center gap-1 font-mono text-[11px] text-muted">
                  <IconGitBranch size={10} class="shrink-0 text-muted/60" />
                  <span class="min-w-0 truncate">{l.worktree.branch ?? "detached"}</span>
                </span>
                <span
                  class={`size-2 rounded-full transition-colors duration-300 ${
                    actedRecent()
                      ? "bg-signal motion-safe:animate-pulse"
                      : lastActed()
                        ? "bg-signal/60"
                        : "bg-line"
                  }`}
                  title={
                    lastActed()
                      ? `Last action: ${lastActed()!.decision} (${lastActed()!.outcome}) at ${formatTime(
                          lastActed()!.at,
                        )}`
                      : "No recent supervision activity"
                  }
                  aria-label="Supervision status indicator"
                />
              </>
            )}
          </Show>
        </div>

        <button
          type="button"
          class="focus-ring flex size-6 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground disabled:opacity-40"
          onClick={refresh}
          disabled={!lane() || loading()}
          title="Refresh supervision data"
          aria-label="Refresh supervision data"
        >
          <IconRefresh size={12} class={loading() ? "animate-spin" : ""} />
        </button>
      </div>

      {/* Error display */}
      <Show when={error()} keyed>
        {(err) => (
          <div
            role="alert"
            class="m-3 mb-0 flex items-start justify-between gap-3 rounded-xl border border-fault/30 bg-fault/10 p-3 text-xs text-fault"
          >
            <div class="min-w-0">
              <p class="font-semibold">Couldn't load supervision data</p>
              <p class="mt-0.5 break-words text-fault/80">{err.friendly}</p>
            </div>
            <button
              type="button"
              class="focus-ring shrink-0 rounded-lg border border-fault/40 bg-surface px-2.5 py-1 text-xs font-medium text-foreground transition-colors hover:bg-fault/20"
              onClick={refresh}
            >
              Retry
            </button>
          </div>
        )}
      </Show>

      {/* Main content or empty state */}
      <Show
        when={lane()}
        fallback={
          <div class="flex flex-1 items-center justify-center p-4">
            <div class="max-w-[220px] space-y-2 rounded-xl border border-line bg-surface/40 p-3.5 text-center">
              <p class="text-xs font-medium text-foreground">No lane selected</p>
              <p class="text-xs text-muted">Select a lane in the fleet to view and edit its supervision policies.</p>
            </div>
          </div>
        }
      >
        <div class="min-h-0 flex-1 space-y-5 overflow-y-auto p-3.5">
          {/* 2. Master-off notice */}
          <Show when={isMasterOff()}>
            <div
              role="note"
              class="rounded-xl border border-attention/30 bg-attention/10 p-3 text-xs text-attention"
            >
              <div class="flex items-start justify-between gap-3">
                <div class="min-w-0">
                  <p class="font-semibold">Supervision is off globally.</p>
                  <p class="mt-0.5 text-attention/80">
                    Lane supervision policies will not execute while the master switch is disabled.
                  </p>
                </div>
                <Show when={props.actions?.openSettingsTab}>
                  <button
                    type="button"
                    class="focus-ring shrink-0 rounded-lg border border-attention/40 bg-surface px-2.5 py-1 text-xs font-medium text-foreground transition-colors hover:bg-attention/20"
                    onClick={() => props.actions?.openSettingsTab("automation")}
                  >
                    Open settings
                  </button>
                </Show>
              </div>
            </div>
          </Show>

          {/* 3. Lane toggle */}
          <section>
            <Switch
              label="Supervise this lane"
              checked={laneOverrides()?.enabled ?? false}
              disabled={isMasterOff() || loading()}
              onChange={toggleLaneEnabled}
            />
          </section>

          {/* 4. Policy grid */}
          <section class="space-y-2">
            <div>
              <p class="section-label">Permission policies</p>
              <p class="mt-0.5 text-[11px] text-muted">Automate or hold permission dialogs by category.</p>
            </div>

            <div class="space-y-1.5 rounded-xl border border-line bg-raised/20 p-2.5">
              <For each={DIALOG_CLASSES}>
                {(cls) => {
                  const effectiveAction = () =>
                    effectivePolicy()?.classes?.[cls.id] ?? defaults()?.classes?.[cls.id] ?? "hold";
                  const isOverridden = () => laneOverrides()?.classes?.[cls.id] !== undefined;

                  return (
                    <div class="flex items-center justify-between gap-2 rounded-lg bg-surface/60 px-2.5 py-1.5">
                      <div class="min-w-0 flex-1">
                        <div class="flex items-center gap-1.5">
                          <span class="truncate text-xs font-medium text-foreground">{cls.label}</span>
                          <Show when={isOverridden()}>
                            <span
                              class="size-1.5 shrink-0 rounded-full bg-signal"
                              title="Lane override active"
                              aria-label="Lane override active"
                            />
                            <button
                              type="button"
                              class="text-[10px] text-muted transition-colors hover:text-foreground hover:underline"
                              onClick={() => resetClassAction(cls.id)}
                              disabled={isMasterOff() || loading()}
                            >
                              Reset
                            </button>
                          </Show>
                        </div>
                      </div>

                      <div class="shrink-0">
                        <Select
                          ariaLabel={`${cls.label} policy`}
                          size="sm"
                          options={ACTION_OPTIONS}
                          value={effectiveAction()}
                          disabled={isMasterOff() || loading()}
                          class={`w-36 ${classActionColor(effectiveAction())}`}
                          onChange={(val) => updateClassAction(cls.id, val as PolicyAction)}
                        />
                      </div>
                    </div>
                  );
                }}
              </For>
            </div>
          </section>

          {/* 5. Delivery + thresholds */}
          <section class="space-y-3">
            <div>
              <p class="section-label">Delivery and thresholds</p>
              <p class="mt-0.5 text-[11px] text-muted">Mail injection mode and wake-on-mail parameters.</p>
            </div>

            <div class="space-y-3 rounded-xl border border-line bg-raised/20 p-3">
              <div class="space-y-1.5">
                <span class="section-label block">Mail delivery mode</span>
                <Select
                  ariaLabel="Mail delivery mode"
                  size="sm"
                  options={MAIL_MODE_OPTIONS}
                  value={laneOverrides()?.mail_mode ?? effectivePolicy()?.mail_mode ?? defaults()?.mail_mode ?? "nudge"}
                  disabled={isMasterOff() || loading()}
                  onChange={(val) => updateMailMode(val as MailDeliveryMode)}
                />
              </div>

              <div class="space-y-1.5">
                <span class="section-label block">Nudge message text</span>
                <input
                  type="text"
                  placeholder="Repomon: checking in on this lane."
                  class="focus-ring w-full rounded-lg border border-line bg-surface px-3 py-1.5 text-xs text-foreground placeholder:text-muted/60 disabled:opacity-50"
                  value={nudgeDraft()}
                  disabled={isMasterOff() || loading()}
                  onInput={(e) => setNudgeDraft(e.currentTarget.value)}
                  onBlur={saveNudgeText}
                />
              </div>

              <div class="grid grid-cols-2 gap-2.5">
                <div class="space-y-1.5">
                  <span class="section-label block">Stall threshold (mins)</span>
                  <input
                    type="number"
                    min="1"
                    max="1440"
                    class="focus-ring w-full rounded-lg border border-line bg-surface px-2.5 py-1.5 font-mono text-xs text-foreground disabled:opacity-50"
                    value={stallDraft()}
                    disabled={isMasterOff() || loading()}
                    onInput={(e) => setStallDraft(Number(e.currentTarget.value))}
                    onChange={saveStallMins}
                  />
                </div>

                <div class="space-y-1.5">
                  <span class="section-label block">Max nudge retries</span>
                  <input
                    type="number"
                    min="0"
                    max="10"
                    class="focus-ring w-full rounded-lg border border-line bg-surface px-2.5 py-1.5 font-mono text-xs text-foreground disabled:opacity-50"
                    value={retriesDraft()}
                    disabled={isMasterOff() || loading()}
                    onInput={(e) => setRetriesDraft(Number(e.currentTarget.value))}
                    onChange={saveNudgeRetries}
                  />
                </div>
              </div>

              <div>
                <Switch
                  label="Expect this lane to act on mail"
                  checked={laneOverrides()?.expect_work ?? effectivePolicy()?.expect_work ?? false}
                  disabled={isMasterOff() || loading()}
                  onChange={toggleExpectWork}
                />
              </div>
            </div>
          </section>

          {/* 6. Activity log */}
          <section class="space-y-2">
            <p class="section-label">Activity log</p>

            <Show
              when={auditEntries().length > 0}
              fallback={<p class="rounded-xl border border-line/60 bg-raised/10 py-6 text-center text-xs text-muted">No supervision activity yet.</p>}
            >
              <div class="space-y-2">
                <For each={auditEntries()}>
                  {(entry) => {
                    const isExpanded = () => expandedEntries().has(entry.id);
                    const hasDetails = () => Boolean(entry.pane_excerpt || (entry.keys && entry.keys.length > 0));

                    return (
                      <div class="rounded-xl border border-line bg-raised/20 p-3 text-xs">
                        <div class="flex items-center justify-between gap-2">
                          <div class="flex min-w-0 items-center gap-1.5">
                            <span class="truncate font-mono text-xs font-semibold text-foreground">{entry.trigger}</span>
                            <Show when={entry.dialog_class}>
                              {(dc) => <span class="font-mono text-[10px] text-muted">({dc()})</span>}
                            </Show>
                          </div>

                          <div class="flex shrink-0 items-center gap-1.5">
                            <span
                              class={`rounded-full px-2 py-0.5 font-mono text-[10px] font-semibold uppercase ${decisionBadgeClass(
                                entry.decision,
                              )}`}
                            >
                              {entry.decision}
                            </span>
                            <span class="font-mono text-[10px] font-semibold uppercase text-muted/70">{entry.outcome}</span>
                          </div>
                        </div>

                        <div class="mt-1 flex items-center gap-2 text-[11px] text-muted">
                          <span>{formatTime(entry.at)}</span>
                          <Show when={entry.reason ?? entry.subject}>
                            {(text) => <span class="truncate font-mono">· {text()}</span>}
                          </Show>
                        </div>

                        <Show when={hasDetails()}>
                          <button
                            type="button"
                            class="mt-1.5 flex items-center gap-1 font-mono text-[10px] text-muted transition-colors hover:text-foreground"
                            onClick={() => toggleExpanded(entry.id)}
                            aria-expanded={isExpanded()}
                          >
                            <IconChevronDown
                              size={10}
                              class={`transition-transform duration-150 ${isExpanded() ? "rotate-180" : ""}`}
                            />
                            <span>{isExpanded() ? "Hide details" : "Show details"}</span>
                          </button>

                          <Show when={isExpanded()}>
                            <div class="mt-2 space-y-1.5 border-t border-line/50 pt-2">
                              <Show when={entry.keys && entry.keys.length > 0}>
                                <div class="flex items-center gap-1.5 font-mono text-[10px] text-muted">
                                  <span class="text-muted/60">Keys:</span>
                                  <span class="rounded bg-surface px-1.5 py-0.5 text-foreground">
                                    {entry.keys!.join(", ")}
                                  </span>
                                </div>
                              </Show>
                              <Show when={entry.pane_excerpt}>
                                <pre class="max-h-36 overflow-auto whitespace-pre-wrap rounded-lg bg-surface/80 p-2 font-mono text-[10px] text-muted">
                                  {entry.pane_excerpt}
                                </pre>
                              </Show>
                            </div>
                          </Show>
                        </Show>
                      </div>
                    );
                  }}
                </For>
              </div>
            </Show>
          </section>
        </div>
      </Show>
    </div>
  );
}
