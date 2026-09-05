import { Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";

import ActionModals from "./components/ActionModals";
import FleetSidebar from "./components/FleetSidebar";
import ControlCenter from "./components/ControlCenter";
import ExtensionsView from "./components/ExtensionsView";
import UsageView from "./components/UsageView";
import Onboarding from "./components/Onboarding";
import WindowChromeHeader from "./components/WindowChrome";
import RepomindPanel from "./components/RepomindPanel";
import { RepomindStateDot } from "./components/RepomindRow";
import RightPanelHost, {
  RIGHT_PANEL_DEFAULT_WIDTH_PX,
  RIGHT_PANEL_MAX_WIDTH_PX,
  RIGHT_PANEL_MIN_WIDTH_PX,
} from "./components/RightPanelHost";
import { ResizableSplit } from "./components/ResizableSplit";
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
import { BINDINGS, formatChord, matchChord, matchSidebarKey } from "./keymap";
import BrandLockup from "./components/BrandLockup";
import { setAgentIconOverrides } from "./components/icons";
import { applyAccent, applyTheme, nextTheme, readTheme, type Theme } from "./theme";
import { createExtensionsStore } from "./stores/extensions";
import { createUsageStore } from "./stores/usage";
import { createEditorStore } from "./stores/editor";
import { createFleetStore, type FleetSource } from "./stores/fleet";
import { createNotificationStore } from "./stores/notifications";
import { createRepomindStore } from "./stores/repomind";
import { createMessageStore } from "./stores/messages";
import { createWorkspaceStore } from "./stores/workspace";
import { readOnboardingStep } from "./stores/onboarding";
import {
  notifyLayoutChanged,
  readOnboardingCompleted,
  readShortcutsHintLaunchCount,
  recordShortcutsHintLaunch,
  saveOnboardingCompleted,
  SHORTCUTS_HINT_MAX_LAUNCHES,
} from "./stores/uiSettings";
import EditorWorkspace from "./components/EditorWorkspace";
import FileFinder from "./components/FileFinder";
import ShortcutsOverlay from "./components/ShortcutsOverlay";
import { IconChevronDown, IconClose, IconExtensions, IconGitBranch, IconLayers, IconMail, IconMeter, IconMultitask, IconSettings, IconShield, IconSparkles } from "./components/icons";

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
    case "stopped":
      return "Stopped";
  }
}

function formatUptime(totalSeconds?: number): string {
  if (totalSeconds === undefined) return "--";
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  return hours > 0 ? `${hours}h ${minutes.toString().padStart(2, "0")}m` : `${minutes}m`;
}

const REPOMIND_OPEN_STORAGE_KEY = "repomon.repomind_open";

function readRepomindOpen(): boolean {
  try {
    const raw = localStorage.getItem(REPOMIND_OPEN_STORAGE_KEY);
    return raw !== null ? raw === "true" : false;
  } catch {
    return false;
  }
}

function persistRepomindOpen(open: boolean) {
  try {
    localStorage.setItem(REPOMIND_OPEN_STORAGE_KEY, String(open));
  } catch {}
}

/// Look up a chord by binding id in keymap.ts's BINDINGS and format it for display, so a header
/// toolbar button's title can never drift out of sync with what the chord actually does. Mirrors
/// ControlCenter.tsx's helper of the same name and purpose. Returns undefined for an id with no
/// chord, or none registered.
function chordFor(id: string): string | undefined {
  const binding = BINDINGS.find((entry) => entry.id === id);
  return binding ? formatChord(binding.chord) : undefined;
}

