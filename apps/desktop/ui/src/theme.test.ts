import {
  applyColorTheme,
  COLOR_THEME_STORAGE_KEY,
  DEFAULT_COLOR_THEME,
  isColorTheme,
  readStoredColorTheme,
} from "../../../../shared/themes";

describe("color themes", () => {
  afterEach(() => {
    document.documentElement.removeAttribute("data-capture-theme");
    window.localStorage.clear();
  });

  it("applies and remembers a selected theme", () => {
    applyColorTheme("mint");

    expect(document.documentElement).toHaveAttribute("data-capture-theme", "mint");
    expect(window.localStorage.getItem(COLOR_THEME_STORAGE_KEY)).toBe("mint");
    expect(readStoredColorTheme()).toBe("mint");
  });

  it("falls back when stored theme data is unknown", () => {
    window.localStorage.setItem(COLOR_THEME_STORAGE_KEY, "future-theme");

    expect(readStoredColorTheme()).toBe(DEFAULT_COLOR_THEME);
    expect(isColorTheme("future-theme")).toBe(false);
  });
});
