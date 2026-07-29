export const COLOR_THEMES = [
  {
    id: "mustard",
    name: "Mustard",
    description: "Captures mustard and signal red",
  },
  {
    id: "violet",
    name: "Violet",
    description: "Electric violet and hot coral",
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
  {
    id: "custom",
    name: "Custom",
    description: "Choose your own colors",
  },
] as const;

export type ColorTheme = (typeof COLOR_THEMES)[number]["id"];

export interface CustomThemeColors {
  accent: string;
  signal: string;
}

export const DEFAULT_COLOR_THEME: ColorTheme = "mustard";
export const DEFAULT_CUSTOM_THEME: CustomThemeColors = {
  accent: "#ffca28",
  signal: "#ef4650",
};
export const COLOR_THEME_STORAGE_KEY = "captures-color-theme";
export const CUSTOM_THEME_STORAGE_KEY = "captures-custom-theme";

const CUSTOM_THEME_PROPERTIES = [
  "--theme-accent",
  "--theme-accent-hover",
  "--theme-accent-strong",
  "--theme-accent-ink",
  "--theme-accent-readable",
  "--theme-accent-rgb",
  "--theme-accent-text",
  "--theme-accent-text-strong",
  "--theme-accent-surface",
  "--theme-accent-surface-strong",
  "--theme-signal",
  "--theme-signal-hover",
  "--theme-signal-strong",
  "--theme-signal-deep",
  "--theme-signal-ink",
  "--theme-signal-rgb",
  "--theme-signal-text",
  "--theme-signal-text-strong",
  "--theme-signal-surface",
] as const;

const DARK_INK = "#17181b";
const LIGHT_INK = "#ffffff";
const BLACK_INK = "#000000";
const DARK_SURFACE = "#11121a";
const WEB_CANVAS = "#faf9f5";

export function isColorTheme(value: unknown): value is ColorTheme {
  return COLOR_THEMES.some((theme) => theme.id === value);
}

export function normalizeHexColor(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim().toLowerCase();
  if (/^#[0-9a-f]{6}$/u.test(trimmed)) return trimmed;
  const shorthand = trimmed.match(/^#([0-9a-f])([0-9a-f])([0-9a-f])$/u);
  if (!shorthand) return null;
  return `#${shorthand.slice(1).map((channel) => channel.repeat(2)).join("")}`;
}

export function normalizeCustomThemeColors(value: unknown): CustomThemeColors {
  const candidate = value && typeof value === "object"
    ? value as Partial<CustomThemeColors>
    : {};
  return {
    accent: normalizeHexColor(candidate.accent) ?? DEFAULT_CUSTOM_THEME.accent,
    signal: normalizeHexColor(candidate.signal) ?? DEFAULT_CUSTOM_THEME.signal,
  };
}

export function readStoredColorTheme(): ColorTheme {
  try {
    const stored = window.localStorage.getItem(COLOR_THEME_STORAGE_KEY);
    if (stored === "saffron") return "violet";
    return isColorTheme(stored) ? stored : DEFAULT_COLOR_THEME;
  } catch {
    return DEFAULT_COLOR_THEME;
  }
}

export function readStoredCustomTheme(): CustomThemeColors {
  try {
    const stored = window.localStorage.getItem(CUSTOM_THEME_STORAGE_KEY);
    return normalizeCustomThemeColors(stored ? JSON.parse(stored) : null);
  } catch {
    return { ...DEFAULT_CUSTOM_THEME };
  }
}

export function buildCustomThemeVariables(colors: CustomThemeColors): Record<string, string> {
  const custom = normalizeCustomThemeColors(colors);
  const accentInk = preferredInk(custom.accent);
  const signalInk = preferredInk(custom.signal);
  const accentHover = interactiveShade(custom.accent, accentInk);
  const signalHover = interactiveShade(custom.signal, signalInk);

  return {
    "--theme-accent": custom.accent,
    "--theme-accent-hover": accentHover,
    "--theme-accent-strong": mix(custom.accent, "#000000", 0.14),
    "--theme-accent-ink": accentInk,
    "--theme-accent-readable": ensureContrast(custom.accent, WEB_CANVAS, "#000000"),
    "--theme-accent-rgb": rgbChannels(custom.accent),
    "--theme-accent-text": ensureContrast(mix(custom.accent, LIGHT_INK, 0.38), DARK_SURFACE, LIGHT_INK),
    "--theme-accent-text-strong": mix(custom.accent, LIGHT_INK, 0.72),
    "--theme-accent-surface": mix(DARK_SURFACE, custom.accent, 0.14),
    "--theme-accent-surface-strong": mix(DARK_SURFACE, custom.accent, 0.22),
    "--theme-signal": custom.signal,
    "--theme-signal-hover": signalHover,
    "--theme-signal-strong": mix(custom.signal, "#000000", 0.14),
    "--theme-signal-deep": mix(custom.signal, "#000000", 0.3),
    "--theme-signal-ink": signalInk,
    "--theme-signal-rgb": rgbChannels(custom.signal),
    "--theme-signal-text": ensureContrast(mix(custom.signal, LIGHT_INK, 0.42), DARK_SURFACE, LIGHT_INK),
    "--theme-signal-text-strong": mix(custom.signal, LIGHT_INK, 0.64),
    "--theme-signal-surface": mix(DARK_SURFACE, custom.signal, 0.14),
  };
}

export function applyColorTheme(
  theme: ColorTheme,
  customTheme: CustomThemeColors = readStoredCustomTheme(),
): void {
  const root = document.documentElement;
  const custom = normalizeCustomThemeColors(customTheme);
  root.dataset.captureTheme = theme;

  for (const property of CUSTOM_THEME_PROPERTIES) {
    root.style.removeProperty(property);
  }
  if (theme === "custom") {
    for (const [property, value] of Object.entries(buildCustomThemeVariables(custom))) {
      root.style.setProperty(property, value);
    }
  }

  try {
    window.localStorage.setItem(COLOR_THEME_STORAGE_KEY, theme);
    window.localStorage.setItem(CUSTOM_THEME_STORAGE_KEY, JSON.stringify(custom));
  } catch {
    // Persisted app settings remain the source of truth when storage is unavailable.
  }
}

function interactiveShade(color: string, ink: string): string {
  const toward = ink === LIGHT_INK ? "#000000" : LIGHT_INK;
  const candidate = mix(color, toward, ink === LIGHT_INK ? 0.06 : 0.1);
  return ensureContrast(candidate, ink, toward);
}

function preferredInk(background: string): string {
  if (contrast(background, DARK_INK) >= 4.5) return DARK_INK;
  if (contrast(background, LIGHT_INK) >= 4.5) return LIGHT_INK;
  return BLACK_INK;
}

function ensureContrast(foreground: string, background: string, toward: string): string {
  if (contrast(foreground, background) >= 4.5) return foreground;
  for (let amount = 0.04; amount <= 1; amount += 0.04) {
    const candidate = mix(foreground, toward, amount);
    if (contrast(candidate, background) >= 4.5) return candidate;
  }
  return toward;
}

function contrast(first: string, second: string): number {
  const [lighter, darker] = [luminance(first), luminance(second)].sort((a, b) => b - a);
  return (lighter + 0.05) / (darker + 0.05);
}

function luminance(color: string): number {
  const { red, green, blue } = parseHex(color);
  const linear = [red, green, blue].map((channel) => {
    const normalized = channel / 255;
    return normalized <= 0.03928
      ? normalized / 12.92
      : ((normalized + 0.055) / 1.055) ** 2.4;
  });
  return (0.2126 * linear[0]) + (0.7152 * linear[1]) + (0.0722 * linear[2]);
}

function mix(first: string, second: string, amount: number): string {
  const start = parseHex(first);
  const end = parseHex(second);
  const channel = (from: number, to: number) => Math.round(from + ((to - from) * amount));
  return toHex({
    red: channel(start.red, end.red),
    green: channel(start.green, end.green),
    blue: channel(start.blue, end.blue),
  });
}

function rgbChannels(color: string): string {
  const { red, green, blue } = parseHex(color);
  return `${red}, ${green}, ${blue}`;
}

function parseHex(color: string): { red: number; green: number; blue: number } {
  const normalized = normalizeHexColor(color) ?? "#000000";
  return {
    red: Number.parseInt(normalized.slice(1, 3), 16),
    green: Number.parseInt(normalized.slice(3, 5), 16),
    blue: Number.parseInt(normalized.slice(5, 7), 16),
  };
}

function toHex({ red, green, blue }: { red: number; green: number; blue: number }): string {
  return `#${[red, green, blue]
    .map((channel) => channel.toString(16).padStart(2, "0"))
    .join("")}`;
}
