import {
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
    expect(recordShortcut(keyEvent("Digit4", { ctrlKey: true, shiftKey: true }))).toEqual({
      kind: "complete",
      keys: ["Ctrl", "Shift", "4"],
      shortcut: "Control+Shift+Digit4",
    });
  });

  it("records Command and Option combinations", () => {
    expect(recordShortcut(keyEvent("KeyW", { altKey: true, metaKey: true }))).toEqual({
      kind: "complete",
      keys: ["Option", "Cmd", "W"],
      shortcut: "Alt+Super+KeyW",
    });
  });

  it("waits while only modifier keys are held", () => {
    const event = keyEvent("ShiftLeft", { ctrlKey: true, shiftKey: true });
    expect(recordShortcut(event)).toEqual({ kind: "waiting", keys: ["Ctrl", "Shift"] });
    expect(modifierDisplayTokens(event)).toEqual(["Ctrl", "Shift"]);
  });

  it("requires a modifier and reserves Escape for cancellation", () => {
    expect(recordShortcut(keyEvent("KeyQ"))).toEqual({
      kind: "invalid",
      keys: ["Q"],
      message: "Include Ctrl, Shift, Option, or Command.",
    });
    expect(recordShortcut(keyEvent("Escape", { shiftKey: true }))).toEqual({ kind: "cancel" });
  });

  it("renders both legacy and canonical saved shortcut formats as keycaps", () => {
    expect(shortcutDisplayTokens("Ctrl+Shift+4")).toEqual(["Ctrl", "Shift", "4"]);
    expect(shortcutDisplayTokens("shift+control+Digit4")).toEqual(["Shift", "Ctrl", "4"]);
  });
});
