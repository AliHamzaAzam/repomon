import { ClipboardAddon } from "@xterm/addon-clipboard";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import { Show, createSignal, onCleanup, onMount } from "solid-js";

import { daemonCall } from "../ipc/rpc";
import {
  createInputCoalescer,
  translateKeyboardKey,
  watchTerminal,
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
  let stopWatch: (() => Promise<void>) | undefined;
  const [transportError, setTransportError] = createSignal<string | null>(null);
  const [renderer, setRenderer] = createSignal("DOM");

  onMount(() => {
    const target = { laneId: props.laneId, window: props.window };
    const input = createInputCoalescer(target);
    const terminal = new Terminal({
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
    const search = new SearchAddon();
    terminal.loadAddon(fit);
    terminal.loadAddon(search);
    terminal.loadAddon(new ClipboardAddon());
    terminal.loadAddon(new Unicode11Addon());
    terminal.unicode.activeVersion = "11";
    terminal.open(container);

    let webgl: WebglAddon | undefined;
    if ((props.renderer ?? "auto") !== "dom") {
      try {
        webgl = new WebglAddon();
        terminal.loadAddon(webgl);
        setRenderer("WEBGL");
        webgl.onContextLoss(() => {
          webgl?.dispose();
          webgl = undefined;
          setRenderer("DOM");
        });
      } catch {
        webgl?.dispose();
        webgl = undefined;
        setRenderer("DOM");
      }
    }

    terminal.attachCustomKeyEventHandler((event) => {
      if (event.type !== "keydown") return true;
      if ((event.metaKey || event.ctrlKey) && event.shiftKey && event.key.toLowerCase() === "f") {
        event.preventDefault();
        const query = window.prompt("Find in terminal");
        if (query) search.findNext(query, { incremental: true });
        return false;
      }
      const translated = translateKeyboardKey(event);
      if (!translated) return true;
      event.preventDefault();
      void input.key(translated);
      return false;
    });
    terminal.onData((data) => input.push(data));

    function applyGrid(cols?: number | null, rows?: number | null) {
      if (cols && rows && (cols !== terminal.cols || rows !== terminal.rows)) {
        terminal.resize(cols, rows);
      }
    }

    // Keep xterm and the tmux pane the same size. Fit xterm to the container first (so it never
    // overflows and clips the bottom), then reconcile the pane:
    //  - A GUI-owned shell: force the pane to our size with agent.resize, so pane == container.
    //  - A shared agent pane: request politely with the arbitrated agent.fit; if another viewer
    //    (e.g. the TUI) owns the size, pin xterm to the authoritative grid it returns so the raw
    //    byte stream's absolute cursor moves don't land at the wrong columns and garble the pane.
    async function syncSize() {
      try {
        fit.fit();
      } catch {
        return; // a hidden grid tile can briefly have no measurable geometry
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

    let resizeTimer: ReturnType<typeof setTimeout> | undefined;
    const resize = new ResizeObserver(() => {
      if (resizeTimer) clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => void syncSize(), 100);
    });
    resize.observe(container);
    fit.fit();

    void watchTerminal(target, (bytes) => terminal.write(bytes))
      .then((watch) => {
        stopWatch = watch.stop;
        setTransportError(null);
        void syncSize();
        terminal.focus();
      })
      .catch((error: unknown) => {
        setTransportError(error instanceof Error ? error.message : String(error));
      });

    onCleanup(() => {
      resize.disconnect();
      if (resizeTimer) clearTimeout(resizeTimer);
      input.dispose();
      void stopWatch?.();
      webgl?.dispose();
      terminal.dispose();
    });
  });

  return (
    <section class="relative h-full min-h-0 overflow-hidden bg-background" aria-label={props.label}>
      <div ref={container} class="terminal-host absolute inset-0 px-2 pb-2 pt-7" />
      <div class="pointer-events-none absolute inset-x-0 top-0 z-10 flex h-6 items-center justify-between border-b border-line bg-surface/90 px-2 font-mono text-[0.52rem] uppercase tracking-[0.08em] text-muted backdrop-blur">
        <span class="truncate">{props.label}</span>
        <span>{renderer()}</span>
      </div>
      <Show when={transportError()}>
        <div class="absolute inset-x-4 top-10 z-20 rounded-md border border-fault/40 bg-surface p-2 text-xs text-fault shadow-lg">
          Terminal transport unavailable: {transportError()}
        </div>
      </Show>
    </section>
  );
}
