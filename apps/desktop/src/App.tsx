import { Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";

import ActionModals from "./components/ActionModals";
import FleetSidebar from "./components/FleetSidebar";
import ControlCenter from "./components/ControlCenter";
import ExtensionsView from "./components/ExtensionsView";
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
import { matchChord } from "./keymap";
import BrandMark from "./components/BrandMark";
import { setAgentIconOverrides } from "./components/icons";
import { applyAccent, applyTheme, nextTheme, readTheme, themeLabel } from "./theme";
import { createExtensionsStore } from "./stores/extensions";
import { createFleetStore, type FleetSource } from "./stores/fleet";
import { createNotificationStore } from "./stores/notifications";
import { createMessageStore } from "./stores/messages";
import { createWorkspaceStore } from "./stores/workspace";
import { IconClose, IconExtensions, IconSettings, IconSparkles } from "./components/icons";

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
  const [repomindFull, setRepomindFull] = createSignal(false);
  const [extensionsOpen, setExtensionsOpen] = createSignal(false);
  const [update, setUpdate] = createSignal<AvailableUpdate | null>(null);
  const [appVersion, setAppVersion] = createSignal("");
  const source = props.connectionSource ?? tauriConnectionSource;
  const fleet = createFleetStore(props.fleetSource);
  const actions = createActionsStore(fleet);
  const workspace = createWorkspaceStore(fleet);
  const ext = createExtensionsStore();
  const notifications = createNotificationStore((laneId) => fleet.setSelectedLaneId(laneId));
  const messages = createMessageStore((laneId, slot) => {
    fleet.setSelectedLaneId(laneId);
    const lane = fleet.lanes().find((item) => item.id === laneId);
    const window = lane?.agent_sessions[(slot ?? 1) - 1]?.tmux_window;
    if (window) fleet.setFocusedWindow(window);
  });
  let stopListening: (() => void) | undefined;
  let fleetStarted = false;
  let notificationsStarted = false;
  let messagesStarted = false;
  let searchInput: HTMLInputElement | undefined;
  let active = true;

  createEffect(() => {
    if (connection().phase === "connected" && !fleetStarted) {
      fleetStarted = true;
      fleet.start();
      void daemonCall("config.get")
        .then((config) => {
          applyAccent(config.accent);
          if (config.agent_icons) setAgentIconOverrides(config.agent_icons);
        })
        .catch(() => undefined);
    } else if (connection().phase !== "connected" && fleetStarted) {
      fleetStarted = false;
      fleet.stop();
    }
  });

  createEffect(() => {
    if (connection().phase === "connected" && !messagesStarted) {
      messagesStarted = true;
      void messages.start();
    } else if (connection().phase !== "connected" && messagesStarted) {
      messagesStarted = false;
      messages.stop();
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

  createEffect(() => {
    const available = update();
    if (connection().phase === "connected" && available) {
      void notifications.notifyUpdateReady(available.version);
    }
  });

  const onShortcut = (event: KeyboardEvent) => {
    const binding = matchChord(event);
    if (!binding) return;

    if (actions.settingsOpen() || actions.spawnLane() || actions.newLaneOpen()
      || actions.renameTarget() || actions.confirmOptions()) return;

    const lane = fleet.selectedLane();
    if (binding.when && !lane) return;
    const active = workspace.activeWindow();
    const agent = lane?.agent_sessions.find((session) => session.tmux_window === active)
      ?? lane?.agent_sessions.find((session) => session.tmux_window)
      ?? null;
    if (binding.when === "agent" && !agent) return;

    event.preventDefault();
    switch (binding.id) {
      case "panel.settings": actions.openSettings(); break;
      case "panel.extensions": setExtensionsOpen((open) => !open); break;
      case "panel.repomind": setRepomindOpen((open) => !open); break;
      case "panel.repomindFull":
        if (!repomindFull()) setRepomindOpen(true);
        setRepomindFull((full) => !full);
        break;
      case "panel.theme": cycleTheme(); break;
      case "layout.focused": workspace.chooseLayout("focused"); break;
      case "layout.split": workspace.chooseLayout("split"); break;
      case "layout.grid": workspace.chooseLayout("grid"); break;
      case "fleet.filter": searchInput?.focus(); break;
      case "fleet.urgent": fleet.setUrgentOnly(!fleet.urgentOnly()); break;
      case "fleet.refresh": void fleet.refresh(); break;
      case "fleet.newLane": actions.newLane(); break;
      case "fleet.addRepo": void actions.addRepo(); break;
      case "fleet.jumpUrgent": fleet.moveSelection(1, true); break;
      case "fleet.hideRepo": if (lane) void actions.setRepoHidden(lane.repo, true); break;
      case "fleet.repoNotes": if (lane) actions.openRepoNotes(lane.repo); break;
      case "lane.spawn": if (lane) actions.spawn(lane); break;
      case "lane.terminal": void workspace.openShell(actions.reportError); break;
      case "lane.pin": if (lane) void actions.pinLane(lane); break;
      case "lane.delete": if (lane) actions.deleteLane(lane); break;
      case "lane.merge": if (lane) actions.mergeLane(lane); break;
      case "lane.stop": if (lane) actions.stopAgent(lane, agent); break;
      case "agents.prev": workspace.cycleTab(-1, workspace.laneTargets()); break;
      case "agents.next": workspace.cycleTab(1, workspace.laneTargets()); break;
      case "help.open": actions.openSettingsTab("keyboard"); break;
      default:
        console.warn(`No handler for keyboard binding: ${binding.id}`);
        break;
    }
  };

  onMount(() => {
    window.addEventListener("keydown", onShortcut);
    void getVersion().then(setAppVersion).catch(() => undefined);

    void checkForUpdate()
      .then((available) => {
        if (active && available) {
          setUpdate(available);
        }
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
    window.removeEventListener("keydown", onShortcut);
    stopListening?.();
    fleet.stop();
    notifications.stop();
    messages.stop();
  });

  const cycleTheme = () => {
    const value = nextTheme(theme());
    setTheme(value);
    applyTheme(value);
  };

  createEffect(() => {
    if (!repomindFull()) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      setRepomindFull(false);
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

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
    <div class="grid h-screen min-h-[36rem] grid-rows-[2.75rem_minmax(0,1fr)_2rem] overflow-hidden bg-background text-foreground">
      <header class="flex items-center justify-between border-b border-line bg-surface/95 px-3.5 backdrop-blur">
        <div class="flex items-center gap-2.5">
          <BrandMark size={22} />
          <h1 class="text-xs font-semibold tracking-tight text-foreground">Repomon</h1>
        </div>

        <div class="flex items-center gap-1.5">
          <ControlCenter fleet={fleet} notifications={notifications} messages={messages} actions={actions} />
          <button
            type="button"
            class={`focus-ring flex h-7 items-center gap-1.5 rounded-lg border px-2.5 text-xs font-medium transition-colors ${
              extensionsOpen()
                ? "border-signal/50 bg-signal/10 text-signal font-semibold"
                : "border-line bg-raised/70 text-muted hover:bg-raised hover:text-foreground"
            }`}
            onClick={() => setExtensionsOpen(!extensionsOpen())}
            aria-pressed={extensionsOpen()}
            title="Extensions (⌘4)"
          >
            <IconExtensions size={13} />
            <span>Extensions</span>
          </button>
          <button
            type="button"
            class={`focus-ring flex h-7 items-center gap-1.5 rounded-lg border px-2.5 text-xs font-medium transition-colors ${
              repomindOpen()
                ? "border-signal/50 bg-signal/10 text-signal font-semibold"
                : "border-line bg-raised/70 text-muted hover:bg-raised hover:text-foreground"
            }`}
            onClick={() => setRepomindOpen(!repomindOpen())}
            aria-pressed={repomindOpen()}
            title="Repomind (⌘3)"
          >
            <IconSparkles size={13} />
            <span>Repomind</span>
          </button>
          <button
            type="button"
            class="focus-ring flex size-7 items-center justify-center rounded-lg border border-line bg-raised/70 text-muted transition-colors hover:bg-raised hover:text-foreground"
            onClick={() => actions.openSettings()}
            aria-label="Settings"
            title="Settings (⌘,)"
          >
            <IconSettings size={14} />
          </button>
          <button
            type="button"
            class="focus-ring flex h-7 items-center rounded-lg border border-line bg-raised/70 px-2 font-mono text-[10px] uppercase tracking-wider text-muted transition-colors hover:bg-raised hover:text-foreground"
            onClick={cycleTheme}
            aria-label={`Theme: ${themeLabel(theme())}`}
            title="Toggle theme"
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
          <div class="flex items-center justify-between border-b border-line px-3 py-2.5">
            <span class="section-label">Fleet</span>
            <span class="font-mono text-[10px] text-muted">
              {fleet.visibleRepos().length} repos · {fleet.unhiddenLanes().length} lanes
            </span>
          </div>
          <FleetSidebar
            fleet={fleet}
            actions={actions}
            searchRef={(element) => { searchInput = element; }}
            onOpenExtensions={(repoId) => {
              ext.setScope({ scope: "repo", repo_id: repoId });
              setExtensionsOpen(true);
            }}
          />
        </nav>

        <main aria-label="Terminal bay" class="terminal-bay relative min-h-0 overflow-hidden bg-background">
          <div
            class={`absolute inset-0 ${extensionsOpen() ? "warm-terminal-hidden" : ""}`}
            aria-hidden={extensionsOpen() ? "true" : undefined}
            inert={extensionsOpen()}
          >
            <TerminalWorkspace fleet={fleet} actions={actions} workspace={workspace} />
          </div>
          <Show when={extensionsOpen()}>
            <div class="absolute inset-0 z-10 bg-background">
              <ExtensionsView store={ext} fleet={fleet} />
            </div>
          </Show>
        </main>

        <aside
          aria-label="Repomind"
          class="repomind-panel min-h-0 border-l border-line bg-surface"
        >
          <Show when={repomindOpen() && !repomindFull()}>
            <RepomindPanel onToggleFullscreen={() => setRepomindFull(true)} />
          </Show>
        </aside>
      </div>

      <Show when={repomindFull()}>
        <div class="fixed inset-0 z-50 flex flex-col bg-background" role="dialog" aria-modal="true" aria-label="Repomind, full screen">
          <RepomindPanel fullscreen onToggleFullscreen={() => setRepomindFull(false)} />
        </div>
      </Show>

      <footer
        role="status"
        aria-label="Daemon connection"
        class="connection-rail flex items-center justify-between border-t border-line bg-surface px-3.5 py-1.5 font-mono text-[11px] text-muted"
      >
        <div class="flex items-center gap-2 min-w-0">
          <div
            class="flex items-center gap-1.5 text-foreground font-medium cursor-default"
            title={`Daemon socket: ${connection().endpoint}`}
          >
            <span class={`status-light is-${connection().phase}`} aria-hidden="true" />
            <span class="uppercase tracking-wider text-[10px]">{phaseLabel(connection().phase)}</span>
          </div>
          <Show when={connection().message}>
            {(msg) => <span class="truncate text-fault ml-2 font-sans text-xs">{msg()}</span>}
          </Show>
        </div>

        <div class="flex items-center gap-4 text-muted shrink-0">
          <span>
            {connection().daemon?.repos ?? 0} repos / {connection().daemon?.lanes ?? 0} lanes
          </span>
          <span>Uptime {formatUptime(connection().daemon?.uptime_secs)}</span>
          <span>App {appVersion() || "--"} · daemon {connection().daemon?.version ?? "--"}</span>
        </div>
      </footer>

      <ActionModals actions={actions} notifications={notifications} />
      <Show when={actions.error() ?? fleet.error()}>
        {(message) => (
          <div role="alert" class="fixed right-4 top-14 z-[70] flex max-w-md items-start gap-3 rounded-xl border border-fault/30 bg-surface p-3 text-xs text-fault shadow-[0_14px_40px_var(--shadow)]">
            <span class="flex-1 font-medium">{message()}</span>
            <button
              type="button"
              class="focus-ring -mr-1 -mt-1 flex size-5 items-center justify-center rounded text-muted hover:text-foreground"
              aria-label="Dismiss error"
              onClick={() => {
                actions.dismissError();
                fleet.dismissError();
              }}
            >
              <IconClose size={12} />
            </button>
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
