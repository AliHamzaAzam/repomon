import { ClipboardAddon } from "@xterm/addon-clipboard";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import { Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";

import { daemonCall } from "../ipc/rpc";
import {
  createInputCoalescer,
  translateKeyboardKey,
  watchTerminal,
  wheelStep,
  type TerminalRenderer,
  type TerminalTarget,
} from "../ipc/term";

interface TerminalPaneProps extends TerminalTarget {
  label: string;
  renderer?: TerminalRenderer;
  focused?: boolean;
  /// A GUI-owned shell (no other viewer) — safe to force the pane to our size so it always fits.
  shell?: boolean;
}

function terminalTheme(element: HTMLElement) {
  const style = getComputedStyle(element);
  return {
    background: style.getPropertyValue("--background").trim(),
    foreground: style.getPropertyValue("--foreground").trim(),
    cursor: style.getPropertyValue("--signal").trim(),
    selectionBackground: `color-mix(in srgb, ${style.getPropertyValue("--signal").trim()} 24%, transparent)`,
    black: "#101418",
    red: "#e66b61",
    green: "#62c49a",
    yellow: "#e5b45d",
    blue: "#6ca4d9",
    magenta: "#c186d2",
    cyan: "#64c4bb",
    white: "#d9dfe5",
    brightBlack: "#66717c",
    brightRed: "#ff8177",
    brightGreen: "#7bdcaf",
    brightYellow: "#f4c66f",
    brightBlue: "#82b9ed",
    brightMagenta: "#d89be7",
    brightCyan: "#7bded4",
    brightWhite: "#f4f6f8",
  };
}

export default function TerminalPane(props: TerminalPaneProps) {
  let container!: HTMLDivElement;
  let searchInput!: HTMLInputElement;
  let terminal: Terminal | undefined;
  let search: SearchAddon | undefined;
  let webgl: WebglAddon | undefined;
  let input: ReturnType<typeof createInputCoalescer> | undefined;
  let resize: ResizeObserver | undefined;
  let resizeTimer: ReturnType<typeof setTimeout> | undefined;
  let wheelListener: ((event: WheelEvent) => void) | undefined;
  let stopWatch: (() => Promise<void>) | undefined;
  let rendererEpoch = 0;
  let disposed = false;
  let wheelInFlight = Promise.resolve();
  const [transportError, setTransportError] = createSignal<string | null>(null);
  const [renderer, setRenderer] = createSignal("DOM");
  const [ready, setReady] = createSignal(false);
  const [finding, setFinding] = createSignal(false);
  const [query, setQuery] = createSignal("");

  function errorMessage(error: unknown) {
    return error instanceof Error ? error.message : String(error);
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
    setRenderer("DOM");
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
      setRenderer("WEBGL");
      addon.onContextLoss(() => {
        if (webgl !== addon) return;
        addon.dispose();
        webgl = undefined;
        setRenderer("DOM");
      });
    } catch {
      webgl?.dispose();
      webgl = undefined;
      setRenderer("DOM");
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
    if (ready() && props.focused) terminal?.focus();
  });

  onMount(() => {
    void (async () => {
      const target = { laneId: props.laneId, window: props.window };
      if ((props.renderer ?? "dom") !== "dom") await preloadTerminalFont();
      if (disposed) return;

      input = createInputCoalescer(target);
      terminal = new Terminal({
        allowProposedApi: true,
        cursorBlink: true,
        cursorStyle: "bar",
        fontFamily: '"Berkeley Mono", "SFMono-Regular", "Cascadia Code", monospace',
        fontSize: 12,
        lineHeight: 1.18,
        scrollback: 10_000,
        theme: terminalTheme(container),
      });
      const fit = new FitAddon();
      search = new SearchAddon();
      terminal.loadAddon(fit);
      terminal.loadAddon(search);
      terminal.loadAddon(new ClipboardAddon());
      terminal.loadAddon(new Unicode11Addon());
      terminal.unicode.activeVersion = "11";
      terminal.open(container);

      terminal.attachCustomKeyEventHandler((event) => {
        if (event.type !== "keydown") return true;
        if ((event.metaKey || event.ctrlKey) && event.shiftKey && event.key.toLowerCase() === "f") {
          event.preventDefault();
          openFind();
          return false;
        }
        const translated = translateKeyboardKey(event);
        if (!translated) return true;
        event.preventDefault();
        void input?.key(translated).catch((error: unknown) => setTransportError(errorMessage(error)));
        return false;
      });
      terminal.onData((data) => input?.push(data));

      function applyGrid(cols?: number | null, rows?: number | null) {
        if (terminal && cols && rows && (cols !== terminal.cols || rows !== terminal.rows)) {
          terminal.resize(cols, rows);
        }
      }

      // Keep xterm and the backend pane on one authoritative grid. GUI-owned shells can be
      // resized directly. Shared agent panes use the arbitrated fit call so the TUI and desktop
      // never fight over dimensions.
      async function syncSize() {
        if (!terminal) return;
        try {
          fit.fit();
        } catch {
          return;
        }
        const cols = terminal.cols;
        const rows = terminal.rows;
        if (!cols || !rows) return;
        const args = { lane_id: props.laneId, window: props.window, cols, rows };
        if (props.shell) {
          await daemonCall("agent.resize", args).catch(() => undefined);
        } else {
          const grid = await daemonCall("agent.fit", args).catch(() => null);
          if (grid) applyGrid(grid.cols, grid.rows);
        }
      }

      wheelListener = (event: WheelEvent) => {
        if (!terminal) return;
        const step = wheelStep(event.deltaY, event.deltaMode, terminal.rows);
        if (!step) return;
        event.preventDefault();
        event.stopPropagation();
        const current = terminal;
        wheelInFlight = wheelInFlight.then(async () => {
          try {
            const result = await daemonCall("agent.scroll", {
              lane_id: props.laneId,
              window: props.window,
              up: step.up,
              ticks: step.ticks,
            });
            if (!result.forwarded) current.scrollLines(step.up ? -step.ticks : step.ticks);
          } catch (error) {
            current.scrollLines(step.up ? -step.ticks : step.ticks);
            setTransportError(errorMessage(error));
          }
        });
      };

      container.addEventListener("wheel", wheelListener, { capture: true, passive: false });
      resize = new ResizeObserver(() => {
        if (resizeTimer) clearTimeout(resizeTimer);
        resizeTimer = setTimeout(() => void syncSize(), 100);
      });
      resize.observe(container);
      fit.fit();
      setReady(true);

      try {
        const watch = await watchTerminal(target, (bytes) => terminal?.write(bytes));
        if (disposed) {
          await watch.stop();
          return;
        }
        stopWatch = watch.stop;
        setTransportError(null);
        void syncSize();
        if (props.focused) terminal.focus();
      } catch (error) {
        setTransportError(errorMessage(error));
      }
    })().catch((error: unknown) => setTransportError(errorMessage(error)));
  });

  onCleanup(() => {
    disposed = true;
    rendererEpoch += 1;
    resize?.disconnect();
    if (wheelListener) container.removeEventListener("wheel", wheelListener, true);
    if (resizeTimer) clearTimeout(resizeTimer);
    input?.dispose();
    void stopWatch?.();
    webgl?.dispose();
    terminal?.dispose();
  });

  return (
    <section class="relative h-full min-h-0 overflow-hidden bg-background" aria-label={props.label}>
      <div ref={container} class="terminal-host absolute inset-0 px-2 pb-2 pt-7" />
      <div class="pointer-events-none absolute inset-x-0 top-0 z-10 flex h-6 items-center justify-between border-b border-line bg-surface/90 px-2 font-mono text-[0.52rem] uppercase tracking-[0.08em] text-muted backdrop-blur">
        <Show
          when={finding()}
          fallback={<span class="truncate">{props.label}</span>}
        >
          <form
            class="pointer-events-auto flex min-w-0 flex-1 items-center gap-1"
            onSubmit={(event) => {
              event.preventDefault();
              find(true);
            }}
          >
            <label class="sr-only" for={`terminal-find-${props.laneId}-${props.window}`}>Find in terminal</label>
            <input
              ref={searchInput}
              id={`terminal-find-${props.laneId}-${props.window}`}
              type="search"
              class="focus-ring h-5 min-w-20 flex-1 rounded border border-line bg-raised px-1.5 text-[0.58rem] normal-case tracking-normal text-foreground"
              value={query()}
              placeholder="Find in terminal"
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
            <button type="button" class="focus-ring rounded px-1 text-muted hover:text-foreground" aria-label="Previous match" onClick={() => find(false)}>↑</button>
            <button type="submit" class="focus-ring rounded px-1 text-muted hover:text-foreground" aria-label="Next match">↓</button>
            <button type="button" class="focus-ring rounded px-1 text-muted hover:text-foreground" aria-label="Close terminal search" onClick={() => {
              setFinding(false);
              terminal?.focus();
            }}>×</button>
          </form>
        </Show>
        <span class="ml-2 shrink-0">{renderer()}</span>
      </div>
      <Show when={transportError()}>
        <div class="absolute inset-x-4 top-10 z-20 rounded-md border border-fault/40 bg-surface p-2 text-xs text-fault shadow-lg">
          Terminal transport unavailable: {transportError()}
        </div>
      </Show>
    </section>
  );
}
