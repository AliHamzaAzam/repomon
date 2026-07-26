import { createSignal } from "solid-js";

import type { AgentSession, Lane, Repo } from "../bindings";
import type { ConfirmOptions } from "../components/ConfirmDialog";
import type { SettingsTab } from "../components/SettingsModal";
import { pickDirectory } from "../ipc/dialog";
import { daemonCall } from "../ipc/rpc";
import type { FleetStore } from "./fleet";

export interface RenameTarget {
  sessionId: string;
  current: string;
}

/// Owns the state for every input/confirm modal so any surface (sidebar, control center,
/// header) can open one without threading callbacks. The matching <ActionModals> renders them.
export function createActionsStore(fleet: FleetStore) {
  const [settingsOpen, setSettingsOpen] = createSignal(false);
  const [settingsTab, setSettingsTab] = createSignal<SettingsTab>("general");
  const [spawnLane, setSpawnLane] = createSignal<Lane | null>(null);
  const [newLaneOpen, setNewLaneOpen] = createSignal(false);
  const [newLaneRepoId, setNewLaneRepoId] = createSignal<number | null>(null);
  const [renameTarget, setRenameTarget] = createSignal<RenameTarget | null>(null);
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
    setConfirmOptions({
      title: "Delete lane?",
      message: `Remove the ${lane.worktree.name} worktree. The branch is kept.`,
      confirmLabel: "Delete",
      danger: true,
      onConfirm: async () => {
        await daemonCall("lane.delete", { lane_id: lane.id, also_delete_branch: false });
        await fleet.refresh();
      },
    });
  }

  function stopAgent(lane: Lane, agent: AgentSession | null) {
    setConfirmOptions({
      title: "Stop agent?",
      message: "Stop this managed agent. Its terminal session ends.",
      confirmLabel: "Stop",
      danger: true,
      onConfirm: async () => {
        await daemonCall("agent.stop", { lane_id: lane.id, window: agent?.tmux_window ?? undefined });
        await fleet.refresh();
      },
    });
  }

  async function adoptAgent(lane: Lane, agent: AgentSession | null) {
    setError(null);
    try {
      await daemonCall("agent.adopt", { lane_id: lane.id, session_id: agent?.session_id ?? undefined });
      await fleet.refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  return {
    fleet,
    error,
    dismissError: () => setError(null),
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
    pinLane,
    mergeLane,
    deleteLane,
    stopAgent,
    adoptAgent,
  };
}

export type ActionsStore = ReturnType<typeof createActionsStore>;
