import { ClipboardAddon } from "@xterm/addon-clipboard";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal, type ILink } from "@xterm/xterm";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";

import { daemonCall } from "../ipc/rpc";
import {
  createInputCoalescer,
  isTerminalFindChord,
  isTerminalReleaseChord,
  recordTrace,
  takeWheelBatch,
  terminalPointerCell,
  translateKeyboardKey,
  watchTerminal,
  wheelLines,
  type TerminalRenderer,
  type TerminalTarget,
} from "../ipc/term";
import {
  readTerminalAppearance,
  terminalFontFamily,
  terminalSurfaceStyle,
  type TerminalAppearance,
  type TerminalThemeTokens,
} from "../theme";
import { onLayoutChanged } from "../stores/uiSettings";
import type { FleetStore } from "../stores/fleet";
import type { EditorStore } from "../stores/editor";
import type { WorkspaceStore } from "../stores/workspace";
import ConversationPane from "./ConversationPane";
import ViewToggle from "./controls/ViewToggle";
import TranscriptDetailToggle, { type TranscriptDetail } from "./controls/TranscriptDetailToggle";
import { type AgentView } from "../stores/agentViews";
import { matchChord } from "../keymap";
import { IconArrowDown, IconArrowUp, IconClose, IconSearch } from "./icons";
import { multitaskRowFloor } from "./terminalMetrics";
import { findPathRefs, isMacPlatform } from "./terminalPathLinks";
import { markChatLatency } from "../ipc/chatLatency";

interface TerminalPaneProps extends TerminalTarget {
  label: string;
  renderer?: TerminalRenderer;
  focused?: boolean;
  visible?: boolean;
  /// Keep dashboard panes pinned to their live prompt instead of preserving stale scrollback.
  followTail?: boolean;
  /// Report this pane's fixed Multitasking row-height floor (chrome plus room for
  /// `MULTITASK_MIN_ROWS` rows at the renderer's cell height, see `terminalMetrics.ts`).
  /// Multitasking takes the max across all visible panes to size its grid rows.
  onMinimumHeight?: (pixels: number) => void;
  /// A GUI-owned shell (no other viewer): safe to force the pane to our size so it always fits.
  shell?: boolean;
  fleet?: FleetStore;
  editor?: EditorStore;
  workspace?: WorkspaceStore;
  onEnsureEditorOpen?: () => void;
}

interface TerminalHostInsets {
  left: number;
  right: number;
  top: number;
  bottom: number;
}

export function devicePixelAlignedInsets(
  insets: TerminalHostInsets,
  left: number,
  top: number,
  devicePixelRatio: number,
): TerminalHostInsets {
  const dpr = Number.isFinite(devicePixelRatio) && devicePixelRatio > 0
    ? devicePixelRatio
    : 1;
  const dx = Math.round(left * dpr) / dpr - left;
  const dy = Math.round(top * dpr) / dpr - top;
  return {
    left: insets.left + dx,
    right: insets.right - dx,
    top: insets.top + dy,
    bottom: insets.bottom - dy,
  };
}

// Retry until CSS assigns visible cell geometry, with a frame limit for panes that remain
// collapsed.
const VISIBLE_LAYOUT_RETRY_FRAMES = 30;

function terminalTheme(element: HTMLElement, appearance?: TerminalAppearance) {
  // The theme vars hold modern color syntax (space-separated hsl()) that xterm's color
  // parser rejects - it then silently falls back to its defaults (pure-black background,
  // visibly darker than the app's). Resolve each var through the browser to plain rgb().
  const resolve = (value: string) => {
    const probe = document.createElement("span");
    probe.style.color = value;
    element.appendChild(probe);
    const rgb = getComputedStyle(probe).color;
    probe.remove();
    return rgb;
  };
  const style = getComputedStyle(element);
  const varColor = (name: string) => resolve(style.getPropertyValue(name).trim());
  const tokens: TerminalThemeTokens = {
    background: varColor("--background"),
    foreground: varColor("--foreground"),
    signal: varColor("--signal"),
  };
  const [r, g, b] = tokens.signal.match(/\d+(?:\.\d+)?/g) ?? ["100", "196", "187"];
  const app = appearance ?? readTerminalAppearance();
  const surface = terminalSurfaceStyle(app, tokens);
  // surface.background is a color-mix() expression when tinted, built from the already-resolved
  // rgb() tokens above. xterm's own color parser can't handle color-mix() (see the comment at
  // the top of this function), so resolve it through the browser once more into a plain rgb().
  const bg = surface.background === tokens.background ? surface.background : resolve(surface.background);

  return {
    background: bg,
    foreground: surface.foreground,
    cursor: tokens.signal,
    selectionBackground: `rgba(${r}, ${g}, ${b}, 0.24)`,
    black: varColor("--ansi-black"),
    red: varColor("--ansi-red"),
    green: varColor("--ansi-green"),
    yellow: varColor("--ansi-yellow"),
    blue: varColor("--ansi-blue"),
    magenta: varColor("--ansi-magenta"),
    cyan: varColor("--ansi-cyan"),
    white: varColor("--ansi-white"),
    brightBlack: varColor("--ansi-bright-black"),
    brightRed: varColor("--ansi-bright-red"),
    brightGreen: varColor("--ansi-bright-green"),
    brightYellow: varColor("--ansi-bright-yellow"),
    brightBlue: varColor("--ansi-bright-blue"),
    brightMagenta: varColor("--ansi-bright-magenta"),
    brightCyan: varColor("--ansi-bright-cyan"),
    brightWhite: varColor("--ansi-bright-white"),
  };
}

