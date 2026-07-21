import { Show } from "solid-js";

import type { ActionsStore } from "../stores/actions";
import ConfirmDialog from "./ConfirmDialog";
import NewLaneModal from "./NewLaneModal";
import RenameModal from "./RenameModal";
import SettingsModal from "./SettingsModal";
import SpawnModal from "./SpawnModal";

/// Mounts whichever action modal the actions store currently has open.
export default function ActionModals(props: { actions: ActionsStore }) {
  const actions = props.actions;
  return (
    <>
      <Show when={actions.settingsOpen()}>
        <SettingsModal onClose={actions.closeSettings} />
      </Show>
      <Show when={actions.spawnLane()}>
        {(lane) => <SpawnModal lane={lane()} onClose={actions.closeSpawn} onDone={() => actions.fleet.refresh()} />}
      </Show>
      <Show when={actions.newLaneOpen()}>
        <NewLaneModal
          repos={actions.fleet.repos()}
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
