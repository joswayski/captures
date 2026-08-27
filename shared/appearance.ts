export const APPEARANCE_MODES = [
  { id: "system", name: "System" },
  { id: "light", name: "Light" },
  { id: "dark", name: "Dark" },
] as const;

export type AppearanceMode = (typeof APPEARANCE_MODES)[number]["id"];
export type ResolvedAppearance = "light" | "dark";

export const DEFAULT_APPEARANCE: AppearanceMode = "system";
export const APPEARANCE_STORAGE_KEY = "captures-appearance";

export function isAppearanceMode(value: unknown): value is AppearanceMode {
  return APPEARANCE_MODES.some((mode) => mode.id === value);
}

export function prefersDarkAppearance(): boolean {
  try {
    return window.matchMedia("(prefers-color-scheme: dark)").matches;
  } catch {
    // jsdom and older WebViews do not implement matchMedia; dark is the default.
    return true;
  }
}

export function resolveAppearance(mode: AppearanceMode): ResolvedAppearance {
  if (mode === "light" || mode === "dark") return mode;
  return prefersDarkAppearance() ? "dark" : "light";
}

export function readStoredAppearance(): AppearanceMode {
  try {
    const stored = window.localStorage.getItem(APPEARANCE_STORAGE_KEY);
    return isAppearanceMode(stored) ? stored : DEFAULT_APPEARANCE;
  } catch {
    return DEFAULT_APPEARANCE;
  }
}

/**
 * Applies an appearance mode to the document.
 *
 * `data-appearance` is the resolved light/dark value that CSS reads;
 * `data-appearance-mode` keeps the user's choice so "System" can be reflected
 * in the UI without re-reading storage.
 */
export function applyAppearance(mode: AppearanceMode): ResolvedAppearance {
  const resolved = resolveAppearance(mode);
  const root = document.documentElement;
  root.dataset.appearance = resolved;
  root.dataset.appearanceMode = mode;

  try {
    window.localStorage.setItem(APPEARANCE_STORAGE_KEY, mode);
  } catch {
    // Persisted app settings remain the source of truth when storage is unavailable.
  }
  return resolved;
}

/**
 * Re-applies the resolved appearance whenever the OS flips light/dark.
 * Returns a disposer; a no-op when the platform cannot report the preference.
 */
export function watchSystemAppearance(onChange: () => void): () => void {
  let query: MediaQueryList;
  try {
    query = window.matchMedia("(prefers-color-scheme: dark)");
  } catch {
    return () => undefined;
  }
  const listener = () => onChange();
  if (typeof query.addEventListener === "function") {
    query.addEventListener("change", listener);
    return () => query.removeEventListener("change", listener);
  }
  return () => undefined;
}
