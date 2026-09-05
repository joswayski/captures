import {
  collectPreferenceFindTargets,
  matchPreferenceFindTargets,
  preferenceFindCountLabel,
  preferenceTextMatches,
  preferencesFindCommand,
  wrapFindIndex,
} from "./preferencesFind";

function keyEvent(
  key: string,
  code: string,
  modifiers: Partial<Pick<KeyboardEvent, "altKey" | "ctrlKey" | "metaKey" | "shiftKey">> = {},
) {
  return {
    key,
    code,
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    shiftKey: false,
    defaultPrevented: false,
    isComposing: false,
    target: document.body,
    ...modifiers,
  };
}

describe("preference find matching", () => {
  it("matches setting copy without depending on extra whitespace", () => {
    expect(preferenceTextMatches("Automatically copy captures\n  to the clipboard", "clipboard"))
      .toBe(true);
    expect(preferenceTextMatches("Accent color", "  ACCENT ")).toBe(true);
    expect(preferenceTextMatches("Accent color", "")).toBe(false);
    expect(preferenceTextMatches("Accent color", "clipboard")).toBe(false);
  });

  it("collects find targets from preference chrome and skips nested controls", () => {
    const root = document.createElement("div");
    root.innerHTML = `
      <section class="settings-card">
        <header class="settings-card-header"><h2>Capture</h2></header>
        <div class="setting-row"><span>Save captures to</span><input value="/tmp" /></div>
        <label class="check-row switch-row"><span>Automatically copy captures to the clipboard</span></label>
        <div class="shortcut-row"><span>New Capture</span></div>
      </section>
    `;
    const targets = collectPreferenceFindTargets(root);
    expect(targets.map((target) => target.className)).toEqual([
      "settings-card-header",
      "setting-row",
      "check-row switch-row",
      "shortcut-row",
    ]);
    expect(matchPreferenceFindTargets(targets, "clipboard")).toHaveLength(1);
    expect(matchPreferenceFindTargets(targets, "save captures")).toHaveLength(1);
    expect(matchPreferenceFindTargets(targets, "new capture")).toHaveLength(1);
  });

  it("wraps next and previous indices", () => {
    expect(wrapFindIndex(3, 2, 1)).toBe(0);
    expect(wrapFindIndex(3, 0, -1)).toBe(2);
    expect(wrapFindIndex(0, 0, 1)).toBe(0);
  });

  it("labels match counts the way the find bar should read them", () => {
    expect(preferenceFindCountLabel("", 0, 0)).toBe("");
    expect(preferenceFindCountLabel("cursor", 0, 0)).toBe("No results");
    expect(preferenceFindCountLabel("cursor", 2, 1)).toBe("2 of 2");
  });
});

describe("preference find shortcuts", () => {
  it("opens find with the platform find chord", () => {
    expect(preferencesFindCommand(keyEvent("f", "KeyF", { metaKey: true }), "macos", false))
      .toBe("open");
    expect(preferencesFindCommand(keyEvent("f", "KeyF", { ctrlKey: true }), "macos", false))
      .toBeNull();
    expect(preferencesFindCommand(keyEvent("f", "KeyF", { ctrlKey: true }), "windows", false))
      .toBe("open");
    expect(preferencesFindCommand(keyEvent("f", "KeyF", { ctrlKey: true }), "linux", false))
      .toBe("open");
    expect(preferencesFindCommand(keyEvent("f", "KeyF", { metaKey: true }), "windows", false))
      .toBeNull();
  });

  it("keeps find-next chords inert until the find bar is open", () => {
    expect(preferencesFindCommand(keyEvent("g", "KeyG", { metaKey: true }), "macos", false))
      .toBeNull();
    expect(preferencesFindCommand(keyEvent("g", "KeyG", { metaKey: true }), "macos", true))
      .toBe("next");
    expect(preferencesFindCommand(keyEvent("g", "KeyG", { metaKey: true, shiftKey: true }), "macos", true))
      .toBe("previous");
    expect(preferencesFindCommand(keyEvent("F3", "F3", { ctrlKey: true }), "linux", true))
      .toBeNull();
    expect(preferencesFindCommand(keyEvent("F3", "F3"), "linux", true)).toBe("next");
    expect(preferencesFindCommand(keyEvent("F3", "F3", { shiftKey: true }), "windows", true))
      .toBe("previous");
  });

  it("closes find with Escape once the bar is open", () => {
    expect(preferencesFindCommand(keyEvent("Escape", "Escape"), "linux", false)).toBeNull();
    expect(preferencesFindCommand(keyEvent("Escape", "Escape"), "linux", true)).toBe("close");
  });

  it("uses Enter in the search field for next and previous, not find-bar buttons", () => {
    const find = document.createElement("div");
    find.className = "preferences-find";
    const input = document.createElement("input");
    input.className = "preferences-find-input";
    const previous = document.createElement("button");
    previous.setAttribute("aria-label", "Previous match");
    const close = document.createElement("button");
    close.setAttribute("aria-label", "Close find");
    find.append(input, previous, close);

    expect(preferencesFindCommand({ ...keyEvent("Enter", "Enter"), target: input }, "linux", true))
      .toBe("next");
    expect(preferencesFindCommand(
      { ...keyEvent("Enter", "Enter", { shiftKey: true }), target: input },
      "linux",
      true,
    )).toBe("previous");
    expect(preferencesFindCommand({ ...keyEvent("Enter", "Enter"), target: previous }, "linux", true))
      .toBeNull();
    expect(preferencesFindCommand({ ...keyEvent("Enter", "Enter"), target: close }, "linux", true))
      .toBeNull();
  });
});