export default function TerminalPane(props: TerminalPaneProps) {
  let pane!: HTMLElement;
  let container!: HTMLDivElement;
  let searchInput!: HTMLInputElement;
  let terminal: Terminal | undefined;
  let search: SearchAddon | undefined;
  let webgl: WebglAddon | undefined;
  let fit: FitAddon | undefined;
  let input: ReturnType<typeof createInputCoalescer> | undefined;
  let resize: ResizeObserver | undefined;
  let layoutMutation: MutationObserver | undefined;
  let intersection: IntersectionObserver | undefined;
  let onWindowResize: (() => void) | undefined;
  let unsubLayout: (() => void) | undefined;
  let onAppearanceChanged: ((e: Event) => void) | undefined;
  let resizeTimer: ReturnType<typeof setTimeout> | undefined;
  let syncTimer: ReturnType<typeof setTimeout> | undefined;
  let wheelListener: ((event: WheelEvent) => void) | undefined;
  let wheelFrame: number | undefined;
  let visibilityFrame: number | undefined;
  let visibleSyncFrame: number | undefined;
  let stopWatch: (() => Promise<void>) | undefined;
  let syncSize: (() => Promise<void>) | undefined;
  let rendererEpoch = 0;
  let hostInsets: TerminalHostInsets | undefined;
  let disposed = false;
  let scrollRequestInFlight = false;
  let syncInFlight = false;
  let pendingSync = false;
  let retryWatch: (() => void) | undefined;
  // Generation checks prevent a delayed retry from replacing or orphaning a newer watch.
  let watchRun = 0;
  const [viewError, setViewError] = createSignal<string | null>(null);
  const [transportError, setTransportError] = createSignal<string | null>(null);
  const [retrying, setRetrying] = createSignal(false);
  const [ready, setReady] = createSignal(false);
  const [finding, setFinding] = createSignal(false);
  const [query, setQuery] = createSignal("");
  const [detail, setDetail] = createSignal<TranscriptDetail>("normal");
  const [headline, setHeadline] = createSignal<string | null>(null);
  const lane = createMemo(() => props.fleet?.lanes().find((item) => item.id === props.laneId));
  const taskTitle = () => headline()?.trim() || lane()?.worktree.branch || lane()?.worktree.name || props.label;
  createEffect(() => {
    const laneId = props.laneId;
    let disposed = false;
    setHeadline(null);
    void daemonCall("lane.headline", { lane_id: laneId }).then((value) => { if (!disposed) setHeadline(value); }).catch(() => undefined);
    onCleanup(() => { disposed = true; });
  });
  const [localView, setLocalView] = createSignal<AgentView>("terminal");
  const windowSession = createMemo(() => props.fleet?.lanes().find((item) => item.id === props.laneId)?.agent_sessions?.find((agent) => agent.tmux_window === props.window));
  const agentKind = createMemo(() => windowSession()?.agent ?? "agent");
  // Looked up fresh by (laneId, window) rather than trusting props.sessionId, which is threaded
  // down through TerminalWorkspace's target-stabilization cache and can lag behind a window
  // switching agents. A non-Claude session has no identity except its tmux_window (the daemon
  // never assigns it a session_id), so sending one at all - stale or not - now fails the RPC
  // outright once the daemon makes window authoritative; omit it whenever this window's own
  // session doesn't actually carry one.
  const windowSessionId = createMemo(() => windowSession()?.session_id ?? undefined);
  const view = createMemo(() => props.workspace?.viewFor?.({ laneId: props.laneId, agent: agentKind(), shell: !!props.shell }) ?? localView());
  const [command, setCommand] = createSignal<string | null>(null);
  const commandOpen = () => view() === "conversation" && command() !== null;
  const terminalShown = () => view() === "terminal" || commandOpen();
  createEffect(() => {
    commandOpen();
    // The same canvas changes its top inset when the command heading is present.
    hostInsets = undefined;
    if (container) for (const edge of ["top", "right", "bottom", "left"]) container.style.removeProperty(edge);
  });
  async function openCommand(text?: string) {
    setCommand(text ?? "Agent controls");
    try {
      if (text) await daemonCall("agent.send_input", { lane_id: props.laneId, window: props.window, text, enter: true });
      terminal?.scrollToBottom();
      terminal?.focus();
    } catch (error) { setCommand(null); throw error; }
  }
  function closeCommand() {
    setCommand(null);
    requestAnimationFrame(() => pane?.querySelector<HTMLTextAreaElement>(".conversation-compose textarea")?.focus());
  }
  const setView = (value: AgentView | null) => {
    if (value === "conversation") markChatLatency("chat_clicked", { lane_id: props.laneId, window: props.window });
    setCommand(null);
    setFinding(false);
    setViewError(null);
    if (props.workspace) void props.workspace.setView(props.laneId, value).catch((error) => setViewError(String(error)));
    else setLocalView(value ?? "terminal");
  };
  const [paneBg, setPaneBg] = createSignal<string>("");

  createEffect(() => {
    const laneId = props.laneId;
    if (typeof laneId === "number" && props.editor) {
      void props.editor.ensureIndex(laneId);
    }
  });

  function errorMessage(error: unknown) {
    return error instanceof Error ? error.message : String(error);
  }

  function followTailIfNeeded() {
    if (
      !disposed
      && props.followTail
      && props.visible !== false
      && terminalShown()
    ) {
      terminal?.scrollToBottom();
    }
  }

  function reportMinimumHeight() {
    if (
      !props.followTail
      || props.visible === false
      || !terminal
      || !pane?.isConnected
      || !container?.isConnected
    ) return;
    const screen = terminal.element?.querySelector<HTMLElement>(".xterm-screen");
    if (!screen) return;
    const paneRect = pane.getBoundingClientRect();
    const hostRect = container.getBoundingClientRect();
    const screenRect = screen.getBoundingClientRect();
    if (paneRect.height <= 0 || hostRect.height <= 0 || screenRect.height <= 0) return;
    const chromeHeight = Math.max(0, hostRect.top - paneRect.top)
      + Math.max(0, paneRect.bottom - hostRect.bottom);

    // Derive the floor from cell height, not the current grid, to avoid feedback-driven row growth;
    // guard private xterm metrics and fall back to measured screen height per row.
    const core = (terminal as unknown as {
      _core?: { _renderService?: { dimensions?: { css?: { cell?: { height?: number } } } } };
    })._core;
    const rendererCellHeight = core?._renderService?.dimensions?.css?.cell?.height;
    const cellHeight = typeof rendererCellHeight === "number" && Number.isFinite(rendererCellHeight) && rendererCellHeight > 0
      ? rendererCellHeight
      : terminal.rows > 0 ? screenRect.height / terminal.rows : Number.NaN;
    if (!Number.isFinite(cellHeight) || cellHeight <= 0) return;

    props.onMinimumHeight?.(multitaskRowFloor({ chromeHeight, cellHeight }));
  }

  function alignTerminalHostToDevicePixels() {
    if (!container?.isConnected) return false;
    if (!hostInsets) {
      const style = getComputedStyle(container);
      const pixels = (value: string) => {
        const parsed = Number.parseFloat(value);
        return Number.isFinite(parsed) ? parsed : 0;
      };
      hostInsets = {
        left: pixels(style.left),
        right: pixels(style.right),
        top: pixels(style.top),
        bottom: pixels(style.bottom),
      };
    }

    // Measure from the unadjusted box each time. Otherwise the correction becomes the next
    // measurement's baseline and oscillates between aligned and fractional positions.
    container.style.left = `${hostInsets.left}px`;
    container.style.right = `${hostInsets.right}px`;
    container.style.top = `${hostInsets.top}px`;
    container.style.bottom = `${hostInsets.bottom}px`;
    const rect = container.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return false;
    const aligned = devicePixelAlignedInsets(
      hostInsets,
      rect.left,
      rect.top,
      window.devicePixelRatio,
    );
    const changed = aligned.left !== hostInsets.left || aligned.top !== hostInsets.top;
    container.style.left = `${aligned.left}px`;
    container.style.right = `${aligned.right}px`;
    container.style.top = `${aligned.top}px`;
    container.style.bottom = `${aligned.bottom}px`;
    return changed;
  }

  async function preloadTerminalFont() {
    if (typeof document === "undefined" || !document.fonts) return;
    await document.fonts.load('12px "Berkeley Mono"');
    await document.fonts.ready;
  }

  async function applyRenderer(requested: TerminalRenderer) {
    const epoch = ++rendererEpoch;
    webgl?.dispose();
    webgl = undefined;
    if (requested === "dom" || !terminal) return;

    try {
      await preloadTerminalFont();
      if (disposed || epoch !== rendererEpoch || !terminal) return;
      const addon = new WebglAddon();
      terminal.loadAddon(addon);
      if (disposed || epoch !== rendererEpoch) {
        addon.dispose();
        return;
      }
      webgl = addon;
      addon.onContextLoss(() => {
        if (webgl !== addon) return;
        addon.dispose();
        webgl = undefined;
      });
    } catch {
      webgl?.dispose();
      webgl = undefined;
    }
  }

  function find(next: boolean) {
    const value = query().trim();
    if (!value || !search) return;
    if (next) search.findNext(value, { incremental: true });
    else search.findPrevious(value, { incremental: true });
  }

  function openFind() {
    setFinding(true);
    queueMicrotask(() => {
      searchInput?.focus();
      searchInput?.select();
    });
  }

  createEffect(() => {
    const requested = props.renderer ?? "dom";
    if (ready()) void applyRenderer(requested);
  });

  createEffect(() => {
    const visible = ready() && props.visible !== false;
    const focused = props.focused;
    const followTail = props.followTail;
    if (visibleSyncFrame !== undefined) {
      cancelAnimationFrame(visibleSyncFrame);
      visibleSyncFrame = undefined;
    }
    if (!visible || !terminalShown() || disposed) return;
    let retriesRemaining = VISIBLE_LAYOUT_RETRY_FRAMES;
    const syncVisiblePane = () => {
      visibleSyncFrame = requestAnimationFrame(() => {
        visibleSyncFrame = undefined;
        if (
          disposed
          || props.visible === false
          || !terminalShown()
          || !terminal
          || !container?.isConnected
        ) return;
        if (
          (container.clientWidth === 0 || container.clientHeight === 0)
          && retriesRemaining > 0
        ) {
          retriesRemaining -= 1;
          syncVisiblePane();
          return;
        }
        const finish = () => {
          if (followTail) followTailIfNeeded();
          if (focused && !disposed && terminal) terminal.focus();
        };
        const syncing = syncSize?.();
        if (syncing) void syncing.then(finish, finish);
        else finish();
      });
    };
    syncVisiblePane();
  });


  onMount(() => {
    void (async () => {
      const target = { laneId: props.laneId, window: props.window };
      if ((props.renderer ?? "dom") !== "dom") await preloadTerminalFont();
      if (disposed) return;

      input = createInputCoalescer(target, (error) => setTransportError(errorMessage(error)));
      const initialApp = readTerminalAppearance();
      const initialTheme = terminalTheme(container, initialApp);
      setPaneBg(initialTheme.background);
      terminal = new Terminal({
        allowProposedApi: true,
        cursorBlink: true,
        cursorStyle: "bar",
        // Send hyperlinks to the system browser. xterm's default opens them with `window.open`,
        // which the webview treats as a navigation request: it prompts, and then goes nowhere
        // because the app's own window cannot navigate off to an external site.
        linkHandler: {
          activate: (_event, uri) => {
            void openUrl(uri).catch((error: unknown) => setTransportError(errorMessage(error)));
          },
        },
        fontFamily: terminalFontFamily(initialApp.fontFamily),
        fontSize: initialApp.fontSize,
        lineHeight: 1.18,
        scrollback: 10_000,
        smoothScrollDuration: 60,
        theme: initialTheme,
      });
      fit = new FitAddon();
      search = new SearchAddon();
      terminal.loadAddon(fit);
      terminal.loadAddon(search);
      terminal.loadAddon(new ClipboardAddon());
      terminal.loadAddon(new Unicode11Addon());
      terminal.unicode.activeVersion = "11";

      terminal.registerLinkProvider({
        provideLinks(bufferLineNumber: number, callback: (links: ILink[] | undefined) => void) {
          const bufferLine = terminal?.buffer.active.getLine(bufferLineNumber - 1);
          if (!bufferLine) {
            callback(undefined);
            return;
          }
          const lineText = bufferLine.translateToString(true);
          if (!lineText) {
            callback(undefined);
            return;
          }

          const matches = findPathRefs(lineText);
          if (matches.length === 0) {
            callback(undefined);
            return;
          }

          const laneId = props.laneId;
          const lane = typeof laneId === "number" ? props.fleet?.lanes().find((l) => l.id === laneId) : undefined;
          const worktreeRoot = lane?.worktree.path;

          const links: ILink[] = [];
          for (const match of matches) {
            let relPath: string | null = null;
            if (match.path.startsWith("/")) {
              if (worktreeRoot) {
                const normRoot = worktreeRoot.endsWith("/") ? worktreeRoot.slice(0, -1) : worktreeRoot;
                if (match.path === normRoot || match.path.startsWith(normRoot + "/")) {
                  relPath = match.path.slice(normRoot.length).replace(/^\/+/, "");
                }
              }
            } else {
              relPath = match.path.replace(/^\.\//, "");
            }

            if (!relPath) continue;

            // Only link paths that exist in the lane's file.index cache
            if (typeof laneId === "number" && props.editor) {
              if (!props.editor.isPathInIndex(laneId, relPath)) {
                continue;
              }
            }

            const targetRelPath = relPath;
            links.push({
              range: {
                start: { x: match.startIndex + 1, y: bufferLineNumber },
                end: { x: match.endIndex, y: bufferLineNumber },
              },
              text: match.raw,
              decorations: {
                underline: true,
                pointerCursor: true,
              },
              activate: (event: MouseEvent) => {
                const isMac = isMacPlatform();
                const isModifier = isMac ? event.metaKey : event.ctrlKey;
                if (!isModifier) return; // Plain click keeps default selection behavior
                if (props.onEnsureEditorOpen) {
                  props.onEnsureEditorOpen();
                } else if (props.workspace && !props.workspace.editorWorkspace()) {
                  props.workspace.setEditorWorkspace(true);
                }
                void props.editor?.openAt(targetRelPath, match.line ?? 1, match.column ?? 1);
              },
            });
          }

          callback(links.length > 0 ? links : undefined);
        },
      });

      terminal.open(container);

      terminal.attachCustomKeyEventHandler((event) => {
        if (event.type !== "keydown") return true;
        if (matchChord(event)?.id === "lane.toggleView") return false;
        if (isTerminalFindChord(event)) {
          event.preventDefault();
          openFind();
          return false;
        }
        // Shift+Escape hands focus back to the app shell so the fleet list can be driven by
        // keyboard. Plain Escape still goes to the agent: Claude Code uses it to interrupt.
        if (isTerminalReleaseChord(event)) {
          event.preventDefault();
          terminal?.blur();
          document.querySelector<HTMLElement>('nav[aria-label="Fleet"]')?.focus();
          return false;
        }
        const translated = translateKeyboardKey(event);
        if (!translated) return true;
        event.preventDefault();
        void input?.key(translated).catch((error: unknown) => setTransportError(errorMessage(error)));
        return false;
      });
      terminal.onData((data) => input?.push(data));

      // Use the backend-confirmed grid because width mismatches corrupt cursor-relative repaint
      // sequences.
      let confirmedGrid: { cols: number; rows: number } | null = null;
      const bufferedWrites: (string | Uint8Array)[] = [];

      function writeIncoming(bytes: string | Uint8Array) {
        if (disposed || !terminal) return;
        if (syncInFlight) {
          recordTrace("BUFFERED_WRITE", props.window, bytes);
          bufferedWrites.push(bytes);
        } else {
          recordTrace("XTERM_DIRECT_WRITE", props.window, bytes);
          terminal.write(bytes, followTailIfNeeded);
        }
      }

      function flushBufferedWrites() {
        if (disposed || !terminal || syncInFlight) return;
        while (bufferedWrites.length > 0) {
          const chunk = bufferedWrites.shift();
          if (chunk && !disposed && terminal) {
            recordTrace("XTERM_FLUSHED_WRITE", props.window, chunk);
            terminal.write(chunk, followTailIfNeeded);
          }
        }
      }

      // Apply the authoritative grid even when another viewer owns a taller size; clip locally and
      // follow the prompt instead of changing the layout floor.
      function applyGrid(cols?: number | null, rows?: number | null) {
        if (disposed || !terminal || !cols || !rows) return;
        alignTerminalHostToDevicePixels();
        confirmedGrid = { cols, rows };
        if (cols !== terminal.cols || rows !== terminal.rows) {
          try {
            terminal.resize(cols, rows);
          } catch {
            return;
          }
        }
        if (disposed || !terminal) return;
        try {
          terminal.refresh(0, Math.max(0, terminal.rows - 1));
        } catch {
          // ignore
        }
        reportMinimumHeight();
        followTailIfNeeded();
      }

      // Keep xterm and the backend pane on one authoritative grid. GUI-owned shells can be
      // resized directly. Shared agent panes use the arbitrated fit call so the TUI and desktop
      // never fight over dimensions.
      syncSize = async () => {
        if (disposed || !terminal || !fit || props.visible === false || !terminalShown()) return;
        if (!container || !container.isConnected || container.clientWidth === 0 || container.clientHeight === 0) return;
        const hostRealigned = alignTerminalHostToDevicePixels();
        let proposed: { cols: number; rows: number } | undefined;
        try {
          proposed = fit.proposeDimensions();
        } catch {
          return;
        }
        if (!proposed || !proposed.cols || !proposed.rows || disposed || !terminal) return;
        const { cols, rows } = proposed;

        // Skip firing a redundant resize RPC if the backend is already aligned on this exact geometry.
        if (confirmedGrid && confirmedGrid.cols === cols && confirmedGrid.rows === rows) {
          if (hostRealigned) {
            try {
              terminal.refresh(0, Math.max(0, terminal.rows - 1));
            } catch {
              // ignore
            }
          }
          reportMinimumHeight();
          return;
        }

        if (syncInFlight) {
          pendingSync = true;
          return;
        }
        syncInFlight = true;

        try {
          const args = { lane_id: props.laneId, window: props.window, cols, rows };
          if (props.shell) {
            // A GUI-owned shell has no other viewer, so our own resize is the authoritative one.
            await daemonCall("agent.resize", args).catch(() => undefined);
            if (!disposed) applyGrid(cols, rows);
          } else {
            const grid = await daemonCall("agent.fit", args).catch(() => null);
            if (disposed || !terminal) return;
            if (grid?.cols && grid?.rows) {
              applyGrid(grid.cols, grid.rows);
            }
          }
        } finally {
          syncInFlight = false;
          flushBufferedWrites();
          if (pendingSync && !disposed) {
            pendingSync = false;
            requestSyncSize(true);
          }
        }
      };

      const requestSyncSize = (immediate = false) => {
        if (disposed || !terminal || !container?.isConnected) return;
        if (visibilityFrame !== undefined) {
          cancelAnimationFrame(visibilityFrame);
          visibilityFrame = undefined;
        }
        if (syncTimer !== undefined) {
          clearTimeout(syncTimer);
          syncTimer = undefined;
        }

        if (immediate) {
          visibilityFrame = requestAnimationFrame(() => {
            visibilityFrame = undefined;
            if (disposed || !terminal || !container?.isConnected) return;
            void syncSize?.();
          });
        } else {
          syncTimer = setTimeout(() => {
            syncTimer = undefined;
            if (disposed || !terminal || !container?.isConnected) return;
            void syncSize?.();
          }, 60);
        }
      };

      // Accumulate fractional movement and issue at most one remote scroll at a time. New movement
      // is merged while the request is in flight, so long trackpad gestures cannot build a tail.
      let wheelAccum = 0;
      let wheelCell = { col: 1, row: 1 };

      const scheduleWheelFlush = () => {
        if (wheelFrame !== undefined || disposed) return;
        wheelFrame = requestAnimationFrame(flushWheel);
      };

      const flushWheel = () => {
        wheelFrame = undefined;
        if (!terminal || scrollRequestInFlight || disposed) return;
        const batch = takeWheelBatch(wheelAccum);
        if (batch.ticks === 0) return;
        wheelAccum = batch.remainder;
        const current = terminal;

        if (current.buffer.active.type === "normal") {
          current.scrollLines(batch.ticks);
          if (Math.abs(wheelAccum) >= 1) scheduleWheelFlush();
          return;
        }

        scrollRequestInFlight = true;
        void daemonCall("agent.scroll", {
          lane_id: props.laneId,
          window: props.window,
          up: batch.ticks < 0,
          ticks: Math.abs(batch.ticks),
          col: wheelCell.col,
          row: wheelCell.row,
        })
          .then((result) => {
            if (!result.forwarded && !disposed) current.scrollLines(batch.ticks);
          })
          .catch((error) => {
            if (!disposed) setTransportError(errorMessage(error));
          })
          .finally(() => {
            scrollRequestInFlight = false;
            if (Math.abs(wheelAccum) >= 1 && !disposed) scheduleWheelFlush();
          });
      };

      wheelListener = (event: WheelEvent) => {
        if (!terminal || disposed) return;
        event.preventDefault();
        event.stopPropagation();
        const screen = terminal.element?.querySelector<HTMLElement>(".xterm-screen");
        const screenRect = screen?.getBoundingClientRect();
        const screenHeight = screenRect?.height ?? 0;
        const pixelsPerLine = screenHeight > 0 && terminal.rows > 0
          ? screenHeight / terminal.rows
          : (terminal.options.fontSize ?? 12) * (terminal.options.lineHeight ?? 1);
        if (screenRect) {
          wheelCell = terminalPointerCell(
            event.clientX,
            event.clientY,
            screenRect.left,
            screenRect.top,
            screenRect.width,
            screenRect.height,
            terminal.cols,
            terminal.rows,
          );
        }
        wheelAccum += wheelLines(
          event.deltaY,
          event.deltaMode,
          terminal.rows,
          pixelsPerLine,
        );
        scheduleWheelFlush();
      };

      container.addEventListener("wheel", wheelListener, { capture: true, passive: false });
      resize = new ResizeObserver(() => {
        if (disposed || !container?.isConnected) return;
        requestSyncSize();
      });
      resize.observe(container);

      // ResizeObserver covers footprint changes that alter the box size, but not a pure position
      // change such as pane reordering. Watch the wrapper and grid attributes as well so every
      // layout/style transition re-snaps the persistent WebGL canvas to the physical pixel grid.
      if (typeof MutationObserver !== "undefined") {
        layoutMutation = new MutationObserver(() => requestSyncSize(true));
        const wrapper = pane.parentElement;
        if (wrapper) {
          layoutMutation.observe(wrapper, {
            attributes: true,
            attributeFilter: ["class", "style", "aria-hidden", "inert"],
          });
          if (wrapper.parentElement) {
            layoutMutation.observe(wrapper.parentElement, {
              attributes: true,
              attributeFilter: ["class", "style"],
            });
          }
        }
      }

      if (typeof IntersectionObserver !== "undefined") {
        intersection = new IntersectionObserver((entries) => {
          if (disposed || !container?.isConnected) return;
          for (const entry of entries) {
            if (entry.isIntersecting && entry.intersectionRatio > 0) {
              requestSyncSize();
            }
          }
        });
        if (container?.isConnected) {
          intersection.observe(container);
        }
      }

      onWindowResize = () => {
        if (disposed || !container?.isConnected) return;
        requestSyncSize();
      };
      window.addEventListener("resize", onWindowResize);
      unsubLayout = onLayoutChanged(() => {
        if (disposed || !container?.isConnected) return;
        requestSyncSize();
      });

      if (props.visible !== false) fit.fit();
      // Establish the pane's authoritative grid before the first checkpoint is painted. Painting
      // at one width and resizing afterward corrupts cursor-relative full-screen output.
      await syncSize?.();
      setReady(true);

      let currentFontFamily = initialApp.fontFamily;
      let currentFontSize = initialApp.fontSize;

      const applyAppearance = (app?: TerminalAppearance) => {
        if (!terminal || disposed || !container?.isConnected) return;
        const conf = app ?? readTerminalAppearance();
        const theme = terminalTheme(container, conf);
        setPaneBg(theme.background);
        terminal.options.theme = theme;

        const fontChanged = conf.fontFamily !== currentFontFamily || conf.fontSize !== currentFontSize;
        if (fontChanged) {
          currentFontFamily = conf.fontFamily;
          currentFontSize = conf.fontSize;
          terminal.options.fontFamily = terminalFontFamily(conf.fontFamily);
          terminal.options.fontSize = conf.fontSize;
          requestSyncSize();
        }
      };

      onAppearanceChanged = (e: Event) => {
        applyAppearance((e as CustomEvent<TerminalAppearance>).detail);
      };
      window.addEventListener("repomon:terminal-appearance-changed", onAppearanceChanged);

      // Retry transient boot failures, using a generation guard so stale attempts cannot replace or
      // orphan the current watch.
      const openWatch = async () => {
        const run = ++watchRun;
        const previousStop = stopWatch;
        stopWatch = undefined;
        if (previousStop) await previousStop().catch(() => undefined);
        if (disposed || run !== watchRun) return;

        const backoffMs = [0, 300, 900];
        for (let attempt = 0; attempt < backoffMs.length; attempt += 1) {
          if (attempt > 0) {
            await new Promise((resolve) => setTimeout(resolve, backoffMs[attempt]));
            if (disposed || run !== watchRun) return;
          }
          try {
            const watch = await watchTerminal(
              target,
              (bytes) => {
                writeIncoming(bytes);
              },
              (ack) => applyGrid(ack.cols, ack.rows),
              ({ cols, rows }) => applyGrid(cols, rows),
            );
            if (disposed || run !== watchRun) {
              await watch.stop();
              return;
            }
            stopWatch = async () => {
              if (onAppearanceChanged) {
                window.removeEventListener("repomon:terminal-appearance-changed", onAppearanceChanged);
                onAppearanceChanged = undefined;
              }
              await watch.stop();
            };
            setTransportError(null);
            if (props.focused && terminalShown() && terminal) terminal.focus();
            return;
          } catch (error) {
            if (disposed || run !== watchRun) return;
            if (attempt === backoffMs.length - 1) setTransportError(errorMessage(error));
          }
        }
      };

      retryWatch = () => {
        if (disposed || retrying()) return;
        setRetrying(true);
        void openWatch().finally(() => {
          if (!disposed) setRetrying(false);
        });
      };

      await openWatch();
    })().catch((error: unknown) => {
      if (!disposed) setTransportError(errorMessage(error));
    });
  });

  onCleanup(() => {
    disposed = true;
    watchRun += 1;
    retryWatch = undefined;
    rendererEpoch += 1;
    if (visibilityFrame !== undefined) {
      cancelAnimationFrame(visibilityFrame);
      visibilityFrame = undefined;
    }
    if (visibleSyncFrame !== undefined) {
      cancelAnimationFrame(visibleSyncFrame);
      visibleSyncFrame = undefined;
    }
    if (wheelFrame !== undefined) {
      cancelAnimationFrame(wheelFrame);
      wheelFrame = undefined;
    }
    resize?.disconnect();
    resize = undefined;
    layoutMutation?.disconnect();
    layoutMutation = undefined;
    intersection?.disconnect();
    intersection = undefined;
    if (onWindowResize) {
      window.removeEventListener("resize", onWindowResize);
      onWindowResize = undefined;
    }
    unsubLayout?.();
    unsubLayout = undefined;
    if (onAppearanceChanged) {
      window.removeEventListener("repomon:terminal-appearance-changed", onAppearanceChanged);
      onAppearanceChanged = undefined;
    }
    if (wheelListener && container) {
      container.removeEventListener("wheel", wheelListener, true);
      wheelListener = undefined;
    }
    if (resizeTimer) {
      clearTimeout(resizeTimer);
      resizeTimer = undefined;
    }
    if (syncTimer) {
      clearTimeout(syncTimer);
      syncTimer = undefined;
    }
    input?.dispose();
    input = undefined;
    void stopWatch?.();
    stopWatch = undefined;
    webgl?.dispose();
    webgl = undefined;
    fit = undefined;
    search = undefined;
    terminal?.dispose();
    terminal = undefined;
  });

  return (
    <section
      ref={pane}
      class="relative isolate h-full min-h-0 overflow-hidden bg-background"
      style={{ "background-color": paneBg() || undefined }}
      aria-label={props.label}
    >
      {/* Clip the canvas below the header and isolate pane stacking so oversized or promoted canvases
 * cannot paint over this or another pane’s controls. */}
      <div
        ref={container}
        class={`terminal-host absolute inset-x-2 bottom-0 top-10 overflow-hidden ${terminalShown() ? "" : "invisible pointer-events-none"} ${commandOpen() ? "terminal-command-host" : ""}`}
        style={{ "background-color": paneBg() || undefined }}
        aria-hidden={!terminalShown()}
      />
      <Show when={commandOpen()}>
        <div class="terminal-command-heading" role="region" aria-label="Agent command controls">
          <div><strong>{agentKind() === "claude-code" ? "Claude" : agentKind()}</strong><span>{command()}</span></div>
          <button class="focus-ring rounded" onClick={closeCommand}>Back to chat <IconClose size={12} /></button>
        </div>
      </Show>
      <Show when={!props.shell}>
        <div class={`absolute inset-0 z-[5] pt-10 ${view() === "conversation" && !commandOpen() ? "" : "hidden"}`}>
          {/* visible is pane-level: the transcript watch stays subscribed across a Terminal/Chat
             toggle, since the pane itself is still mounted and still the one on screen - only
             shown (chat is the displayed view right now) gates cosmetic, display-only work, so
             switching back to Chat repaints already-loaded rows instantly instead of re-paging. */}
          <ConversationPane target={{ lane_id: props.laneId, window: props.window, session_id: windowSessionId(), kind: agentKind() }}
            kind={agentKind()} lane={lane()} onFiles={props.onEnsureEditorOpen} onFocusAgent={(window) => props.workspace?.setActiveWindow(window)} detail={detail()} visible={props.visible !== false} shown={view() === "conversation" && !commandOpen()} onTerminal={() => setView("terminal")} onCommand={openCommand} />
        </div>
      </Show>
      <div class="pointer-events-none absolute inset-x-0 top-0 z-10 flex h-10 items-center justify-between border-b border-line bg-surface/95 px-2.5 font-mono text-[10px] uppercase tracking-wider text-muted backdrop-blur">
        <Show
          when={finding()}
          fallback={<span class="flex min-w-0 items-baseline gap-3 normal-case tracking-normal" title={`${taskTitle()}${lane() ? ` · ${lane()!.repo.label ?? lane()!.repo.name}` : ""}`}>
            <span class="truncate font-sans text-xs font-semibold text-foreground">{props.shell ? props.label : taskTitle()}</span>
            <Show when={lane() && !props.shell}><span class="max-w-32 truncate font-mono text-[10px] text-muted">{lane()!.repo.label ?? lane()!.repo.name}</span></Show>
          </span>}
        >
          <form
            class="pointer-events-auto flex min-w-0 flex-1 items-center gap-1.5"
            onSubmit={(event) => {
              event.preventDefault();
              find(true);
            }}
          >
            <label class="sr-only" for={`terminal-find-${props.laneId}-${props.window}`}>Find in terminal</label>
            <div class="relative flex min-w-28 flex-1 items-center">
              <input
                ref={searchInput}
                id={`terminal-find-${props.laneId}-${props.window}`}
                type="search"
                class="focus-ring h-5 w-full rounded border border-line bg-raised pl-5 pr-2 font-sans text-xs normal-case tracking-normal text-foreground placeholder:text-muted/60"
                value={query()}
                placeholder="Find in buffer…"
                onInput={(event) => {
                  setQuery(event.currentTarget.value);
                  find(true);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Escape") {
                    event.preventDefault();
                    setFinding(false);
                    terminal?.focus();
                  }
                }}
              />
              <span class="pointer-events-none absolute left-1.5 text-muted">
                <IconSearch size={11} />
              </span>
            </div>
            <button
              type="button"
              class="focus-ring flex size-5 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground"
              aria-label="Previous match"
              onClick={() => find(false)}
            >
              <IconArrowUp size={11} />
            </button>
            <button
              type="submit"
              class="focus-ring flex size-5 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground"
              aria-label="Next match"
            >
              <IconArrowDown size={11} />
            </button>
            <button
              type="button"
              class="focus-ring flex size-5 items-center justify-center rounded text-muted hover:bg-raised hover:text-foreground"
              aria-label="Close terminal search"
              onClick={() => {
                setFinding(false);
                terminal?.focus();
              }}
            >
              <IconClose size={12} />
            </button>
          </form>
        </Show>
        <div class="ml-2 flex shrink-0 items-center gap-2">
          <Show when={!props.shell && !finding()}>
            <button class="pointer-events-auto focus-ring text-[10px] normal-case tracking-normal hover:text-foreground" title="Use the default view from Settings > Agents" onClick={() => setView(null)}>Use default</button>
            <ViewToggle value={view()} onChange={setView} />
            <Show when={view() === "conversation"}><TranscriptDetailToggle value={detail()} onChange={setDetail} /></Show>
          </Show>
        </div>
      </div>
      <Show when={viewError() || (terminalShown() && transportError())}>
        <div
          role="alert"
          class="absolute inset-x-4 top-10 z-20 flex items-center justify-between gap-3 rounded-xl border border-fault/30 bg-surface p-3 text-xs text-fault shadow-lg"
        >
          <span>{viewError() ? `Could not save view: ${viewError()}` : `Terminal transport unavailable: ${transportError()}`}</span>
          <button
            type="button"
            class="focus-ring shrink-0 rounded-md border border-fault/40 px-2 py-1 font-medium text-fault transition-colors hover:bg-fault/10 disabled:opacity-50"
            disabled={!viewError() && retrying()}
            onClick={() => viewError() ? setViewError(null) : retryWatch?.()}
          >
            {viewError() ? "Dismiss" : retrying() ? "Retrying…" : "Retry"}
          </button>
        </div>
      </Show>
    </section>
  );
}
