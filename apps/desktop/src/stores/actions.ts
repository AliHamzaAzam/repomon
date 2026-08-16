import { createSignal } from "solid-js";

import type { AgentSession, Lane, Repo } from "../bindings";
import type { ConfirmOptions } from "../components/ConfirmDialog";
import type { SettingsTab } from "../components/SettingsModal";
import { pickDirectory } from "../ipc/dialog";
import { daemonCall } from "../ipc/rpc";
import type { FleetStore } from "./fleet";
import type { WorkspaceStore } from "./workspace";

export interface RenameTarget {
  sessionId: string;
  current: string;
}

/// Owns the state for every input/confirm modal so any surface (sidebar, control center,
/// header) can open one without threading callbacks. The matching <ActionModals> renders them.
export function createActionsStore(fleet: FleetStore, workspace?: WorkspaceStore) {
  const [controlOpen, setControlOpen] = createSignal(false);
  const [settingsOpen, setSettingsOpen] = createSignal(false);
  const [settingsTab, setSettingsTab] = createSignal<SettingsTab>("general");
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
    settingsOpen,
    settingsTab,
    openSettings: () => {
      setSettingsTab("general");
      setSettingsOpen(true);
    },
    openSettingsTab: (tab: SettingsTab) => {
      setSettingsTab(tab);
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
  };
}

export type ActionsStore = ReturnType<typeof createActionsStore>;
