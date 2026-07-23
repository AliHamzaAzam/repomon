import { Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";

import ActionModals from "./components/ActionModals";
import FleetSidebar from "./components/FleetSidebar";
import ControlCenter from "./components/ControlCenter";
import RepomindPanel from "./components/RepomindPanel";
import TerminalWorkspace from "./components/TerminalWorkspace";
import UpdateBanner from "./components/UpdateBanner";
import { getVersion } from "@tauri-apps/api/app";
import { checkForUpdate, type AvailableUpdate } from "./ipc/updater";
import { createActionsStore } from "./stores/actions";
import {
  initialConnection,
  tauriConnectionSource,
  type ConnectionPhase,
  type ConnectionSource,
} from "./ipc/connection";
import { daemonCall } from "./ipc/rpc";
import { applyAccent, applyTheme, nextTheme, readTheme, themeLabel } from "./theme";
import { createFleetStore, type FleetSource } from "./stores/fleet";
import { createNotificationStore } from "./stores/notifications";

interface AppProps {
  connectionSource?: ConnectionSource;
  fleetSource?: FleetSource;
}

function phaseLabel(phase: ConnectionPhase): string {
  switch (phase) {
    case "starting":
      return "Starting";
    case "connecting":
      return "Connecting";
    case "connected":
      return "Connected";
    case "retrying":
      return "Retrying";
  }
}

function formatUptime(totalSeconds?: number): string {
  if (totalSeconds === undefined) return "--";
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  return hours > 0 ? `${hours}h ${minutes.toString().padStart(2, "0")}m` : `${minutes}m`;
}

function App(props: AppProps) {
  const [theme, setTheme] = createSignal(readTheme());
  const [connection, setConnection] = createSignal(initialConnection);
  const [repomindOpen, setRepomindOpen] = createSignal(true);
  const [update, setUpdate] = createSignal<AvailableUpdate | null>(null);
  const [appVersion, setAppVersion] = createSignal("");
  const source = props.connectionSource ?? tauriConnectionSource;
  const fleet = createFleetStore(props.fleetSource);
  const actions = createActionsStore(fleet);
  const notifications = createNotificationStore((laneId) => fleet.setSelectedLaneId(laneId));
  let stopListening: (() => void) | undefined;
  let fleetStarted = false;
  let notificationsStarted = false;
  let searchInput: HTMLInputElement | undefined;
  let active = true;

  createEffect(() => {
    if (connection().phase === "connected" && !fleetStarted) {
      fleetStarted = true;
      fleet.start();
      void daemonCall("config.get").then((config) => applyAccent(config.accent)).catch(() => undefined);
    } else if (connection().phase !== "connected" && fleetStarted) {
      fleetStarted = false;
      fleet.stop();
    }
  });

  createEffect(() => {
    if (connection().phase === "connected" && !notificationsStarted) {
      notificationsStarted = true;
      void notifications.start();
    } else if (connection().phase !== "connected" && notificationsStarted) {
      notificationsStarted = false;
      notifications.stop();
    }
  });

  const onSettingsShortcut = (event: KeyboardEvent) => {
    if ((event.metaKey || event.ctrlKey) && event.key === ",") {
      event.preventDefault();
      actions.openSettings();
    }
  };

  onMount(() => {
    window.addEventListener("keydown", onSettingsShortcut);
    void getVersion().then(setAppVersion).catch(() => undefined);

    // Check for a newer build once on launch; silent if current or if not a Tauri build.
    void checkForUpdate()
      .then((available) => {
        if (active && available) setUpdate(available);
      })
      .catch(() => undefined);

    void source
      .subscribe(setConnection)
      .then((stop) => {
        if (active) stopListening = stop;
        else stop();
      })
      .catch(() => undefined);

    void source
      .current()
      .then((snapshot) => {
        if (active) setConnection(snapshot);
      })
      .catch((error: unknown) => {
        if (!active) return;
        setConnection({
          phase: "retrying",
          endpoint: initialConnection.endpoint,
          message: error instanceof Error ? error.message : String(error),
          daemon: null,
        });
      });
  });

  onCleanup(() => {
    active = false;
    window.removeEventListener("keydown", onSettingsShortcut);
    stopListening?.();
    fleet.stop();
    notifications.stop();
  });

  const cycleTheme = () => {
    const value = nextTheme(theme());
    setTheme(value);
    applyTheme(value);
  };

  const navigateFleet = (event: KeyboardEvent) => {
    const target = event.target;
    if (
      target instanceof HTMLInputElement
      || target instanceof HTMLTextAreaElement
      || target instanceof HTMLSelectElement
      || (target instanceof HTMLElement && target.isContentEditable)
    ) {
      if (event.key === "Escape") event.currentTarget instanceof HTMLElement && event.currentTarget.focus();
      return;
    }
    if (event.key === "/") {
      event.preventDefault();
      searchInput?.focus();
    } else if (event.key === "j" || event.key === "ArrowDown") {
      event.preventDefault();
      fleet.moveSelection(1);
    } else if (event.key === "k" || event.key === "ArrowUp") {
      event.preventDefault();
      fleet.moveSelection(-1);
    } else if (event.key === "n") {
      event.preventDefault();
      fleet.moveSelection(1, true);
    }
  };

  return (
    <div class="grid h-screen min-h-[38rem] grid-rows-[3.5rem_minmax(0,1fr)_2.75rem] overflow-hidden bg-background text-foreground">
      <header class="flex items-center justify-between border-b border-line bg-surface px-4">
        <div class="flex items-center gap-3">
          <div class="brand-mark" aria-hidden="true">
            <span />
            <span />
            <span />
          </div>
          <div class="flex items-baseline gap-3">
            <h1 class="text-[0.95rem] font-semibold tracking-[-0.02em]">Repomon</h1>
            <span class="font-mono text-[0.64rem] uppercase tracking-[0.18em] text-muted">
              Mission control
            </span>
          </div>
        </div>

        <div class="flex items-center gap-2">
          <span class="rounded-full border border-line bg-raised px-2.5 py-1 font-mono text-[0.6rem] uppercase tracking-[0.14em] text-muted">
            Local
          </span>
          <ControlCenter fleet={fleet} notifications={notifications} actions={actions} />
          <button
            type="button"
            class="focus-ring rounded-md border border-line bg-raised px-2.5 py-1.5 font-mono text-[0.58rem] uppercase tracking-[0.1em] text-muted hover:text-foreground"
            onClick={() => actions.openSettings()}
            aria-label="Settings"
            title="Settings (⌘,)"
          >Settings</button>
          <button
            type="button"
            class={`focus-ring rounded-md border px-2.5 py-1.5 font-mono text-[0.58rem] uppercase tracking-[0.1em] ${repomindOpen() ? "border-signal/40 bg-signal/10 text-signal" : "border-line bg-raised text-muted"}`}
            onClick={() => setRepomindOpen(!repomindOpen())}
            aria-pressed={repomindOpen()}
          >Repomind</button>
          <button
            type="button"
            class="focus-ring rounded-md border border-line bg-raised px-2.5 py-1.5 font-mono text-[0.64rem] uppercase tracking-[0.12em] text-muted transition-colors hover:text-foreground"
            onClick={cycleTheme}
            aria-label={`Theme: ${themeLabel(theme())}`}
          >
            {themeLabel(theme())}
          </button>
        </div>
      </header>

      <div class={`mission-grid ${repomindOpen() ? "is-repomind-open" : ""}`}>
        <nav
          aria-label="Fleet"
          class="flex min-h-0 flex-col border-r border-line bg-surface outline-none"
          tabIndex={0}
          onKeyDown={navigateFleet}
        >
          <div class="flex items-center justify-between border-b border-line px-4 py-3">
            <span class="section-label">Fleet</span>
              <span class="font-mono text-[0.62rem] text-muted">
                {fleet.repos().length} / {fleet.lanes().length}
              </span>
          </div>
          <FleetSidebar fleet={fleet} actions={actions} searchRef={(element) => { searchInput = element; }} />
        </nav>

        <main aria-label="Terminal bay" class="terminal-bay relative min-h-0 overflow-hidden bg-background">
          <TerminalWorkspace fleet={fleet} actions={actions} />
        </main>

        <aside
          aria-label="Repomind"
          class="repomind-panel min-h-0 border-l border-line bg-surface"
        >
          <div class="flex items-center justify-between border-b border-line px-4 py-3">
            <span class="section-label">Repomind</span>
            <span class="size-1.5 rounded-full bg-muted/50" aria-hidden="true" />
          </div>
          <Show when={repomindOpen()}>
            <RepomindPanel />
          </Show>
        </aside>
      </div>

      <footer
        role="status"
        aria-label="Daemon connection"
        class="connection-rail grid grid-cols-[auto_minmax(11rem,1fr)_auto_auto_auto] items-center gap-5 border-t border-line bg-surface px-4 font-mono text-[0.64rem] text-muted"
      >
        <div class="flex items-center gap-2 text-foreground">
          <span class={`status-light is-${connection().phase}`} aria-hidden="true" />
          <span class="uppercase tracking-[0.12em]">{phaseLabel(connection().phase)}</span>
        </div>
        <span class="flex min-w-0 items-center gap-2 truncate">
          <span class="truncate">{connection().endpoint}</span>
          {connection().message ? (
            <span class="truncate text-fault">{connection().message}</span>
          ) : null}
        </span>
        <span>App {appVersion() || "--"} · daemon {connection().daemon?.version ?? "--"}</span>
        <span>
          {connection().daemon?.repos ?? 0} repos / {connection().daemon?.lanes ?? 0} lanes
        </span>
        <span>Uptime {formatUptime(connection().daemon?.uptime_secs)}</span>
      </footer>

      <ActionModals actions={actions} />
      <Show when={actions.error() ?? fleet.error()}>
        {(message) => (
          <div role="alert" class="fixed right-4 top-16 z-[70] flex max-w-md items-start gap-3 rounded-md border border-fault/40 bg-surface p-3 text-xs text-fault shadow-lg">
            <span>{message()}</span>
            <button type="button" class="focus-ring rounded px-1 text-muted hover:text-foreground" aria-label="Dismiss error" onClick={() => {
              actions.dismissError();
              fleet.dismissError();
            }}>×</button>
          </div>
        )}
      </Show>
      <Show when={update()}>
        {(available) => <UpdateBanner update={available()} onDismiss={() => setUpdate(null)} />}
      </Show>
    </div>
  );
}

export default App;
