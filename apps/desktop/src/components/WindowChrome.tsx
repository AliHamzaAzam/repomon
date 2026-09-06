import { Show, createSignal, onCleanup, onMount, type JSX } from "solid-js";

import { isMac, isWindows } from "../keymap";

/// Padding that keeps a full-window header clear of the macOS traffic lights.
///
/// The window is configured with `titleBarStyle: "Overlay"` and `hiddenTitle: true`
/// (`src-tauri/tauri.conf.json`), so macOS paints close/minimize/zoom *over* the top-left of the
/// web view and the app owns every pixel underneath. Anything drawn at the leading edge without
/// this inset lands under those three buttons. On every other platform the system draws no such
/// overlay, so the header takes ordinary symmetric padding instead.
///
/// The `platform` argument exists for tests; at runtime it is resolved from `navigator`.
export function windowChromeInsetClass(platform?: string): string {
  return isMac(platform) ? "pl-[78px]" : "px-3.5";
}

/// The three window operations the Windows caption controls need, plus the two reads that keep
/// the maximize glyph honest. Injected in tests; the app resolves them from the Tauri window API
/// the first time a control is drawn, so no test and no non-Windows session ever touches it.
export interface WindowControlsApi {
  minimize(): Promise<void>;
  toggleMaximize(): Promise<void>;
  close(): Promise<void>;
  isMaximized(): Promise<boolean>;
  /// Subscribe to size changes; resolves to the unsubscribe function.
  onResized(handler: () => void): Promise<() => void>;
}

async function tauriWindowControls(): Promise<WindowControlsApi> {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const current = getCurrentWindow();
  return {
    minimize: () => current.minimize(),
    toggleMaximize: () => current.toggleMaximize(),
    close: () => current.close(),
    isMaximized: () => current.isMaximized(),
    onResized: (handler) => current.onResized(() => handler()),
  };
}

export interface WindowChromeHeaderProps {
  /// Extra classes appended after the shared chrome classes.
  class?: string;
  /// "mac", "windows" or "linux"; tests inject it, the app resolves it from `navigator`.
  platform?: string;
  /// Tests inject a fake; the app talks to the Tauri window.
  windowControls?: WindowControlsApi;
  children: JSX.Element;
}

/// The app's title bar, shared by every screen that owns the whole window.
///
/// There is exactly one of these on screen at a time: the mission control shell draws it, and the
/// setup wizard draws it instead while it is up. Both need the same things, which is why this is
/// a component rather than a copied class string: `data-tauri-drag-region` so the frameless
/// window can still be dragged (and, on Windows and Linux, maximized by a double-click) from its
/// title bar via Tauri's second-mousedown drag-region listener,
/// [`windowChromeInsetClass`] so the leading content clears the macOS traffic lights,
/// and on Windows the caption controls the system no longer draws, since the Windows builds turn
/// native decorations off (`tauri.*.win.conf.json`) to get one bar instead of two.
///
/// The first child takes the leading edge; everything after it gathers at the trailing edge, in
/// front of the caption controls. The drag attribute sits on the header only: a button that
/// carried it would start a window drag instead of clicking.
export default function WindowChromeHeader(props: WindowChromeHeaderProps) {
  return (
    <header
      data-tauri-drag-region
      data-window-chrome
      class={`flex h-[35px] shrink-0 items-center gap-2 border-b border-line bg-surface/95 pr-1.5 backdrop-blur select-none [&>*:first-child]:mr-auto ${windowChromeInsetClass(props.platform)} ${props.class ?? ""}`}
    >
      {props.children}
      <Show when={isWindows(props.platform)}>
        <WindowCaptionControls controls={props.windowControls} />
      </Show>
    </header>
  );
}

/// Minimize, maximize-or-restore, and close, in the Windows caption grammar: 46px hit targets
/// the full height of the bar, 10px hairline glyphs, a flat hover, and a close that turns the
/// fault red. Drawn as SVG in the current color so every theme recolors them with the rest of
/// the chrome.
function WindowCaptionControls(props: { controls?: WindowControlsApi }) {
  const [maximized, setMaximized] = createSignal(false);
  let api: Promise<WindowControlsApi> | undefined;
  const controls = () => (api ??= props.controls ? Promise.resolve(props.controls) : tauriWindowControls());

  onMount(() => {
    let unlisten: (() => void) | undefined;
    let live = true;
    void controls().then(async (c) => {
      const refresh = () => void c.isMaximized().then((value) => live && setMaximized(value)).catch(() => undefined);
      refresh();
      const stop = await c.onResized(refresh);
      if (live) unlisten = stop;
      else stop();
    }).catch(() => undefined);
    onCleanup(() => {
      live = false;
      unlisten?.();
    });
  });

  const run = (action: (c: WindowControlsApi) => Promise<void>) => {
    void controls().then(action).catch(() => undefined);
  };

  const button =
    "flex h-full w-[46px] items-center justify-center text-foreground/80 transition-colors";

  return (
    <div class="-mr-1.5 flex h-full shrink-0 items-stretch self-stretch" role="group" aria-label="Window">
      <button type="button" class={`${button} hover:bg-raised hover:text-foreground`} onClick={() => run((c) => c.minimize())} aria-label="Minimize" title="Minimize">
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="1" aria-hidden="true">
          <path d="M0 5.5h10" />
        </svg>
      </button>
      <button
        type="button"
        class={`${button} hover:bg-raised hover:text-foreground`}
        onClick={() => run((c) => c.toggleMaximize())}
        aria-label={maximized() ? "Restore" : "Maximize"}
        title={maximized() ? "Restore down" : "Maximize"}
      >
        <Show
          when={maximized()}
          fallback={
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="1" aria-hidden="true">
              <rect x="0.5" y="0.5" width="9" height="9" />
            </svg>
          }
        >
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="1" aria-hidden="true">
            <path d="M2.5 2.5v-2h7v7h-2" />
            <rect x="0.5" y="2.5" width="7" height="7" />
          </svg>
        </Show>
      </button>
      <button
        type="button"
        class={`${button} hover:bg-fault hover:text-background`}
        onClick={() => run((c) => c.close())}
        aria-label="Close"
        title="Close"
      >
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="1.1" aria-hidden="true">
          <path d="M0.5 0.5l9 9M9.5 0.5l-9 9" />
        </svg>
      </button>
    </div>
  );
}
