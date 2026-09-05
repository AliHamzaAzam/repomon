import { createSignal } from "solid-js";

import type { AgentSession, Lane, Repo } from "../bindings";
import type { ConfirmOptions } from "../components/ConfirmDialog";
import type { PolicySection } from "../components/PolicySettings";
import type { SettingsTab } from "../components/SettingsModal";
import { pickDirectory } from "../ipc/dialog";
import { daemonCall } from "../ipc/rpc";
import type { FleetStore } from "./fleet";
import type { WorkspaceStore } from "./workspace";

export interface RenameTarget {
  targetId: string;
  sessionId?: string | null;
  current: string;
}

/// Owns the state for every input/confirm modal so any surface (sidebar, control center,
/// header) can open one without threading callbacks. The matching <ActionModals> renders them.
export function createActionsStore(fleet: FleetStore, workspace?: WorkspaceStore) {
  const [controlOpen, setControlOpen] = createSignal(false);
  const [shortcutsGuideOpen, setShortcutsGuideOpen] = createSignal(false);
  const [settingsOpen, setSettingsOpen] = createSignal(false);
  const [settingsTab, setSettingsTab] = createSignal<SettingsTab>("general");
  // Which sub-tab of Settings > Automation to land on, for callers that mean one of them
  // specifically rather than the tab as a whole.
  const [policySection, setPolicySection] = createSignal<PolicySection | undefined>();
  // A model id to pre-fill Settings > Usage's filter with, for the Usage view's unpriced-model
  // warning.
  const [usageFilter, setUsageFilter] = createSignal<string | undefined>();
  const [spawnLane, setSpawnLane] = createSignal<Lane | null>(null);
  const [newLaneOpen, setNewLaneOpen] = createSignal(false);
  const [newLaneRepoId, setNewLaneRepoId] = createSignal<number | null>(null);
  const [renameTarget, setRenameTarget] = createSignal<RenameTarget | null>(null);
  const [notesRepo, setNotesRepo] = createSignal<Repo | null>(null);
  const [confirmOptions, setConfirmOptions] = createSignal<ConfirmOptions | null>(null);
  const [error, setError] = createSignal<string | null>(null);

  async function addRepo() {
    setError(null);
    try {
      const path = await pickDirectory("Choose a git repository");
      if (!path) return;
      await daemonCall("repo.add", { path });
      await fleet.refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  function removeRepo(repo: Repo) {
    setConfirmOptions({
      title: `Remove ${repo.name}?`,
      message: `Stop tracking ${repo.name} in repomon. Files and worktrees on disk are left untouched.`,
      confirmLabel: "Remove",
      danger: true,
      onConfirm: async () => {
        await daemonCall("repo.remove", { repo_id: repo.id });
        await fleet.refresh();
      },
    });
  }

  /// Open the per-repo notes editor. repomind reads these when planning and folds them into the
  /// prompts of workers it spawns in this repo.
  function openRepoNotes(repo: Repo) {
    setNotesRepo(repo);
  }

  function closeRepoNotes() {
    setNotesRepo(null);
  }

  /// Hide or reveal a repo. No confirmation: unlike removeRepo this keeps the registration and
  /// every lane, so it is fully reversible from the sidebar's hidden list.
  async function setRepoHidden(repo: Repo, hidden: boolean) {
    setError(null);
    try {
      await daemonCall("repo.set_hidden", { repo_id: repo.id, hidden });
      await fleet.refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  /// Set a repo's display label (shown instead of the folder name). An empty label clears the
  /// override.
  async function renameRepo(repo: Repo, label: string) {
    setError(null);
    try {
      await daemonCall("repo.rename", { repo_id: repo.id, label });
      await fleet.refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  /// Persist a manual sidebar ordering. The caller computes the full visible order; the daemon
  /// assigns dense positions so it survives restarts and reaches the TUI too.
  async function reorderRepos(orderedIds: number[]) {
    setError(null);
    try {
      await daemonCall("repo.reorder", { ordered_ids: orderedIds });
      await fleet.refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  /// Persist a lane's manual agent-tab ordering (managed window ids or external transcript ids).
  /// meaningful while the tab sort mode is "manual"; the daemon re-applies it on every overlay.
  async function setAgentTabOrder(laneId: number, orderedSessionIds: string[]) {
    setError(null);
    try {
      await daemonCall("agent.set_tab_order", { lane_id: laneId, ordered_ids: orderedSessionIds });
      await fleet.refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  /// Deleting a playbook throws away procedural memory that took real work to earn, so it asks
  /// first. Approving does not: reading the content and clicking Approve is itself the review.
  function confirmPlaybookDelete(name: string, onConfirm: () => Promise<void>) {
    setConfirmOptions({
      title: `Delete playbook ${name}?`,
      message: "repomind loses this procedure and will re-derive it from scratch next time.",
      confirmLabel: "Delete",
      danger: true,
      onConfirm,
    });
  }

  /// Pin or unpin the lane. Pinning is not destructive, so it applies immediately.
  async function pinLane(lane: Lane) {
    setError(null);
    try {
      await daemonCall("agent.pin", { lane_id: lane.id, pinned: !lane.pinned });
      await fleet.refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  function mergeLane(lane: Lane) {
    // The daemon refuses to delete or merge a main worktree, so do not raise a destructive
    // prompt for an operation that cannot succeed.
    if (lane.worktree.is_main) return;
    setConfirmOptions({
      title: "Merge lane?",
      message: `Merge ${lane.worktree.branch ?? lane.worktree.name} into the repository base branch.`,
      confirmLabel: "Merge",
      onConfirm: async () => {
        await daemonCall("lane.merge", { lane_id: lane.id });
        await fleet.refresh();
      },
    });
  }

  function deleteLane(lane: Lane) {
    // The daemon refuses to delete or merge a main worktree, so do not raise a destructive
    // prompt for an operation that cannot succeed.
    if (lane.worktree.is_main) return;
    setConfirmOptions({
      title: "Delete lane?",
      message: `Remove the ${lane.worktree.branch ?? lane.worktree.name} worktree. The branch is kept.`,
      confirmLabel: "Delete",
      danger: true,
      onConfirm: async () => {
        await daemonCall("lane.delete", { lane_id: lane.id, also_delete_branch: false });
        await fleet.refresh();
      },
    });
  }

  function stopAgent(lane: Lane, agent: AgentSession | null, targetWindow?: string) {
    const name = agent?.custom_label ?? agent?.title ?? agent?.agent;
    const window = targetWindow ?? agent?.tmux_window ?? undefined;
    setConfirmOptions({
      title: "Stop agent?",
      message: name ? `Stop ${name}. Its terminal session ends.` : "Stop this managed agent. Its terminal session ends.",
      confirmLabel: "Stop",
      danger: true,
      onConfirm: async () => {
        if (window) workspace?.markClosing(window);
        try {
          await daemonCall("agent.stop", { lane_id: lane.id, window });
          await fleet.refresh();
        } catch (cause) {
          if (window) workspace?.unmarkClosing(window);
          setError(cause instanceof Error ? cause.message : String(cause));
        }
      },
    });
  }

  /// Start or stop the repomind controller. `orchestrator.start`/`orchestrator.stop` are the
  /// daemon's aliases onto the controller lane's primary window, so the pinned sidebar row, its
  /// context menu, and the panel header all drive one lifecycle rather than three.
  async function repomindLifecycle(action: "start" | "stop") {
    setError(null);
    try {
      if (action === "start") await daemonCall("orchestrator.start", {});
      else await daemonCall("orchestrator.stop");
      await fleet.refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function adoptAgent(lane: Lane, agent: AgentSession | null) {
    setError(null);
    try {
      await daemonCall("agent.adopt", {
        lane_id: lane.id,
        session_id: agent?.session_id ?? undefined,
        agent: agent?.agent,
      });
      await fleet.refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  /// Bulk-adopt every orphaned/external session across all lanes in one click.
  /// Returns the number of sessions that were successfully restored.
  async function restoreAllAgents(): Promise<number> {
    setError(null);
    const allLanes = fleet.lanes();
    const candidates: Array<{ lane: Lane; session: AgentSession }> = [];

    for (const lane of allLanes) {
      for (const sess of lane.agent_sessions) {
        if (sess.external || (!sess.tmux_window && sess.session_id)) {
          candidates.push({ lane, session: sess });
        }
      }
    }

    let restored = 0;
    for (const { lane, session } of candidates) {
      try {
        await daemonCall("agent.adopt", {
          lane_id: lane.id,
          session_id: session.session_id ?? undefined,
          agent: session.agent,
        });
        restored++;
      } catch {
        // Continue with remaining sessions even if one fails
      }
    }

    await fleet.refresh();
    return restored;
  }

  return {
    fleet,
    error,
    dismissError: () => setError(null),
    reportError: (message: string) => setError(message),
    controlOpen,
    openControl: () => setControlOpen(true),
    closeControl: () => setControlOpen(false),
    toggleControl: () => setControlOpen((open) => !open),
    // The shortcuts cheat sheet overlay (mod+? or a bare "?" outside a text input). Exported here
    // as `openShortcutsGuide` rather than as a bare module function so any surface holding an
    // ActionsStore reference - the header, the control palette, or onboarding's Done step - can
    // open it the same way it opens every other panel.
    shortcutsGuideOpen,
    openShortcutsGuide: () => setShortcutsGuideOpen(true),
    closeShortcutsGuide: () => setShortcutsGuideOpen(false),
    settingsOpen,
    settingsTab,
    policySection,
    usageFilter,
    openSettings: () => {
      setSettingsTab("general");
      setPolicySection(undefined);
      setUsageFilter(undefined);
      setSettingsOpen(true);
    },
    openSettingsTab: (tab: SettingsTab, section?: PolicySection, usageFilterValue?: string) => {
      setSettingsTab(tab);
      setPolicySection(section);
      setUsageFilter(usageFilterValue);
      setSettingsOpen(true);
    },
    closeSettings: () => setSettingsOpen(false),
    spawnLane,
    spawn: (lane: Lane) => setSpawnLane(lane),
    closeSpawn: () => setSpawnLane(null),
    newLaneOpen,
    newLaneRepoId,
    newLane: (repoId?: number) => {
      setNewLaneRepoId(repoId ?? null);
      setNewLaneOpen(true);
    },
    closeNewLane: () => setNewLaneOpen(false),
    renameTarget,
    rename: (target: RenameTarget) => setRenameTarget(target),
    closeRename: () => setRenameTarget(null),
    confirmOptions,
    confirm: (options: ConfirmOptions) => setConfirmOptions(options),
    closeConfirm: () => setConfirmOptions(null),
    addRepo,
    removeRepo,
    setRepoHidden,
    renameRepo,
    reorderRepos,
    setAgentTabOrder,
    confirmPlaybookDelete,
    notesRepo,
    openRepoNotes,
    closeRepoNotes,
    pinLane,
    mergeLane,
    deleteLane,
    stopAgent,
    adoptAgent,
    restoreAllAgents,
    startRepomind: () => repomindLifecycle("start"),
    stopRepomind: () => repomindLifecycle("stop"),
  };
}

export type ActionsStore = ReturnType<typeof createActionsStore>;
