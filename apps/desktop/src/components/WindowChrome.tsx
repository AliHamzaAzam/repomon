import type { JSX } from "solid-js";

import { isMac } from "../keymap";

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

export interface WindowChromeHeaderProps {
  /// Extra classes appended after the shared chrome classes.
  class?: string;
  children: JSX.Element;
}

/// The app's title bar, shared by every screen that owns the whole window.
///
/// There is exactly one of these on screen at a time: the mission control shell draws it, and the
/// setup wizard draws it instead while it is up. Both need the same two things, which is why this
/// is a component rather than a copied class string: `data-tauri-drag-region` so the frameless
/// window can still be dragged by its title bar, and [`windowChromeInsetClass`] so the leading
/// content clears the macOS traffic lights.
///
/// Children are laid out with `justify-between`: leading content first, trailing content last.
export default function WindowChromeHeader(props: WindowChromeHeaderProps) {
  return (
    <header
      data-tauri-drag-region
      data-window-chrome
      class={`flex h-[35px] shrink-0 items-center justify-between border-b border-line bg-surface/95 pr-1.5 backdrop-blur select-none ${windowChromeInsetClass()} ${props.class ?? ""}`}
    >
      {props.children}
    </header>
  );
}
