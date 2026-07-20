export type Theme = "light" | "dark" | "system";

const storageKey = "repomon-theme";
const themes: Theme[] = ["system", "dark", "light"];

export function readTheme(): Theme {
  if (typeof window === "undefined") return "system";
  const saved = window.localStorage.getItem(storageKey);
  return themes.includes(saved as Theme) ? (saved as Theme) : "system";
}

export function applyTheme(theme: Theme): void {
  if (typeof window === "undefined") return;

  const root = window.document.documentElement;
  const systemDark = window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;
  const resolved = theme === "system" ? (systemDark ? "dark" : "light") : theme;

  root.classList.remove("light", "dark");
  root.classList.add(resolved);
  window.localStorage.setItem(storageKey, theme);
}

export function nextTheme(theme: Theme): Theme {
  return themes[(themes.indexOf(theme) + 1) % themes.length];
}

export function themeLabel(theme: Theme): string {
  return theme === "system" ? "System" : theme === "dark" ? "Dark" : "Light";
}
