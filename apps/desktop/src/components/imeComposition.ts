/**
 * Enter that CONFIRMS an IME candidate must never reach an Enter handler.
 *
 * `event.isComposing` alone is not enough. **Safari fires `compositionend` BEFORE the confirming
 * Enter's `keydown`**, so the flag is already `false` when the handler runs and the naive guard
 * lets the keypress through — the message sends half-composed. Chrome and Firefox fire it after,
 * which is why those two look correct without any of this and the bug reads as Safari-only.
 *
 * That makes it the DEFAULT case here rather than an edge one: Tauri on macOS runs WKWebView.
 * Measured on a maintainer's machine, `compositionend` arrived **3 ms** before the `keydown`.
 *
 * A tight window after `compositionend` covers it. That sequence is synchronous — microseconds —
 * while a human pressing Enter a second time takes 100 ms or more and never lands inside it.
 *
 * Composition is tracked on `window` in the capture phase rather than per field, so a composition
 * that starts in one input and ends after focus moves still closes, and a handler bound to
 * something that is not a text field can call this safely: no composition ever opens there, so it
 * always answers `false`.
 */

export const SAFARI_IME_RACE_WINDOW_MS = 30;

let composing = false;
// Not 0: `performance.now()` counts from the page's time origin, so `now - 0` sits inside
// the window for the page's first 30ms and would swallow an Enter no composition preceded.
let lastCompositionEndAt = Number.NEGATIVE_INFINITY;

/** Injectable for tests; `performance.now()` everywhere else. */
let now: () => number = () => performance.now();

function onCompositionStart(): void {
  composing = true;
}

function onCompositionEnd(): void {
  composing = false;
  lastCompositionEndAt = now();
}

// `blur` does not bubble, but a capture-phase listener on `window` still sees an element's blur on
// the way down — otherwise a composition abandoned by clicking away would stay open forever.
function onBlur(): void {
  composing = false;
}

if (typeof window !== "undefined") {
  window.addEventListener("compositionstart", onCompositionStart, true);
  window.addEventListener("compositionend", onCompositionEnd, true);
  window.addEventListener("blur", onBlur, true);
}

/**
 * True when this keydown is (or is very likely) an IME confirmation rather than a deliberate
 * keypress. Guard every Enter handler with it — including ones on buttons and rows, where it
 * costs nothing and removes the need to decide which elements can hold a composition.
 */
export function isImeConfirmation(event: Pick<KeyboardEvent, "isComposing">): boolean {
  if (event.isComposing || composing) return true;
  return now() - lastCompositionEndAt < SAFARI_IME_RACE_WINDOW_MS;
}

/** Test seam: drive the clock and the listeners without a real IME. */
export const __ime = {
  setNow(fn: () => number) {
    now = fn;
  },
  reset() {
    composing = false;
    lastCompositionEndAt = Number.NEGATIVE_INFINITY;
    now = () => performance.now();
  },
  start: onCompositionStart,
  end: onCompositionEnd,
  blur: onBlur,
};
