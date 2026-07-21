import { createSignal } from "solid-js";

import type { Lane, Repo } from "../bindings";
import type { ConfirmOptions } from "../components/ConfirmDialog";
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

  return {
    fleet,
    error,
    dismissError: () => setError(null),
    settingsOpen,
    openSettings: () => setSettingsOpen(true),
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
  };
}

export type ActionsStore = ReturnType<typeof createActionsStore>;
