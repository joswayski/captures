import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent } from "@testing-library/react";
import { play, set } from "@foleyjs/core";
import { installInteractionSounds, playSound, setSoundsEnabled, soundsEnabled } from "./sounds";

vi.mock("@foleyjs/core", () => ({ play: vi.fn(), set: vi.fn() }));

describe("interaction sounds", () => {
  let dispose: () => void;
  beforeEach(() => {
    localStorage.clear();
    vi.clearAllMocks();
    document.body.innerHTML = "";
    dispose = installInteractionSounds();
  });
  afterEach(() => dispose());

  it("covers dynamically mounted controls and nested button icons once", () => {
    document.body.innerHTML = '<button><svg><path /></svg></button>';
    fireEvent.click(document.querySelector("path")!);
    expect(play).toHaveBeenCalledExactlyOnceWith("tap", undefined);
  });

  it("uses the new native checkbox state, including forwarded label clicks", () => {
    document.body.innerHTML = '<label><input type="checkbox" />Sounds</label>';
    document.querySelector("label")!.click();
    expect(play).toHaveBeenLastCalledWith("on", undefined);
    document.querySelector("label")!.click();
    expect(play).toHaveBeenLastCalledWith("off", undefined);
    expect(play).toHaveBeenCalledTimes(2);
  });

  it("distinguishes modes and aria switches", () => {
    document.body.innerHTML = '<button aria-pressed="false">Mode</button><button role="switch" aria-checked="true">Toggle</button>';
    fireEvent.click(document.querySelector("button")!);
    expect(play).toHaveBeenLastCalledWith("switch", undefined);
    fireEvent.click(document.querySelector('[role="switch"]')!);
    expect(play).toHaveBeenLastCalledWith("off", undefined);
  });

  it("skips disabled, opted-out controls, typing, and hover", () => {
    document.body.innerHTML = '<button disabled>Disabled</button><div data-sound="off"><button>Custom audio</button></div><button aria-disabled="true">Unavailable</button><input />';
    document.querySelectorAll("button").forEach((el) => fireEvent.click(el));
    fireEvent.input(document.querySelector("input")!, { target: { value: "hello" } });
    fireEvent.pointerOver(document.querySelector("input")!);
    expect(play).not.toHaveBeenCalled();
  });

  it("throttles continuous slider input", () => {
    document.body.innerHTML = '<input type="range" />';
    const slider = document.querySelector("input")!;
    for (let i = 0; i < 20; i++) fireEvent.input(slider);
    expect(play).toHaveBeenCalledExactlyOnceWith("tick", { volume: 0.35 });
  });

  it("sounds canvas gesture endpoints but not movement or cancelled drops", () => {
    document.body.innerHTML = '<div data-sound-gesture><canvas /></div>';
    const canvas = document.querySelector("canvas")!;
    const pointer = (type: string) => {
      const event = new MouseEvent(type, { bubbles: true, button: 0 });
      Object.defineProperty(event, "pointerId", { value: 1 });
      fireEvent(canvas, event);
    };
    pointer("pointerdown");
    pointer("pointermove");
    pointer("pointerup");
    expect(vi.mocked(play).mock.calls.map(([cue]) => cue)).toEqual(["press", "drop"]);
    vi.clearAllMocks();
    pointer("pointerdown");
    pointer("pointercancel");
    pointer("pointerup");
    expect(play).toHaveBeenCalledExactlyOnceWith("press", { volume: 0.5 });
  });

  it("persists mute, applies it immediately, and acknowledges enabling", () => {
    setSoundsEnabled(false);
    expect(soundsEnabled()).toBe(false);
    playSound("complete");
    expect(play).not.toHaveBeenCalled();
    expect(set).toHaveBeenLastCalledWith({ muted: true });
    setSoundsEnabled(true);
    expect(play).toHaveBeenCalledExactlyOnceWith("on", undefined);
    expect(set).toHaveBeenLastCalledWith({ muted: false });
  });

  it("syncs mute from another window and removes listeners on teardown", () => {
    localStorage.setItem("captures-interaction-sounds", "off");
    window.dispatchEvent(new StorageEvent("storage", { key: "captures-interaction-sounds" }));
    expect(set).toHaveBeenLastCalledWith({ muted: true });
    localStorage.clear();
    dispose();
    document.body.innerHTML = "<button>Save</button>";
    fireEvent.click(document.querySelector("button")!);
    expect(play).not.toHaveBeenCalled();
  });

  it("announces a new error once rather than every DOM mutation", async () => {
    document.body.innerHTML = '<p role="alert">Could not save</p>';
    await Promise.resolve();
    expect(play).toHaveBeenCalledExactlyOnceWith("error", undefined);
    document.body.append(document.createElement("span"));
    await Promise.resolve();
    expect(play).toHaveBeenCalledTimes(1);
  });

  it("does not break actions when audio is unavailable", () => {
    vi.mocked(play).mockImplementationOnce(() => { throw new Error("Audio unavailable"); });
    expect(() => playSound("tap")).not.toThrow();
  });
});
