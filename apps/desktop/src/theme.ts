export type Theme = "light" | "dark" | "system";

const storageKey = "repomon-theme";
const themes: Theme[] = ["system", "dark", "light"];
const accents: Record<string, string> = {
  cyan: "hsl(169 61% 49%)",
  green: "hsl(145 56% 45%)",
  magenta: "hsl(300 55% 52%)",
  amber: "hsl(35 86% 52%)",
  blue: "hsl(207 68% 52%)",
  red: "hsl(5 68% 52%)",
  violet: "hsl(265 62% 58%)",
};

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

export function applyAccent(accent?: string | null): void {
  if (typeof document === "undefined") return;
  const value = accent?.trim().toLowerCase();
  const color = value && /^#[0-9a-f]{3}([0-9a-f]{3})?$/i.test(value)
    ? value
    : value === "mono" || value === "none" || value === "off"
      ? "var(--muted)"
      : accents[value ?? "cyan"] ?? accents.cyan;
  document.documentElement.style.setProperty("--signal", color);
}
