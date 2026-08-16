import { Show } from "solid-js";

import type { ActionsStore } from "../stores/actions";
import type { NotificationStore } from "../stores/notifications";
import ConfirmDialog from "./ConfirmDialog";
import NewLaneModal from "./NewLaneModal";
import RepoNotesModal from "./RepoNotesModal";
import RenameModal from "./RenameModal";
import SettingsModal from "./SettingsModal";
import SpawnModal from "./SpawnModal";

/// Mounts whichever action modal the actions store currently has open.
export default function ActionModals(props: {
  actions: ActionsStore;
  notifications: NotificationStore;
  onReplayOnboarding?: () => void;
}) {
  const actions = props.actions;
  return (
    <>
      <Show when={actions.settingsOpen()}>
        <SettingsModal
          onClose={actions.closeSettings}
          initialTab={actions.settingsTab()}
          onConfigSaved={props.notifications.setConfig}
          onPreviewSound={props.notifications.preview}
          onUpdateAvailable={(version) => void props.notifications.notifyUpdateReady(version)}
          onReplayOnboarding={props.onReplayOnboarding}
          fleet={actions.fleet}
          actions={actions}
        />
      </Show>
      <Show when={actions.spawnLane()}>
        {(lane) => (
          <SpawnModal
            lane={lane()}
            onClose={actions.closeSpawn}
            onDone={() => actions.fleet.refresh()}
            onOpenSettingsTab={actions.openSettingsTab}
          />
        )}
      </Show>
      <Show keyed when={actions.notesRepo()}>
        {(repo) => <RepoNotesModal repo={repo} onClose={actions.closeRepoNotes} />}
      </Show>
      <Show when={actions.newLaneOpen()}>
        <NewLaneModal
          repos={actions.fleet.visibleRepos()}
          initialRepoId={actions.newLaneRepoId() ?? undefined}
          onClose={actions.closeNewLane}
          onDone={async (laneId) => {
            await actions.fleet.refresh();
            actions.fleet.setSelectedLaneId(laneId);
          }}
        />
      </Show>
      <Show when={actions.renameTarget()}>
        {(target) => (
          <RenameModal
            sessionId={target().sessionId}
            current={target().current}
            onClose={actions.closeRename}
            onDone={() => actions.fleet.refresh()}
          />
        )}
      </Show>
      <Show when={actions.confirmOptions()}>
        {(options) => <ConfirmDialog options={options()} onClose={actions.closeConfirm} />}
      </Show>
    </>
  );
}
