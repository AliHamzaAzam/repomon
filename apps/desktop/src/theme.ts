export type Theme = "system" | "dark" | "midnight" | "nord" | "dracula" | "sepia" | "light";

const storageKey = "repomon-theme";
export const THEMES: Theme[] = ["system", "dark", "midnight", "nord", "dracula", "sepia", "light"];

export interface ThemePreset {
  id: Theme;
  name: string;
  description: string;
  isDark: boolean;
  preview: {
    bg: string;
    surface: string;
    line: string;
    signal: string;
  };
}

export const THEME_PRESETS: ThemePreset[] = [
  {
    id: "system",
    name: "System",
    description: "Follows your operating system color scheme",
    isDark: true,
    preview: {
      bg: "hsl(224 20% 7%)",
      surface: "hsl(224 18% 11%)",
      line: "hsl(224 12% 21%)",
      signal: "hsl(168 62% 54%)",
    },
  },
  {
    id: "dark",
    name: "Dark Default",
    description: "Balanced slate dark theme with soft contrast",
    isDark: true,
    preview: {
      bg: "hsl(224 20% 7%)",
      surface: "hsl(224 18% 11%)",
      line: "hsl(224 12% 21%)",
      signal: "hsl(168 62% 54%)",
    },
  },
  {
    id: "midnight",
    name: "Midnight OLED",
    description: "True pitch-black background with crisp borders",
    isDark: true,
    preview: {
      bg: "hsl(0 0% 0%)",
      surface: "hsl(0 0% 5%)",
      line: "hsl(0 0% 18%)",
      signal: "hsl(168 70% 50%)",
    },
  },
  {
    id: "nord",
    name: "Nord Arctic",
    description: "Cool arctic palette inspired by arctic ice and fog",
    isDark: true,
    preview: {
      bg: "hsl(220 16% 14%)",
      surface: "hsl(222 16% 19%)",
      line: "hsl(220 16% 30%)",
      signal: "hsl(179 25% 65%)",
    },
  },
  {
    id: "dracula",
    name: "Dracula",
    description: "Dark gothic theme with vibrant neon highlights",
    isDark: true,
    preview: {
      bg: "hsl(231 15% 14%)",
      surface: "hsl(232 14% 19%)",
      line: "hsl(232 14% 34%)",
      signal: "hsl(135 94% 65%)",
    },
  },
  {
    id: "sepia",
    name: "Warm Paper",
    description: "Soft warm-toned palette easy on the eyes for reading",
    isDark: false,
    preview: {
      bg: "hsl(36 30% 93%)",
      surface: "hsl(36 33% 97%)",
      line: "hsl(36 18% 78%)",
      signal: "hsl(168 50% 32%)",
    },
  },
  {
    id: "light",
    name: "Modern Light",
    description: "Clean, high-clarity daylight interface",
    isDark: false,
    preview: {
      bg: "hsl(220 18% 97%)",
      surface: "hsl(0 0% 100%)",
      line: "hsl(220 13% 86%)",
      signal: "hsl(168 60% 36%)",
    },
  },
];

export const ACCENTS: Record<string, string> = {
  cyan: "hsl(169 61% 49%)",
  green: "hsl(145 56% 45%)",
  magenta: "hsl(300 55% 52%)",
  amber: "hsl(35 86% 52%)",
  blue: "hsl(207 68% 52%)",
  red: "hsl(5 68% 52%)",
  violet: "hsl(265 62% 58%)",
};

export const ACCENT_SWATCHES = [
  { id: "cyan", label: "Cyan", color: "hsl(169 61% 49%)" },
  { id: "green", label: "Emerald", color: "hsl(145 56% 45%)" },
  { id: "blue", label: "Blue", color: "hsl(207 68% 52%)" },
  { id: "violet", label: "Violet", color: "hsl(265 62% 58%)" },
  { id: "magenta", label: "Magenta", color: "hsl(300 55% 52%)" },
  { id: "amber", label: "Amber", color: "hsl(35 86% 52%)" },
  { id: "red", label: "Crimson", color: "hsl(5 68% 52%)" },
  { id: "mono", label: "Monochrome", color: "hsl(220 9% 46%)" },
];

export function readTheme(): Theme {
  if (typeof window === "undefined") return "system";
  const saved = window.localStorage.getItem(storageKey);
  return THEMES.includes(saved as Theme) ? (saved as Theme) : "system";
}

const ALL_THEME_CLASSES = [
  "light",
  "dark",
  "theme-midnight",
  "theme-nord",
  "theme-dracula",
  "theme-sepia",
  "theme-light",
  "theme-dark",
];

export function applyTheme(theme: Theme): void {
  if (typeof window === "undefined") return;

  const root = window.document.documentElement;
  const systemDark = window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;

  root.classList.remove(...ALL_THEME_CLASSES);

  switch (theme) {
    case "system":
      root.classList.add(systemDark ? "dark" : "light");
      break;
    case "dark":
      root.classList.add("dark", "theme-dark");
      break;
    case "midnight":
      root.classList.add("dark", "theme-midnight");
      break;
    case "nord":
      root.classList.add("dark", "theme-nord");
      break;
    case "dracula":
      root.classList.add("dark", "theme-dracula");
      break;
    case "sepia":
      root.classList.add("light", "theme-sepia");
      break;
    case "light":
      root.classList.add("light", "theme-light");
      break;
    default:
      root.classList.add("dark");
      break;
  }

  window.localStorage.setItem(storageKey, theme);
}

export function nextTheme(theme: Theme): Theme {
  return THEMES[(THEMES.indexOf(theme) + 1) % THEMES.length];
}

export function themeLabel(theme: Theme): string {
  const found = THEME_PRESETS.find((p) => p.id === theme);
  return found?.name ?? theme;
}

export function applyAccent(accent?: string | null): void {
  if (typeof document === "undefined") return;
  const value = accent?.trim().toLowerCase();
  const color = value && /^#[0-9a-f]{3}([0-9a-f]{3})?$/i.test(value)
    ? value
    : value === "mono" || value === "none" || value === "off"
      ? "var(--muted)"
      : ACCENTS[value ?? "cyan"] ?? ACCENTS.cyan;
  document.documentElement.style.setProperty("--signal", color);
}