function App(props: AppProps) {
  const [theme, setTheme] = createSignal(readTheme());
  const [connection, setConnection] = createSignal(initialConnection);
  const [repomindOpen, setRepomindOpen] = createSignal(readRepomindOpen());
  const [repomindFull, setRepomindFull] = createSignal(false);
  // C1: which right-rail tab is currently showing, and a one-shot command to switch to it — see
  // RightPanelHost's `requestTab`/`onActiveTabChange` props for why this isn't just local state.
  // Shared by every panel.* shortcut (git for C1, editor for D4), since RightPanelHost's
  // `requestTab` prop is a single slot, not one per tab id.
  const [rightPanelTab, setRightPanelTab] = createSignal("repomind");
  const [panelTabRequest, setPanelTabRequest] = createSignal<{ id: string; token: number } | null>(null);
  let panelTabRequestToken = 0;
  const [onboardingOpen, setOnboardingOpen] = createSignal(false);
  const [extensionsOpen, setExtensionsOpen] = createSignal(false);
  // The Usage view takes the whole bay: a timeline, five breakdowns and a sessions table do not
  // fit the right rail, and reading them is its own task rather than something done beside a lane.
  const [usageOpen, setUsageOpen] = createSignal(false);
  const [update, setUpdate] = createSignal<AvailableUpdate | null>(null);
  const [appVersion, setAppVersion] = createSignal("");
  // Footer discoverability hint for the shortcuts guide, shown for a launch count decided once at
  // mount (see onMount below) rather than recomputed on every render, so it cannot flip mid-session.
  const [showShortcutsHint, setShowShortcutsHint] = createSignal(false);
  // E5: Debounced sustained-disconnect banner. The footer pill signals blips; this banner only
  // appears after the daemon has been unreachable for a continuous 5 seconds, so transient
  // reconnect cycles during daemon restart don't produce noise.
  const [sustainedDisconnect, setSustainedDisconnect] = createSignal(false);
  const source = props.connectionSource ?? tauriConnectionSource;
  const fleet = createFleetStore(props.fleetSource);
  const workspace = createWorkspaceStore(fleet);
  const editor = createEditorStore(fleet);
  const actions = createActionsStore(fleet, workspace);
  const ext = createExtensionsStore();
  const usage = createUsageStore();
  const repomind = createRepomindStore();
  const notifications = createNotificationStore((laneId) => fleet.setSelectedLaneId(laneId));
  const messages = createMessageStore((laneId, slot, sourceWindow) => {
    fleet.setSelectedLaneId(laneId);
    const lane = fleet.lanes().find((item) => item.id === laneId);
    const window = sourceWindow ?? lane?.agent_sessions[(slot ?? 1) - 1]?.tmux_window;
    if (window) fleet.setFocusedWindow(window);
  });
  let stopListening: (() => void) | undefined;
  let fleetStarted = false;
  let notificationsStarted = false;
  let messagesStarted = false;
  let repomindStarted = false;
  let searchInput: HTMLInputElement | undefined;
  let active = true;

  createEffect(() => {
    applyTheme(theme());
  });

  createEffect(() => {
    if (connection().phase === "connected" && fleet.synced()) {
      const repos = fleet.repos();
      const completed = readOnboardingCompleted();
      // A fresh install has no repos; a wizard abandoned midway has a stored resume step, and
      // usually a repo too, since adding one is step 3. Both cases reopen it.
      if (!completed && (repos.length === 0 || readOnboardingStep() !== null)) {
        setOnboardingOpen(true);
      }
    }
  });

  createEffect(() => {
    if (connection().phase === "connected" && !fleetStarted) {
      fleetStarted = true;
      fleet.start();
      void daemonCall("config.get")
        .then((config) => {
          if (config.theme && typeof config.theme === "string") {
            setTheme(config.theme as Theme);
            applyTheme(config.theme as Theme);
          }
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
    if (connection().phase === "connected" && !repomindStarted) {
      repomindStarted = true;
      repomind.start();
    } else if (connection().phase !== "connected" && repomindStarted) {
      repomindStarted = false;
      repomind.stop();
    }
  });

  // The status store refreshes itself the moment an agent in the controller lane changes state,
  // which is when its counts actually move; this keeps it told which windows those are.
  createEffect(() => {
    repomind.setControllerWindows(
      fleet
        .controllerLanes()
        .flatMap((lane) => lane.agent_sessions)
        .flatMap((session) => (session.tmux_window ? [session.tmux_window] : [])),
    );
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

  // Switching lanes should surface that lane's agent session, not leave the Extensions
  // panel covering it.
  createEffect(() => {
    fleet.selectedLaneId();
    setExtensionsOpen(false);
  });

  createEffect(() => {
    // Notify terminal panes whenever sibling panels or fullscreens change dimensions
    extensionsOpen();
    repomindOpen();
    repomindFull();
    notifyLayoutChanged();
  });

  createEffect(() => {
    if (!workspace.multitasking()) return;
    setExtensionsOpen(false);
    setRepomindOpen(false);
    setRepomindFull(false);
    persistRepomindOpen(false);
  });

  // Shared open/switch/toggle-close behavior for the mail, git, editor, and supervision right-rail
  // tabs, used by both the mod+2 / mod+3 / mod+7 / mod+8 shortcuts and their header icon-button
  // counterparts (item 4) so the entry points can never drift apart.
  const openPanelTab = (id: "repomind" | "git" | "editor" | "mail" | "supervision") => {
    // The right rail and fleet-wide grid use mutually exclusive mission-grid column models.
    // Entering multitasking already closes the rail above; make opening a rail tab symmetric.
    workspace.setMultitasking(false);
    workspace.setEditorWorkspace(false);
    if (!repomindOpen()) {
      // Closed → open already on the requested tab. RightPanelHost consults `requestTab.id` on its
      // very first render, so bumping this in the same tick as opening is enough — no need to wait
      // for the panel to mount before it takes effect.
      setPanelTabRequest({ id, token: ++panelTabRequestToken });
      setRepomindOpen(true);
      persistRepomindOpen(true);
    } else if (rightPanelTab() === id) {
      // Already open and already on this tab: mirrors the repomind toggle's own feel — activating
      // "the panel I'm looking at" closes it.
      setRepomindOpen(false);
      persistRepomindOpen(false);
    } else {
      // Open on some other tab: switch to the requested one without closing.
      setPanelTabRequest({ id, token: ++panelTabRequestToken });
    }
  };

  const isEditorActive = () =>
    workspace.editorWorkspace() || (repomindOpen() && rightPanelTab() === "editor");

  const toggleEditor = () => {
    if (workspace.editorWorkspace()) {
      workspace.setEditorWorkspace(false);
    } else if (repomindOpen() && rightPanelTab() === "editor") {
      setRepomindOpen(false);
      persistRepomindOpen(false);
    } else {
      workspace.setMultitasking(false);
      setRepomindOpen(false);
      persistRepomindOpen(false);
      workspace.setEditorWorkspace(true);
    }
  };

  const onShortcut = (event: KeyboardEvent) => {
    if (event.defaultPrevented) return;
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
      case "panel.control": actions.toggleControl(); break;
      case "panel.settings": actions.openSettings(); break;
      case "panel.multitasking": workspace.toggleMultitasking(); break;
      case "panel.extensions":
        workspace.setMultitasking(false);
        setUsageOpen(false);
        setExtensionsOpen((open) => !open);
        break;
      case "panel.usage":
        workspace.setMultitasking(false);
        setExtensionsOpen(false);
        setUsageOpen((open) => !open);
        break;
      case "panel.git": openPanelTab("git"); break;
      case "panel.editor": openPanelTab("editor"); break;
      case "panel.mail": openPanelTab("mail"); break;
      case "panel.supervision": openPanelTab("supervision"); break;
      case "finder.open":
        if (lane) editor.openFinder();
        break;
      case "search.project":
        if (lane) {
          if (!workspace.editorWorkspace() && (!repomindOpen() || rightPanelTab() !== "editor")) {
            openPanelTab("editor");
          }
        }
        break;
      case "editor.markdownPreview":
        if (lane) editor.toggleMarkdownPreview();
        break;
      case "panel.repomind":
        openPanelTab("repomind");
        break;
      case "panel.repomindFull":
        workspace.setMultitasking(false);
        if (!repomindFull()) {
          setRepomindOpen(true);
          persistRepomindOpen(true);
        }
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
      case "help.open": actions.openShortcutsGuide(); break;
      default:
        console.warn(`No handler for keyboard binding: ${binding.id}`);
        break;
    }
  };

  // The bare "?" alternative to mod+? for opening the shortcuts guide (help.open above already
  // covers mod+?). Unlike every chord in keymap.ts this carries no modifier at all, so it needs
  // its own guard against stealing input: it only fires outside a text input, and only when no
  // other modal already owns the keyboard.
  const onBareHelpKey = (event: KeyboardEvent) => {
    if (event.defaultPrevented || event.key !== "?" || event.metaKey || event.ctrlKey || event.altKey) return;
    const target = event.target;
    if (
      target instanceof HTMLInputElement
      || target instanceof HTMLTextAreaElement
      || target instanceof HTMLSelectElement
      || (target instanceof HTMLElement && target.isContentEditable)
    ) {
      return;
    }
    if (actions.settingsOpen() || actions.spawnLane() || actions.newLaneOpen()
      || actions.renameTarget() || actions.confirmOptions()) return;
    event.preventDefault();
    actions.openShortcutsGuide();
  };

  onMount(() => {
    window.addEventListener("keydown", onShortcut);
    window.addEventListener("keydown", onBareHelpKey);
    setShowShortcutsHint(readShortcutsHintLaunchCount() < SHORTCUTS_HINT_MAX_LAUNCHES);
    recordShortcutsHintLaunch();
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
    window.removeEventListener("keydown", onBareHelpKey);
    stopListening?.();
    fleet.stop();
    notifications.stop();
    messages.stop();
    repomind.stop();
  });

  const cycleTheme = () => {
    const value = nextTheme(theme());
    setTheme(value);
    applyTheme(value);
    void daemonCall("config.set", { theme: value }).catch(() => undefined);
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

  // E5: Debounced 5 s sustained-disconnect banner. React immediately on reconnect; only show
  // the banner if the daemon has been continuously unreachable for at least 5 seconds so that
  // transient blips during daemon restart don't produce noise.
  createEffect(() => {
    const phase = connection().phase;
    if (phase === "connected") {
      setSustainedDisconnect(false);
      return;
    }
    const timer = setTimeout(() => setSustainedDisconnect(true), 5000);
    onCleanup(() => clearTimeout(timer));
  });

  // The bare-key match itself lives in keymap.ts's matchSidebarKey, described there as "sidebar"
  // scope bindings, so the shortcuts guide and this dispatch can never disagree about what "j"
  // does here.
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
    const action = matchSidebarKey(event);
    if (!action) return;
    event.preventDefault();
    switch (action) {
      case "sidebar.filter": searchInput?.focus(); break;
      case "sidebar.next": fleet.moveSelection(1); break;
      case "sidebar.prev": fleet.moveSelection(-1); break;
      case "sidebar.jumpUrgent": fleet.moveSelection(1, true); break;
    }
  };

  return (
    <div class="grid h-screen min-h-[36rem] grid-rows-[35px_minmax(0,1fr)_2rem] overflow-hidden bg-background text-foreground">
      <WindowChromeHeader>
        <BrandLockup
          heading
          onActivate={() => actions.openSettingsTab("general")}
          activateLabel="About Repomon"
        />

        <div class="flex items-center">
          <button
            type="button"
            class={`focus-ring flex h-7 items-center gap-1.5 px-2 text-xs font-medium transition-colors ${
              repomindOpen() && rightPanelTab() === "git"
                ? "text-signal font-semibold"
                : "text-muted hover:text-foreground"
            }`}
            onClick={() => openPanelTab("git")}
            aria-pressed={repomindOpen() && rightPanelTab() === "git"}
            title={`Git (${chordFor("panel.git")})`}
          >
            <IconGitBranch size={13} />
            <span>Git</span>
          </button>
          <span class="h-3.5 w-px bg-line/60 mx-1" aria-hidden="true" />
          <div class="flex items-center">
            <button
              type="button"
              class={`focus-ring flex h-7 items-center gap-1.5 rounded-l px-2 text-xs font-medium transition-colors ${
                isEditorActive()
                  ? "text-signal font-semibold"
                  : "text-muted hover:text-foreground"
              }`}
              onClick={toggleEditor}
              aria-pressed={isEditorActive()}
              title="Editor workspace (center mode)"
            >
              <IconLayers size={13} />
              <span>Editor</span>
            </button>
            <button
              type="button"
              class={`focus-ring flex h-7 items-center rounded-r px-1 text-xs transition-colors ${
                repomindOpen() && rightPanelTab() === "editor"
                  ? "text-signal font-semibold"
                  : "text-muted hover:text-foreground"
              }`}
              onClick={(e) => {
                e.stopPropagation();
                openPanelTab("editor");
              }}
              aria-pressed={repomindOpen() && rightPanelTab() === "editor"}
              title={`Toggle compact editor in side rail (${chordFor("panel.editor")})`}
            >
              <IconChevronDown size={10} />
            </button>
          </div>
          <span class="h-3.5 w-px bg-line/60 mx-1" aria-hidden="true" />
          <ControlCenter fleet={fleet} notifications={notifications} messages={messages} actions={actions} />
          <span class="h-3.5 w-px bg-line/60 mx-1" aria-hidden="true" />
          <button
            type="button"
            class={`focus-ring flex h-7 items-center gap-1.5 px-2 text-xs font-medium transition-colors ${
              workspace.multitasking()
                ? "text-signal font-semibold"
                : "text-muted hover:text-foreground"
            }`}
            onClick={workspace.toggleMultitasking}
            aria-pressed={workspace.multitasking()}
            title={`Multitasking (${chordFor("panel.multitasking")})`}
          >
            <IconMultitask size={13} />
            <span>Multitasking</span>
          </button>
          <span class="h-3.5 w-px bg-line/60 mx-1" aria-hidden="true" />
          <button
            type="button"
            class={`focus-ring flex h-7 items-center gap-1.5 px-2 text-xs font-medium transition-colors ${
              extensionsOpen()
                ? "text-signal font-semibold"
                : "text-muted hover:text-foreground"
            }`}
            onClick={() => {
              workspace.setMultitasking(false);
              setExtensionsOpen(!extensionsOpen());
            }}
            aria-pressed={extensionsOpen()}
            title={`Extensions (${chordFor("panel.extensions")})`}
          >
            <IconExtensions size={13} />
            <span>Extensions</span>
          </button>
          <span class="h-3.5 w-px bg-line/60 mx-1" aria-hidden="true" />
          <button
            type="button"
            class={`focus-ring flex h-7 items-center gap-1.5 px-2 text-xs font-medium transition-colors ${
              repomindOpen() && rightPanelTab() === "supervision"
                ? "text-signal font-semibold"
                : "text-muted hover:text-foreground"
            }`}
            onClick={() => openPanelTab("supervision")}
            aria-pressed={repomindOpen() && rightPanelTab() === "supervision"}
            title={`Supervision (${chordFor("panel.supervision")})`}
          >
            <IconShield size={13} />
            <span>Supervision</span>
          </button>
          <button
            type="button"
            class={`focus-ring flex h-7 items-center gap-1.5 px-2 text-xs font-medium transition-colors ${
              usageOpen() ? "text-signal font-semibold" : "text-muted hover:text-foreground"
            }`}
            onClick={() => {
              workspace.setMultitasking(false);
              setExtensionsOpen(false);
              setUsageOpen((open) => !open);
            }}
            aria-pressed={usageOpen()}
            title={`Usage (${chordFor("panel.usage")})`}
          >
            <IconMeter size={13} />
            <span>Usage</span>
          </button>
          <span class="h-3.5 w-px bg-line/60 mx-1" aria-hidden="true" />
          <button
            type="button"
            class={`focus-ring flex h-7 items-center gap-1.5 px-2 text-xs font-medium transition-colors ${
              repomindOpen() && rightPanelTab() === "mail"
                ? "text-signal font-semibold"
                : "text-muted hover:text-foreground"
            }`}
            onClick={() => openPanelTab("mail")}
            aria-pressed={repomindOpen() && rightPanelTab() === "mail"}
            title={`Repomail (${chordFor("panel.mail")})`}
          >
            <IconMail size={13} />
            <span>Repomail</span>
            <Show when={messages.unread() > 0}>
              <span class="rounded-full bg-signal/15 px-1 font-mono text-[9px] text-signal">{messages.unread()}</span>
            </Show>
          </button>
          <span class="h-3.5 w-px bg-line/60 mx-1" aria-hidden="true" />
          <button
            type="button"
            class={`focus-ring flex h-7 items-center gap-1.5 px-2 text-xs font-medium transition-colors ${
              repomindOpen() && rightPanelTab() === "repomind"
                ? "text-signal font-semibold"
                : "text-muted hover:text-foreground"
            }`}
            onClick={() => openPanelTab("repomind")}
            aria-pressed={repomindOpen() && rightPanelTab() === "repomind"}
            title={`Repomind (${chordFor("panel.repomind")})`}
          >
            <IconSparkles size={13} />
            <span>Repomind</span>
            <RepomindStateDot controller={fleet.controller()} />
          </button>
          <span class="h-3.5 w-px bg-line/60 mx-1" aria-hidden="true" />
          <button
            type="button"
            class="focus-ring flex size-7 items-center justify-center text-muted transition-colors hover:text-foreground"
            onClick={() => actions.openSettings()}
            aria-label="Settings"
            title={`Settings (${chordFor("panel.settings")})`}
          >
            <IconSettings size={14} />
          </button>
        </div>
      </WindowChromeHeader>

      {/* E5: Sustained-disconnect banner — only after ≥5 s of continuous disconnection.
          Rendered as a fixed overlay (non-blocking) so the main layout never shifts.
          The footer pill stays visible for shorter blips. */}
      <Show when={sustainedDisconnect()}>
        <div
          role="alert"
          aria-live="assertive"
          class="fixed inset-x-0 top-[35px] z-50 flex items-center justify-between gap-3 border-b border-attention/40 bg-attention/10 px-4 py-2 text-[11px] text-attention backdrop-blur-sm"
        >
          <span class="flex items-center gap-2 font-medium">
            <span class="inline-block h-1.5 w-1.5 shrink-0 rounded-full bg-attention" aria-hidden="true" />
            Daemon unreachable — reconnecting…
          </span>
          <button
            type="button"
            class="focus-ring rounded px-2 py-0.5 font-mono text-[10px] uppercase tracking-wider text-attention hover:bg-attention/20 transition-colors"
            onClick={() => actions.openSettingsTab("system")}
          >
            Settings › System
          </button>
        </div>
      </Show>

      <div class={`mission-grid ${repomindOpen() ? "is-repomind-open" : ""} ${workspace.multitasking() ? "is-multitasking" : ""}`}>
        <Show when={!workspace.multitasking()}>
          <nav
          aria-label="Fleet"
          class="flex min-h-0 flex-col border-r border-line bg-surface outline-none"
          tabIndex={0}
          onKeyDown={navigateFleet}
        >
          <div class="flex h-10 shrink-0 items-center justify-between border-b border-line bg-surface/95 px-3.5">
            <span class="section-label">Fleet</span>
            <span class="font-mono text-[10px] text-muted">
              {fleet.visibleRepos().length} repos · {fleet.fleetLanes().length} lanes
            </span>
          </div>
          <FleetSidebar
            fleet={fleet}
            actions={actions}
            workspace={workspace}
            repomind={repomind}
            searchRef={(element) => { searchInput = element; }}
            onOpenRepomindPanel={() => openPanelTab("repomind")}
            onOpenEditor={() => {
              workspace.setMultitasking(false);
              setRepomindOpen(false);
              persistRepomindOpen(false);
              workspace.setEditorWorkspace(true);
            }}
            onOpenExtensions={(repoId) => {
              ext.setScope({ scope: "repo", repo_id: repoId });
              setExtensionsOpen(true);
            }}
          />
          </nav>
        </Show>

        <main aria-label="Terminal bay" class="terminal-bay relative min-h-0 overflow-hidden bg-background">
          <div
            class={`absolute inset-0 ${extensionsOpen() || workspace.editorWorkspace() ? "warm-terminal-hidden" : ""}`}
            aria-hidden={extensionsOpen() || workspace.editorWorkspace() ? "true" : undefined}
            inert={extensionsOpen() || workspace.editorWorkspace()}
          >
            <TerminalWorkspace
              fleet={fleet}
              actions={actions}
              workspace={workspace}
              editor={editor}
              onEnsureEditorOpen={() => {
                if (!isEditorActive()) {
                  workspace.setMultitasking(false);
                  setRepomindOpen(false);
                  persistRepomindOpen(false);
                  workspace.setEditorWorkspace(true);
                }
              }}
            />
          </div>
          <Show when={extensionsOpen()}>
            <div class="absolute inset-0 z-10 bg-background">
              <ExtensionsView store={ext} fleet={fleet} />
            </div>
          </Show>
          <Show when={usageOpen()}>
            <div class="absolute inset-0 z-10 bg-background">
              <UsageView
                store={usage}
                fleet={fleet}
                onOpenLane={(laneId) => {
                  fleet.setSelectedLaneId(laneId);
                  setUsageOpen(false);
                }}
              />
            </div>
          </Show>
          <Show when={workspace.editorWorkspace()}>
            <div class="absolute inset-0 z-10 bg-background">
              <EditorWorkspace
                fleet={fleet}
                editor={editor}
                actions={actions}
                onOpenFinder={() => editor.openFinder()}
              />
            </div>
          </Show>
        </main>

        <aside
          aria-label="Repomind"
          class="repomind-panel min-h-0 bg-surface"
        >
          <Show when={repomindOpen() && !repomindFull()}>
            <div class="flex h-full min-h-0 flex-row">
              <ResizableSplit
                storageKey="repomon:right-panel-width"
                defaultWidth={RIGHT_PANEL_DEFAULT_WIDTH_PX}
                minWidth={RIGHT_PANEL_MIN_WIDTH_PX}
                maxWidth={RIGHT_PANEL_MAX_WIDTH_PX}
                panelSide="after"
                cssVar="--right-panel-width"
                label="Resize right panel"
              />
              {/* min-w-0 is load-bearing: this is a flex-1 item of the row above (ResizableSplit
                  handle + this pane), and without it the classic flexbox "automatic minimum size"
                  rule lets its content's min-content width (e.g. an unwrapped long code line deep
                  in CodeMirror, or a diff's long line) win, pushing this pane wider than the
                  resizable rail instead of letting the descendant scrollers (`.cm-scroller`, the
                  editor tab strip) handle their own horizontal overflow. The `aside` ancestor's
                  `overflow: hidden` was then silently clipping the excess instead of scrolling. */}
              <div class="flex min-h-0 min-w-0 flex-1 flex-col border-l border-line">
                <RightPanelHost
                  onToggleFullscreen={() => setRepomindFull(true)}
                  fleet={fleet}
                  actions={actions}
                  messages={messages}
                  editor={editor}
                  workspace={workspace}
                  onEnsureEditorOpen={() => {
                    workspace.setMultitasking(false);
                    workspace.setEditorWorkspace(true);
                  }}
                  repomind={repomind}
                  requestTab={panelTabRequest()}
                  onActiveTabChange={setRightPanelTab}
                />
              </div>
            </div>
          </Show>
        </aside>
      </div>

      <Show when={repomindFull()}>
        <div class="fixed inset-0 z-50 flex flex-col bg-background" role="dialog" aria-modal="true" aria-label="Repomind, full screen">
          <RepomindPanel
            fullscreen
            onToggleFullscreen={() => setRepomindFull(false)}
            fleet={fleet}
            repomind={repomind}
            actions={actions}
            editor={editor}
            onEnsureEditorOpen={() => {
              setRepomindFull(false);
              workspace.setMultitasking(false);
              workspace.setEditorWorkspace(true);
            }}
          />
        </div>
      </Show>

      <footer
        role="status"
        aria-label="Daemon connection"
        class="connection-rail flex items-center justify-between border-t border-line bg-surface px-3.5 py-1.5 font-mono text-[11px] text-muted"
      >
        <div class="flex items-center gap-2 min-w-0">
          <button
            type="button"
            class="focus-ring flex items-center gap-1.5 rounded-md px-1.5 py-0.5 -mx-1.5 text-foreground font-medium hover:bg-line/40 transition-colors cursor-pointer text-left"
            title={`Daemon socket: ${connection().endpoint} · Click to view System Health`}
            aria-label="View system health and daemon connection"
            onClick={() => actions.openSettingsTab("system")}
          >
            <span class={`status-light is-${connection().phase}`} aria-hidden="true" />
            <span class="uppercase tracking-wider text-[10px]">{phaseLabel(connection().phase)}</span>
          </button>
          <Show when={connection().message}>
            {(msg) => <span class="truncate text-fault ml-2 font-sans text-xs">{msg()}</span>}
          </Show>
        </div>

        <div class="flex items-center gap-4 text-muted shrink-0">
          <Show when={showShortcutsHint()}>
            <button
              type="button"
              class="focus-ring rounded px-1 -mx-1 text-signal hover:text-foreground transition-colors"
              onClick={() => actions.openShortcutsGuide()}
            >
              {formatChord("mod+?")} for shortcuts
            </button>
          </Show>
          <span>
            {connection().daemon?.repos ?? 0} repos / {connection().daemon?.lanes ?? 0} lanes
          </span>
          <span>Uptime {formatUptime(connection().daemon?.uptime_secs)}</span>
          <span>App {appVersion() || "--"} · daemon {connection().daemon?.version ?? "--"}</span>
        </div>
      </footer>

      <Show when={onboardingOpen()}>
        <div
          class="fixed inset-0 z-50 flex flex-col bg-background"
          role="dialog"
          aria-modal="true"
          aria-label="Welcome to Repomon"
        >
          <Onboarding
            actions={actions}
            notifications={notifications}
            onComplete={() => {
              saveOnboardingCompleted(true);
              setOnboardingOpen(false);
            }}
            onSkip={() => {
              saveOnboardingCompleted(true);
              setOnboardingOpen(false);
            }}
          />
        </div>
      </Show>

      <ActionModals
        actions={actions}
        notifications={notifications}
        onReplayOnboarding={() => setOnboardingOpen(true)}
      />
      <FileFinder
        editor={editor}
        isOpen={editor.finderOpen()}
        onClose={() => editor.closeFinder()}
        onOpenPath={(path) => {
          if (!workspace.editorWorkspace() && (!repomindOpen() || rightPanelTab() !== "editor")) {
            openPanelTab("editor");
          }
          void editor.openFile(path);
        }}
      />
      <ShortcutsOverlay actions={actions} />
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
