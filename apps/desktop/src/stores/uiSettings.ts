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

/** Persists auto-collapse and broadcasts it to active views. */
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

/** Sidebar cost visibility uses the same preference/event store as auto-collapse. */
export const SIDEBAR_COST_STORAGE_KEY = "repomon:sidebar-show-today-cost";
const SIDEBAR_COST_EVENT = "repomon:sidebar-show-today-cost-changed";

export function readSidebarShowTodayCost(): boolean {
  try { return localStorage.getItem(SIDEBAR_COST_STORAGE_KEY) !== "false"; }
  catch { return true; }
}

export function saveSidebarShowTodayCost(enabled: boolean): void {
  try { localStorage.setItem(SIDEBAR_COST_STORAGE_KEY, String(enabled)); } catch {}
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent(SIDEBAR_COST_EVENT, { detail: enabled }));
  }
}

export function onSidebarShowTodayCostChanged(callback: (enabled: boolean) => void): () => void {
  if (typeof window === "undefined") return () => {};
  const handler = (event: Event) => {
    const value = (event as CustomEvent<unknown>).detail;
    callback(typeof value === "boolean" ? value : readSidebarShowTodayCost());
  };
  window.addEventListener(SIDEBAR_COST_EVENT, handler);
  return () => window.removeEventListener(SIDEBAR_COST_EVENT, handler);
}

export const LAYOUT_CHANGED_EVENT = "repomon:layout-changed";

/** Requests embedded viewport refitting without waiting for a window resize. */
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

export const SHORTCUTS_HINT_LAUNCH_COUNT_KEY = "repomon:shortcuts-hint-launches";
/// The footer's "Cmd-/ for shortcuts" hint only earns its place for someone who has not
/// discovered the guide yet - past this many launches it has done its job, and stays quiet.
export const SHORTCUTS_HINT_MAX_LAUNCHES = 3;

/**
 * How many app launches have shown the shortcuts hint so far.
 */
export function readShortcutsHintLaunchCount(): number {
  if (typeof window === "undefined" || typeof localStorage === "undefined") {
    return 0;
  }
  const raw = localStorage.getItem(SHORTCUTS_HINT_LAUNCH_COUNT_KEY);
  const parsed = raw === null ? 0 : Number.parseInt(raw, 10);
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : 0;
}

/** Records one hint display per launch, never per render. */
export function recordShortcutsHintLaunch(): void {
  if (typeof window === "undefined" || typeof localStorage === "undefined") {
    return;
  }
  try {
    localStorage.setItem(SHORTCUTS_HINT_LAUNCH_COUNT_KEY, String(readShortcutsHintLaunchCount() + 1));
  } catch {
    // localStorage can throw (quota, private mode) — persistence is best-effort.
  }
}

export const RIGHT_PANEL_ACTIVE_TAB_KEY = "repomon:right-panel-active-tab";

/** Reads the saved panel tab or returns null for registry fallback. */
export function readRightPanelActiveTab(): string | null {
  if (typeof window === "undefined" || typeof localStorage === "undefined") {
    return null;
  }
  return localStorage.getItem(RIGHT_PANEL_ACTIVE_TAB_KEY);
}

/**
 * Persists which right-rail tab is active, so switching Repomind/Git/Editor survives a reload.
 */
export function saveRightPanelActiveTab(id: string): void {
  if (typeof window === "undefined" || typeof localStorage === "undefined") {
    return;
  }
  try {
    localStorage.setItem(RIGHT_PANEL_ACTIVE_TAB_KEY, id);
  } catch {
    // localStorage can throw (quota, private mode) — persistence is best-effort.
  }
}

