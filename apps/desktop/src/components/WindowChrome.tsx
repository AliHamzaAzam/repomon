import { Show, createSignal, onCleanup, onMount, type JSX } from "solid-js";

import { isMac, isWindows } from "../keymap";

/// Reserves space for overlaid macOS traffic lights while using symmetric padding elsewhere.
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

/// Renders the shared draggable title bar with platform insets and Windows caption controls,
/// keeping interactive children outside the drag region.
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

/// Render theme-aware Windows caption controls with full-height hit targets.
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
