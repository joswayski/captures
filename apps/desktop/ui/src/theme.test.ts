import {
  applyColorTheme,
  buildCustomThemeVariables,
  COLOR_THEME_STORAGE_KEY,
  CUSTOM_THEME_STORAGE_KEY,
  DEFAULT_COLOR_THEME,
  DEFAULT_CUSTOM_THEME,
  isColorTheme,
  normalizeHexColor,
  readStoredColorTheme,
  readStoredCustomTheme,
} from "../../../../shared/themes";

describe("color themes", () => {
  afterEach(() => {
    document.documentElement.removeAttribute("data-capture-theme");
    document.documentElement.removeAttribute("style");
    window.localStorage.clear();
  });

  it("applies and remembers a selected preset", () => {
    applyColorTheme("mint", DEFAULT_CUSTOM_THEME);

    expect(document.documentElement).toHaveAttribute("data-capture-theme", "mint");
    expect(window.localStorage.getItem(COLOR_THEME_STORAGE_KEY)).toBe("mint");
    expect(readStoredColorTheme()).toBe("mint");
  });

  it("falls back when stored theme data is unknown", () => {
    window.localStorage.setItem(COLOR_THEME_STORAGE_KEY, "future-theme");

    expect(readStoredColorTheme()).toBe(DEFAULT_COLOR_THEME);
    expect(isColorTheme("future-theme")).toBe(false);
  });

  it("replaces a locally stored Saffron selection with Violet", () => {
    window.localStorage.setItem(COLOR_THEME_STORAGE_KEY, "saffron");

    expect(readStoredColorTheme()).toBe("violet");
  });

  it("normalizes, applies, and persists custom colors", () => {
    applyColorTheme("custom", {
      accent: "#1a2",
      signal: "#345678",
    });

    expect(document.documentElement).toHaveAttribute("data-capture-theme", "custom");
    expect(document.documentElement.style.getPropertyValue("--theme-accent")).toBe("#11aa22");
    expect(document.documentElement.style.getPropertyValue("--theme-signal")).toBe("#345678");
    expect(JSON.parse(window.localStorage.getItem(CUSTOM_THEME_STORAGE_KEY) ?? "")).toEqual({
      accent: "#11aa22",
      signal: "#345678",
    });
    expect(readStoredCustomTheme()).toEqual({
      accent: "#11aa22",
      signal: "#345678",
    });
    expect(normalizeHexColor("not-a-color")).toBeNull();
  });

  it("derives readable custom action colors and clears them for presets", () => {
    const variables = buildCustomThemeVariables({
      accent: "#ffffff",
      signal: "#000000",
    });

    expect(contrast(variables["--theme-accent"], variables["--theme-accent-ink"])).toBeGreaterThanOrEqual(4.5);
    expect(contrast(variables["--theme-accent-hover"], variables["--theme-accent-ink"])).toBeGreaterThanOrEqual(4.5);
    expect(contrast(variables["--theme-signal"], variables["--theme-signal-ink"])).toBeGreaterThanOrEqual(4.5);

    applyColorTheme("custom", {
      accent: "#ffffff",
      signal: "#000000",
    });
    applyColorTheme("mustard", DEFAULT_CUSTOM_THEME);

    expect(document.documentElement.style.getPropertyValue("--theme-accent")).toBe("");
    expect(document.documentElement.style.getPropertyValue("--theme-signal")).toBe("");
  });

  it("keeps custom action text readable on middle gray colors", () => {
    const variables = buildCustomThemeVariables({
      accent: "#777777",
      signal: "#777777",
    });

    expect(contrast(variables["--theme-accent"], variables["--theme-accent-ink"])).toBeGreaterThanOrEqual(4.5);
    expect(contrast(variables["--theme-signal"], variables["--theme-signal-ink"])).toBeGreaterThanOrEqual(4.5);
  });
});

function contrast(first: string, second: string): number {
  const [lighter, darker] = [luminance(first), luminance(second)].sort((a, b) => b - a);
  return (lighter + 0.05) / (darker + 0.05);
}

function luminance(hex: string): number {
  const channels = [1, 3, 5].map(
    (index) => Number.parseInt(hex.slice(index, index + 2), 16) / 255,
  );
  const linear = channels.map((channel) => (
    channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4
  ));
  return (0.2126 * linear[0]) + (0.7152 * linear[1]) + (0.0722 * linear[2]);
}
