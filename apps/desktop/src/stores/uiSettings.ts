/**
 * Desktop UI preferences stored locally in localStorage.
 */

export const AUTO_COLLAPSE_STORAGE_KEY = "repomon:auto-collapse-empty-lanes";
const AUTO_COLLAPSE_EVENT = "repomon:auto-collapse-changed";

/**
 * Reads the auto-collapse setting from localStorage. Defaults to true (enabled).
 */
export function readAutoCollapseEmptyLanes(): boolean {
  if (typeof window === "undefined" || typeof localStorage === "undefined") {
    return true;
  }
  const raw = localStorage.getItem(AUTO_COLLAPSE_STORAGE_KEY);
  if (raw === null) return true;
  return raw === "true";
}

/**
 * Saves the auto-collapse setting to localStorage and dispatches a window event so
 * active views (like FleetSidebar) react immediately.
 */
export function saveAutoCollapseEmptyLanes(enabled: boolean): void {
  if (typeof window === "undefined" || typeof localStorage === "undefined") {
    return;
  }
  localStorage.setItem(AUTO_COLLAPSE_STORAGE_KEY, String(enabled));
  window.dispatchEvent(new CustomEvent(AUTO_COLLAPSE_EVENT, { detail: enabled }));
}

/**
 * Subscribes to changes of the auto-collapse setting. Returns an unsubscribe function.
 */
export function onAutoCollapseChanged(callback: (enabled: boolean) => void): () => void {
  if (typeof window === "undefined") return () => {};
  const handler = (e: Event) => {
    const custom = e as CustomEvent<boolean>;
    callback(typeof custom.detail === "boolean" ? custom.detail : readAutoCollapseEmptyLanes());
  };
  window.addEventListener(AUTO_COLLAPSE_EVENT, handler);
  return () => window.removeEventListener(AUTO_COLLAPSE_EVENT, handler);
}

export const LAYOUT_CHANGED_EVENT = "repomon:layout-changed";

/**
 * Dispatches a global layout change event so embedded viewports (such as xterm terminal panes)
 * immediately recalculate their geometry and repaint without waiting for window resize.
 */
export function notifyLayoutChanged(): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent(LAYOUT_CHANGED_EVENT));
}

/**
 * Subscribes to layout changes across the app. Returns an unsubscribe function.
 */
export function onLayoutChanged(callback: () => void): () => void {
  if (typeof window === "undefined") return () => {};
  window.addEventListener(LAYOUT_CHANGED_EVENT, callback);
  return () => window.removeEventListener(LAYOUT_CHANGED_EVENT, callback);
}

export const ONBOARDING_COMPLETED_KEY = "repomon:onboarding-completed";
const ONBOARDING_COMPLETED_EVENT = "repomon:onboarding-completed-changed";

/**
 * Reads whether the user has completed or skipped the first-run onboarding wizard.
 */
export function readOnboardingCompleted(): boolean {
  if (typeof window === "undefined" || typeof localStorage === "undefined") {
    return false;
  }
  return localStorage.getItem(ONBOARDING_COMPLETED_KEY) === "true";
}

/**
 * Saves whether onboarding has been completed or skipped to localStorage and notifies listeners.
 */
export function saveOnboardingCompleted(completed: boolean): void {
  if (typeof window === "undefined" || typeof localStorage === "undefined") {
    return;
  }
  localStorage.setItem(ONBOARDING_COMPLETED_KEY, String(completed));
  window.dispatchEvent(new CustomEvent(ONBOARDING_COMPLETED_EVENT, { detail: completed }));
}

/**
 * Subscribes to onboarding completed status changes. Returns an unsubscribe function.
 */
export function onOnboardingCompletedChanged(callback: (completed: boolean) => void): () => void {
  if (typeof window === "undefined") return () => {};
  const handler = (e: Event) => {
    const custom = e as CustomEvent<boolean>;
    callback(typeof custom.detail === "boolean" ? custom.detail : readOnboardingCompleted());
  };
  window.addEventListener(ONBOARDING_COMPLETED_EVENT, handler);
  return () => window.removeEventListener(ONBOARDING_COMPLETED_EVENT, handler);
}

