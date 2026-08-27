import {
  detectShortcutPlatform,
  modifierDisplayTokens,
  recordShortcut,
  shortcutDisplayTokens,
} from "./shortcut";

function keyEvent(
  code: string,
  modifiers: Partial<Pick<KeyboardEvent, "altKey" | "ctrlKey" | "metaKey" | "shiftKey">> = {},
) {
  return {
    code,
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    shiftKey: false,
    ...modifiers,
  };
}

describe("shortcut recording", () => {
  it("uses the physical digit key instead of its shifted character", () => {
    expect(recordShortcut(keyEvent("Digit4", { ctrlKey: true, shiftKey: true }), "macos")).toEqual({
      kind: "complete",
      keys: ["Ctrl", "Shift", "4"],
      shortcut: "Control+Shift+Digit4",
    });
  });

  it("records Command and Option combinations on macOS", () => {
    expect(recordShortcut(keyEvent("KeyW", { altKey: true, metaKey: true }), "macos")).toEqual({
      kind: "complete",
      keys: ["Option", "Cmd", "W"],
      shortcut: "Alt+Super+KeyW",
    });
  });

  it("labels Windows modifiers as Alt and Win", () => {
    expect(recordShortcut(keyEvent("KeyW", { altKey: true, metaKey: true }), "windows")).toEqual({
      kind: "complete",
      keys: ["Alt", "Win", "W"],
      shortcut: "Alt+Super+KeyW",
    });
    expect(shortcutDisplayTokens("Alt+Super+KeyW", "windows")).toEqual(["Alt", "Win", "W"]);
    expect(shortcutDisplayTokens("CommandOrControl+Shift+Digit4", "windows")).toEqual([
      "Ctrl",
      "Shift",
      "4",
    ]);
  });

  it("labels Linux Super distinctly from Windows Win", () => {
    expect(recordShortcut(keyEvent("KeyW", { altKey: true, metaKey: true }), "linux")).toEqual({
      kind: "complete",
      keys: ["Alt", "Super", "W"],
      shortcut: "Alt+Super+KeyW",
    });
  });

  it("waits while only modifier keys are held", () => {
    const event = keyEvent("ShiftLeft", { ctrlKey: true, shiftKey: true });
    expect(recordShortcut(event, "linux")).toEqual({ kind: "waiting", keys: ["Ctrl", "Shift"] });
    expect(modifierDisplayTokens(event, "linux")).toEqual(["Ctrl", "Shift"]);
  });

  it("requires a modifier and reserves Escape for cancellation", () => {
    expect(recordShortcut(keyEvent("KeyQ"), "macos")).toEqual({
      kind: "invalid",
      keys: ["Q"],
      message: "Include Ctrl, Shift, Option, or Command.",
    });
    expect(recordShortcut(keyEvent("KeyQ"), "windows")).toEqual({
      kind: "invalid",
      keys: ["Q"],
      message: "Include Ctrl, Shift, Alt, or Win.",
    });
    expect(recordShortcut(keyEvent("Escape", { shiftKey: true }), "linux")).toEqual({
      kind: "cancel",
    });
  });

  it("renders both legacy and canonical saved shortcut formats as keycaps", () => {
    expect(shortcutDisplayTokens("Ctrl+Shift+4", "linux")).toEqual(["Ctrl", "Shift", "4"]);
    expect(shortcutDisplayTokens("shift+control+Digit4", "linux")).toEqual(["Shift", "Ctrl", "4"]);
  });

  it("detects macOS and Windows from user-agent strings", () => {
    expect(detectShortcutPlatform("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)")).toBe("macos");
    expect(detectShortcutPlatform("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe("windows");
    expect(detectShortcutPlatform("Mozilla/5.0 (X11; Linux x86_64)")).toBe("linux");
  });
});
