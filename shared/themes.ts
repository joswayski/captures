export const COLOR_THEMES = [
  {
    id: "mustard",
    name: "Mustard",
    description: "Captures mustard and signal red",
  },
  {
    id: "saffron",
    name: "Saffron",
    description: "Softer gold and coral",
  },
  {
    id: "cobalt",
    name: "Cobalt",
    description: "Technical blue and coral",
  },
  {
    id: "mint",
    name: "Mint",
    description: "Fresh mint and vermilion",
  },
] as const;

export type ColorTheme = (typeof COLOR_THEMES)[number]["id"];

export const DEFAULT_COLOR_THEME: ColorTheme = "mustard";
export const COLOR_THEME_STORAGE_KEY = "captures-color-theme";

export function isColorTheme(value: unknown): value is ColorTheme {
  return COLOR_THEMES.some((theme) => theme.id === value);
}

export function readStoredColorTheme(): ColorTheme {
  try {
    const stored = window.localStorage.getItem(COLOR_THEME_STORAGE_KEY);
    return isColorTheme(stored) ? stored : DEFAULT_COLOR_THEME;
  } catch {
    return DEFAULT_COLOR_THEME;
  }
}

export function applyColorTheme(theme: ColorTheme): void {
  document.documentElement.dataset.captureTheme = theme;
  try {
    window.localStorage.setItem(COLOR_THEME_STORAGE_KEY, theme);
  } catch {
    // A persisted app setting remains the source of truth when storage is unavailable.
  }
}
