import { For, Show, createSignal, onMount } from "solid-js";

import type { ApprovalRule, JournalEntry, Playbook, Schedule } from "../bindings";
import { daemonCall } from "../ipc/rpc";
import {
  formatTime,
  groupApprovalRules,
  journalQueryParams,
  playbookState,
  scheduleAddParams,
} from "./automation";
import { IconCheck, IconClose, IconLayers, IconPlus, IconRefresh, IconSearch, IconSparkles } from "./icons";

interface AutomationSettingsProps {
  onConfirmDeletePlaybook?: (name: string, onConfirm: () => Promise<void>) => void;
}

export default function AutomationSettings(props: AutomationSettingsProps) {
  const [activeSubTab, setActiveSubTab] = createSignal<"playbooks" | "schedules" | "approvals" | "journal">("playbooks");
  const [playbooks, setPlaybooks] = createSignal<Playbook[]>([]);
  const [openPlaybook, setOpenPlaybook] = createSignal<string | null>(null);
  const [schedules, setSchedules] = createSignal<Schedule[]>([]);
  const [specDraft, setSpecDraft] = createSignal("");
  const [promptDraft, setPromptDraft] = createSignal("");
  const [capDraft, setCapDraft] = createSignal("");
  const [approvals, setApprovals] = createSignal<ApprovalRule[]>([]);
  const [journal, setJournal] = createSignal<JournalEntry[]>([]);
  const [journalSearch, setJournalSearch] = createSignal("");
  const [busy, setBusy] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);

  async function loadData() {
    try {
      const [pbRes, schRes, appRes, jRes] = await Promise.all([
        daemonCall<{ playbooks: Playbook[] }>("playbook.list").catch(() => ({ playbooks: [] })),
        daemonCall<{ schedules: Schedule[] }>("schedule.list").catch(() => ({ schedules: [] })),
        daemonCall<{ rules: ApprovalRule[] }>("approval.list").catch(() => ({ rules: [] })),
        daemonCall<{ entries: JournalEntry[] }>("journal.query", journalQueryParams(journalSearch())).catch(() => ({ entries: [] })),
      ]);
      setPlaybooks(pbRes.playbooks ?? []);
      setSchedules(schRes.schedules ?? []);
      setApprovals(appRes.rules ?? []);
      setJournal(jRes.entries ?? []);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  onMount(() => {
    void loadData();
  });

  async function actOnPlaybook(method: "playbook.approve" | "playbook.delete", name: string) {
    setBusy(name);
    setError(null);
    try {
      await daemonCall(method, { name });
      await loadData();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }

  async function addSchedule() {
    const spec = specDraft().trim();
    const prompt = promptDraft().trim();
    if (!spec || !prompt) return;
    setBusy("schedule-add");
    setError(null);
    try {
      await daemonCall("schedule.add", scheduleAddParams(spec, prompt, capDraft()));
      setSpecDraft("");
      setPromptDraft("");
      setCapDraft("");
      await loadData();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }

  async function removeSchedule(id: string) {
    setBusy(id);
    setError(null);
    try {
      await daemonCall("schedule.remove", { id });
      await loadData();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }

  async function revokeApproval(rule: ApprovalRule) {
    setBusy(rule.pattern);
    setError(null);
    try {
      await daemonCall("approval.revoke", { pattern: rule.pattern, repo: rule.repo });
      await loadData();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }

  async function searchJournal(event: Event) {
    event.preventDefault();
    setBusy("journal");
    try {
      const res = await daemonCall<{ entries: JournalEntry[] }>("journal.query", journalQueryParams(journalSearch()));
      setJournal(res.entries ?? []);
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
          <p class="section-label">Orchestration & Standing Rules</p>
          <p class="text-xs text-muted">Manage procedures, background schedules, approval policies, and execution audit history.</p>
        </div>
        <button
          type="button"
          class="focus-ring flex items-center gap-1.5 rounded-lg border border-line bg-raised/70 px-2.5 py-1 text-xs text-muted hover:bg-raised hover:text-foreground"
          onClick={() => void loadData()}
          title="Refresh automation state"
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

      {/* Sub-tab Pill Navigation */}
      <div class="flex gap-1.5 rounded-xl border border-line bg-raised/30 p-1">
        <button
          type="button"
          class={`focus-ring flex-1 rounded-lg py-1.5 text-xs font-medium transition-colors ${
            activeSubTab() === "playbooks"
              ? "bg-surface text-foreground shadow-xs font-semibold"
              : "text-muted hover:text-foreground"
          }`}
          onClick={() => setActiveSubTab("playbooks")}
        >
          Playbooks ({playbooks().length})
        </button>
        <button
          type="button"
          class={`focus-ring flex-1 rounded-lg py-1.5 text-xs font-medium transition-colors ${
            activeSubTab() === "schedules"
              ? "bg-surface text-foreground shadow-xs font-semibold"
              : "text-muted hover:text-foreground"
          }`}
          onClick={() => setActiveSubTab("schedules")}
        >
          Schedules ({schedules().length})
        </button>
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
            activeSubTab() === "journal"
              ? "bg-surface text-foreground shadow-xs font-semibold"
              : "text-muted hover:text-foreground"
          }`}
          onClick={() => setActiveSubTab("journal")}
        >
          Activity Journal
        </button>
      </div>

      {/* 1. PLAYBOOKS */}
      <Show when={activeSubTab() === "playbooks"}>
        <div class="space-y-3">
          <p class="text-xs text-muted">
            Procedures drafted from finished work. Inert until approved, after which Repomind reuses them across the fleet.
          </p>
          <For each={playbooks()} fallback={<p class="py-6 text-center text-xs text-muted">No playbooks recorded yet.</p>}>
            {(book) => {
              const state = () => playbookState(book);
              const isOpen = () => openPlaybook() === book.name;
              return (
                <div class="rounded-xl border border-line bg-raised/20 overflow-hidden">
                  <button
                    type="button"
                    class="focus-ring flex w-full items-center justify-between gap-3 p-3 text-left hover:bg-raised/40"
                    onClick={() => setOpenPlaybook(isOpen() ? null : book.name)}
                    aria-expanded={isOpen()}
                  >
                    <div class="min-w-0">
                      <b class="block truncate text-xs font-semibold text-foreground">{book.name}</b>
                      <span class="mt-0.5 block truncate text-[11px] text-muted">
                        {book.description || "No description provided"}
                      </span>
                    </div>
                    <span
                      class={`shrink-0 rounded-full px-2 py-0.5 font-mono text-[10px] font-semibold uppercase ${
                        state().awaitingApproval
                          ? "bg-attention/15 text-attention"
                          : "bg-signal/15 text-signal"
                      }`}
                    >
                      {state().label}
                    </span>
                  </button>
                  <Show when={isOpen()}>
                    <div class="border-t border-line bg-surface/50 p-3">
                      <pre class="max-h-60 overflow-auto rounded-lg border border-line bg-background p-3 font-mono text-[11px] leading-relaxed text-foreground/90 whitespace-pre-wrap">
                        {book.draft_content ?? book.content}
                      </pre>
                      <div class="mt-3 flex justify-end gap-2">
                        <button
                          type="button"
                          class="focus-ring rounded-lg border border-line bg-surface px-3 py-1 text-xs text-muted hover:border-fault/40 hover:text-fault"
                          disabled={busy() === book.name}
                          onClick={() => {
                            if (props.onConfirmDeletePlaybook) {
                              props.onConfirmDeletePlaybook(book.name, () => actOnPlaybook("playbook.delete", book.name));
                            } else {
                              void actOnPlaybook("playbook.delete", book.name);
                            }
                          }}
                        >
                          Delete
                        </button>
                        <Show when={state().awaitingApproval}>
                          <button
                            type="button"
                            class="focus-ring flex items-center gap-1 rounded-lg bg-signal px-3 py-1 text-xs font-semibold text-background hover:bg-signal/90"
                            disabled={busy() === book.name}
                            onClick={() => void actOnPlaybook("playbook.approve", book.name)}
                          >
                            <IconCheck size={12} />
                            <span>Approve Playbook</span>
                          </button>
                        </Show>
                      </div>
                    </div>
                  </Show>
                </div>
              );
            }}
          </For>
        </div>
      </Show>

      {/* 2. SCHEDULES */}
      <Show when={activeSubTab() === "schedules"}>
        <div class="space-y-4">
          <p class="text-xs text-muted">
            Standing orchestrations: Repomind runs headless on a recurring cron timer under an action cap, reporting findings to the journal.
          </p>

          <form
            class="space-y-2.5 rounded-xl border border-line bg-raised/30 p-3.5"
            onSubmit={(e) => {
              e.preventDefault();
              void addSchedule();
            }}
          >
            <div class="grid grid-cols-3 gap-2">
              <div class="col-span-2">
                <label class="mb-1 block text-[11px] font-medium text-muted">Schedule Spec</label>
                <input
                  class="focus-ring h-8 w-full rounded-lg border border-line bg-surface px-2.5 text-xs text-foreground outline-none"
                  placeholder="e.g. weekdays 09:00 or every 2h"
                  value={specDraft()}
                  onInput={(e) => setSpecDraft(e.currentTarget.value)}
                />
              </div>
              <div>
                <label class="mb-1 block text-[11px] font-medium text-muted">Max Actions</label>
                <input
                  class="focus-ring h-8 w-full rounded-lg border border-line bg-surface px-2.5 text-xs text-foreground outline-none"
                  placeholder="cap (e.g. 20)"
                  inputMode="numeric"
                  value={capDraft()}
                  onInput={(e) => setCapDraft(e.currentTarget.value)}
                />
              </div>
            </div>
            <div>
              <label class="mb-1 block text-[11px] font-medium text-muted">Goal / Prompt</label>
              <input
                class="focus-ring h-8 w-full rounded-lg border border-line bg-surface px-2.5 text-xs text-foreground outline-none"
                placeholder="e.g. Morning fleet briefing & audit open branches"
                value={promptDraft()}
                onInput={(e) => setPromptDraft(e.currentTarget.value)}
              />
            </div>
            <div class="flex items-center justify-between pt-1">
              <span class="text-[10px] text-muted font-mono">
                Formats: daily HH:MM · weekdays HH:MM · weekends HH:MM · every Nm · every Nh
              </span>
              <button
                type="submit"
                class="focus-ring flex items-center gap-1 rounded-lg bg-signal px-3 py-1 text-xs font-semibold text-background disabled:opacity-40"
                disabled={busy() === "schedule-add" || !specDraft().trim() || !promptDraft().trim()}
              >
                <IconPlus size={12} />
                <span>Add Schedule</span>
              </button>
            </div>
          </form>

          <For each={schedules()} fallback={<p class="py-4 text-center text-xs text-muted">No scheduled tasks configured.</p>}>
            {(entry) => (
              <div class="flex items-start justify-between gap-3 rounded-xl border border-line bg-raised/20 p-3">
                <div class="min-w-0">
                  <div class="flex items-center gap-2">
                    <b class="font-mono text-xs font-semibold text-foreground">{entry.spec}</b>
                    <span class="rounded bg-raised px-1.5 py-0.5 font-mono text-[10px] text-muted">
                      cap {entry.max_actions}
                    </span>
                  </div>
                  <p class="mt-1 text-xs text-foreground/90">{entry.prompt}</p>
                  <span class="mt-1 block font-mono text-[10px] text-muted">
                    {entry.last_run_at ? `Last run: ${formatTime(entry.last_run_at)}` : "Never run yet"}
                  </span>
                </div>
                <button
                  type="button"
                  class="focus-ring shrink-0 rounded-lg border border-line bg-surface px-2.5 py-1 text-xs text-muted hover:border-fault/40 hover:text-fault"
                  onClick={() => void removeSchedule(entry.id)}
                >
                  Remove
                </button>
              </div>
            )}
          </For>
        </div>
      </Show>

      {/* 3. APPROVALS */}
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

      {/* 4. ACTIVITY JOURNAL */}
      <Show when={activeSubTab() === "journal"}>
        <div class="space-y-3">
          <form class="flex gap-2" onSubmit={searchJournal}>
            <div class="flex flex-1 items-center gap-2 rounded-lg border border-line bg-surface px-2.5 py-1 text-xs">
              <IconSearch size={13} class="text-muted" />
              <input
                type="text"
                placeholder="Search what Repomind did…"
                class="w-full bg-transparent text-xs text-foreground outline-none placeholder:text-muted/60"
                value={journalSearch()}
                onInput={(e) => setJournalSearch(e.currentTarget.value)}
              />
            </div>
            <button
              type="submit"
              class="focus-ring rounded-lg bg-signal px-3.5 py-1 text-xs font-semibold text-background"
            >
              Filter
            </button>
          </form>

          <div class="max-h-96 space-y-2 overflow-y-auto pr-1">
            <For each={journal()} fallback={<p class="py-6 text-center text-xs text-muted">Nothing journaled yet.</p>}>
              {(entry) => (
                <div class="rounded-xl border border-line bg-raised/20 p-3">
                  <div class="flex items-center justify-between gap-2">
                    <b class="truncate font-mono text-xs text-foreground">{entry.action}</b>
                    <span
                      class={`shrink-0 rounded-full px-2 py-0.5 font-mono text-[10px] font-semibold uppercase ${
                        entry.outcome === "ok" ? "bg-signal/15 text-signal" : "bg-fault/15 text-fault"
                      }`}
                    >
                      {entry.outcome}
                    </span>
                  </div>
                  <div class="mt-1 flex items-center gap-2 text-[11px] text-muted">
                    <span>{formatTime(entry.at)}</span>
                    <Show when={entry.repo}>
                      {(repo) => <span class="truncate font-mono">· {repo()}</span>}
                    </Show>
                  </div>
                  <Show when={entry.detail ?? entry.params}>
                    {(detail) => (
                      <p class="mt-1.5 truncate rounded bg-surface/60 px-2 py-1 font-mono text-[10px] text-muted">
                        {detail()}
                      </p>
                    )}
                  </Show>
                </div>
              )}
            </For>
          </div>
        </div>
      </Show>
    </div>
  );
}
