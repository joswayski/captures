import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { App } from "./App";
import { isPointerOverCaptureGuidance } from "./lib/captureGuidance";
import type { ActiveSession } from "./types";

const { hideCurrentWindow } = vi.hoisted(() => ({
  hideCurrentWindow: vi.fn(async () => undefined),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => true,
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(),
  listen: vi.fn(async () => () => undefined),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ hide: hideCurrentWindow }),
}));

const session: ActiveSession = {
  id: "capture-1",
  mode: "region",
  display: {
    id: "display-1",
    name: "Display",
    x: 0,
    y: 0,
    width: 1440,
    height: 900,
    scale_factor: 2,
    is_primary: true,
  },
  window_coordinate_scale: 1,
  window_corner_radius: 25,
  frozen: true,
  snapshot_url: "capture://session/capture-1",
  windows: [],
};

const guidanceBounds = {
  x: 500,
  y: 120,
  top: 120,
  left: 500,
  right: 760,
  bottom: 180,
  width: 260,
  height: 60,
  toJSON: () => undefined,
} as DOMRect;

/**
 * jsdom completes `capture://` images with `load`/`error` on the next task.
 * Record the URL without setting the src attribute so the overlay cannot reveal
 * until a test fires `load`.
 */
function pauseHtmlImageLoading() {
  const proto = HTMLImageElement.prototype;
  const previousSrc = Object.getOwnPropertyDescriptor(proto, "src");
  const previousSetAttribute = proto.setAttribute;
  const paused = new WeakMap<HTMLImageElement, string>();
  Object.defineProperty(proto, "src", {
    configurable: true,
    enumerable: previousSrc?.enumerable ?? true,
    get() {
      return paused.get(this) ?? this.getAttribute("src") ?? "";
    },
    set(value: string) {
      paused.set(this, String(value));
    },
  });
  proto.setAttribute = function setAttribute(name: string, value: string) {
    if (String(name).toLowerCase() === "src") {
      paused.set(this as unknown as HTMLImageElement, String(value));
      return;
    }
    previousSetAttribute.call(this, name, value);
  };
  return () => {
    proto.setAttribute = previousSetAttribute;
    if (previousSrc) Object.defineProperty(proto, "src", previousSrc);
  };
}

/**
 * Mock guidance geometry on the prototype so React remounts / Strict Mode
 * cannot drop a per-element spy before pointer handlers read bounds.
 */
function mockGuidanceBounds(rect: DOMRect = guidanceBounds) {
  return vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(
    function mockRect(this: HTMLElement) {
      if (this.classList?.contains("capture-guidance")) {
        return rect;
      }
      return {
        x: 0,
        y: 0,
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        width: 0,
        height: 0,
        toJSON: () => undefined,
      } as DOMRect;
    },
  );
}

/** Re-dispatch the move until the listener is attached. waitFor alone will not. */
async function movePointerOverGuidance(
  guidance: HTMLElement,
  coords: { clientX: number; clientY: number },
  faded: boolean,
) {
  await waitFor(() => {
    fireEvent.pointerMove(window, coords);
    if (faded) {
      expect(guidance).toHaveAttribute("data-faded", "true");
    } else {
      expect(guidance).not.toHaveAttribute("data-faded");
    }
  });
}

describe("CaptureOverlay guidance", () => {
  let activeSession: ActiveSession | null;
  let capturePointer: { x: number; y: number; inside: boolean } | null;
  let restoreImageLoading: (() => void) | undefined;

  beforeEach(() => {
    restoreImageLoading = pauseHtmlImageLoading();
    activeSession = session;
    capturePointer = null;
    vi.mocked(listen).mockResolvedValue(() => undefined);
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_active_session" || command === "get_pending_session") {
        return activeSession;
      }
      if (command === "get_capture_pointer_position") {
        return capturePointer;
      }
      if (
        command === "show_capture_overlay"
        || command === "reveal_capture_overlay"
        || command === "sync_capture_cursor"
        || command === "cancel_capture"
        || command === "commit_window"
        || command === "commit_display"
        || command === "commit_region"
        || command === "dismiss_capture_surface"
        || command === "cancel_active_capture"
      ) {
        return undefined;
      }
      throw new Error(`unexpected command: ${command}`);
    });
  });

  afterEach(() => {
    window.history.replaceState({}, "", "/");
    document.documentElement.classList.remove(
      "capture-region-cursor",
      "capture-window-cursor",
      "capture-display-cursor",
    );
    restoreImageLoading?.();
    restoreImageLoading = undefined;
    vi.restoreAllMocks();
    vi.clearAllMocks();
  });

  it("uses the selector guidance for a region shortcut", async () => {
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1",
    );
    render(<App />);

    const guidance = (await screen.findByText("Drag to select a region"))
      .closest(".capture-guidance");
    expect(guidance).toHaveTextContent("Shift for square · Esc to cancel");
    expect(screen.queryByText("Drag to capture · Esc to cancel")).not.toBeInTheDocument();
  });

  it("uses the selector guidance for a window shortcut", async () => {
    activeSession = { ...session, mode: "window" };
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=window&session_id=capture-1",
    );
    render(<App />);

    const guidance = (await screen.findByText("Select a window to continue"))
      .closest(".capture-guidance");
    expect(guidance).toHaveTextContent("Esc to cancel");
    expect(screen.queryByText("Select a window · Esc to cancel")).not.toBeInTheDocument();
  });

  it("fades region guidance when the cursor enters its bounds and restores on leave", async () => {
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1",
    );
    mockGuidanceBounds();
    render(<App />);

    const guidance = (await screen.findByText("Drag to select a region"))
      .closest(".capture-guidance") as HTMLElement;
    expect(guidance).not.toHaveAttribute("data-faded");

    await movePointerOverGuidance(guidance, { clientX: 620, clientY: 150 }, true);
    await movePointerOverGuidance(guidance, { clientX: 20, clientY: 20 }, false);
  });

  it("keeps region guidance faded while the cursor rests on the leave slack edge", async () => {
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1",
    );
    mockGuidanceBounds();
    render(<App />);

    const guidance = (await screen.findByText("Drag to select a region"))
      .closest(".capture-guidance") as HTMLElement;

    await movePointerOverGuidance(guidance, { clientX: 620, clientY: 150 }, true);
    // Just outside the painted box but inside the 40px leave zone — stay faded.
    await movePointerOverGuidance(guidance, { clientX: 790, clientY: 150 }, true);
    // Clear the slack zone — restore.
    await movePointerOverGuidance(guidance, { clientX: 810, clientY: 150 }, false);
  });

  it("fades region guidance as the cursor approaches the chip", async () => {
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1",
    );
    mockGuidanceBounds();
    render(<App />);

    const guidance = (await screen.findByText("Drag to select a region"))
      .closest(".capture-guidance") as HTMLElement;

    // On the painted left edge — fade immediately.
    await movePointerOverGuidance(guidance, { clientX: 500, clientY: 150 }, true);
    // Still in the 28px approach pad.
    await movePointerOverGuidance(guidance, { clientX: 480, clientY: 150 }, true);
  });

  it("fades window guidance when the cursor enters its bounds", async () => {
    activeSession = { ...session, mode: "window" };
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=window&session_id=capture-1",
    );
    mockGuidanceBounds();
    render(<App />);

    const guidance = (await screen.findByText("Select a window to continue"))
      .closest(".capture-guidance") as HTMLElement;

    await movePointerOverGuidance(guidance, { clientX: 620, clientY: 150 }, true);
  });

  it("uses enter/leave hysteresis for guidance hit testing", () => {
    const bounds = { left: 500, right: 760, top: 120, bottom: 180 };

    // Approach pad is 28px; the painted edge and nearby cursor fade.
    expect(isPointerOverCaptureGuidance(500, 150, bounds, false)).toBe(true);
    expect(isPointerOverCaptureGuidance(480, 150, bounds, false)).toBe(true);
    expect(isPointerOverCaptureGuidance(471, 150, bounds, false)).toBe(false);
    expect(isPointerOverCaptureGuidance(510, 150, bounds, false)).toBe(true);
    // Leave slack is 12px beyond the 28px approach pad while already faded.
    expect(isPointerOverCaptureGuidance(790, 150, bounds, true)).toBe(true);
    expect(isPointerOverCaptureGuidance(801, 150, bounds, true)).toBe(false);
  });

  it("hides region guidance while the user is dragging a selection", async () => {
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1",
    );
    const { container } = render(<App />);
    const guidance = (await screen.findByText("Drag to select a region"))
      .closest(".capture-guidance");
    expect(guidance).not.toHaveAttribute("data-faded");

    const surface = container.querySelector<HTMLElement>(".capture-surface");
    expect(surface).not.toBeNull();
    surface!.setPointerCapture = vi.fn();
    vi.spyOn(surface!, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 1440,
      bottom: 900,
      width: 1440,
      height: 900,
      toJSON: () => undefined,
    });

    fireEvent.pointerDown(surface!, { pointerId: 1, clientX: 200, clientY: 150 });
    expect(guidance).toHaveAttribute("data-faded", "true");
  });

  it("constrains a shortcut-started region to a square while Shift is held", async () => {
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1",
    );
    const { container } = render(<App />);
    await screen.findByText("Drag to select a region");

    const surface = container.querySelector<HTMLElement>(".capture-surface");
    expect(surface).not.toBeNull();
    surface!.setPointerCapture = vi.fn();
    vi.spyOn(surface!, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 1440,
      bottom: 900,
      width: 1440,
      height: 900,
      toJSON: () => undefined,
    });

    fireEvent.pointerDown(surface!, { pointerId: 1, clientX: 120, clientY: 80 });
    fireEvent.pointerMove(surface!, { pointerId: 1, clientX: 620, clientY: 480 });
    fireEvent.keyDown(window, { key: "Shift" });

    await waitFor(() => {
      expect(container.querySelector(".selection-box")).toHaveStyle({
        left: "120px",
        top: "80px",
        width: "500px",
        height: "500px",
      });
    });

    fireEvent.pointerUp(surface!, {
      pointerId: 1,
      clientX: 620,
      clientY: 480,
      shiftKey: true,
    });
    expect(invoke).toHaveBeenCalledWith("commit_region", {
      sessionId: "capture-1",
      rect: { x: 120, y: 80, width: 500, height: 500 },
    });
  });

  it("commits the freeform region if Shift is released before the mouse", async () => {
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1",
    );
    const { container } = render(<App />);
    await screen.findByText("Drag to select a region");

    const surface = container.querySelector<HTMLElement>(".capture-surface");
    expect(surface).not.toBeNull();
    surface!.setPointerCapture = vi.fn();
    surface!.hasPointerCapture = vi.fn(() => true);
    surface!.releasePointerCapture = vi.fn();
    vi.spyOn(surface!, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 1440,
      bottom: 900,
      width: 1440,
      height: 900,
      toJSON: () => undefined,
    });

    fireEvent.pointerDown(surface!, { pointerId: 1, clientX: 120, clientY: 80 });
    fireEvent.pointerMove(surface!, { pointerId: 1, clientX: 620, clientY: 480 });
    fireEvent.keyDown(window, { key: "Shift" });
    await waitFor(() => {
      expect(container.querySelector(".selection-box")).toHaveStyle({
        width: "500px",
        height: "500px",
      });
    });
    fireEvent.keyUp(window, { key: "Shift" });
    await waitFor(() => {
      expect(container.querySelector(".selection-box")).toHaveStyle({
        width: "500px",
        height: "400px",
      });
    });
    fireEvent.pointerUp(surface!, {
      pointerId: 1,
      clientX: 620,
      clientY: 480,
    });
    expect(invoke).toHaveBeenCalledWith("commit_region", {
      sessionId: "capture-1",
      rect: { x: 120, y: 80, width: 500, height: 400 },
    });
  });

  it("starts hiding the native overlay as soon as a region drag is released", async () => {
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1",
    );
    const { container } = render(<App />);
    await screen.findByText("Drag to select a region");

    const surface = container.querySelector<HTMLElement>(".capture-surface");
    expect(surface).not.toBeNull();
    surface!.setPointerCapture = vi.fn();
    vi.spyOn(surface!, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 1440,
      bottom: 900,
      width: 1440,
      height: 900,
      toJSON: () => undefined,
    });

    fireEvent.pointerDown(surface!, { pointerId: 1, clientX: 120, clientY: 80 });
    fireEvent.pointerMove(surface!, { pointerId: 1, clientX: 620, clientY: 480 });
    fireEvent.pointerUp(surface!, { pointerId: 1, clientX: 620, clientY: 480 });

    expect(hideCurrentWindow).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledWith("dismiss_capture_surface");
    expect(invoke).toHaveBeenCalledWith("commit_region", {
      sessionId: "capture-1",
      rect: { x: 120, y: 80, width: 500, height: 400 },
    });
    const dismissCall = vi.mocked(invoke).mock.calls.findIndex(([command]) => (
      command === "dismiss_capture_surface"
    ));
    const commitCall = vi.mocked(invoke).mock.calls.findIndex(([command]) => (
      command === "commit_region"
    ));
    expect(dismissCall).toBeGreaterThanOrEqual(0);
    expect(commitCall).toBeGreaterThanOrEqual(0);
    expect(vi.mocked(invoke).mock.invocationCallOrder[dismissCall]).toBeLessThan(
      hideCurrentWindow.mock.invocationCallOrder[0],
    );
    expect(hideCurrentWindow.mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(invoke).mock.invocationCallOrder[commitCall],
    );
  });

  it("starts hiding the native overlay as soon as Escape cancels the session", async () => {
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1",
    );
    render(<App />);
    await screen.findByText("Drag to select a region");

    fireEvent.keyDown(window, { key: "Escape" });

    expect(hideCurrentWindow).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledWith("dismiss_capture_surface");
    expect(invoke).toHaveBeenCalledWith("cancel_capture", { sessionId: "capture-1" });
    const dismissCall = vi.mocked(invoke).mock.calls.findIndex(([command]) => (
      command === "dismiss_capture_surface"
    ));
    const cancelCall = vi.mocked(invoke).mock.calls.findIndex(([command]) => (
      command === "cancel_capture"
    ));
    expect(dismissCall).toBeGreaterThanOrEqual(0);
    expect(cancelCall).toBeGreaterThanOrEqual(0);
    expect(vi.mocked(invoke).mock.invocationCallOrder[dismissCall]).toBeLessThan(
      hideCurrentWindow.mock.invocationCallOrder[0],
    );
    expect(hideCurrentWindow.mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(invoke).mock.invocationCallOrder[cancelCall],
    );
  });

  it("cancels on Escape even when the session id has not loaded yet", async () => {
    activeSession = null;
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_active_session" || command === "get_pending_session") {
        return null;
      }
      if (
        command === "show_capture_overlay"
        || command === "reveal_capture_overlay"
        || command === "sync_capture_cursor"
        || command === "cancel_capture"
        || command === "cancel_active_capture"
        || command === "dismiss_capture_surface"
      ) {
        return undefined;
      }
      throw new Error(`unexpected command: ${command}`);
    });
    window.history.replaceState({}, "", "/?view=overlay&mode=region");
    render(<App />);
    await screen.findByText("Preparing capture…");

    fireEvent.keyDown(window, { code: "Escape" });

    expect(hideCurrentWindow).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledWith("cancel_active_capture");
  });

  it("cancels a live overlay with the Escape key code", async () => {
    activeSession = { ...session, frozen: false, snapshot_url: "" };
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1&frozen=0",
    );
    render(<App />);
    await screen.findByText("Drag to select a region");

    fireEvent.keyDown(window, { key: "Esc", code: "Escape" });

    expect(invoke).toHaveBeenCalledWith("cancel_capture", { sessionId: "capture-1" });
  });

  it("keeps a top-left region square and moves its dimensions on-screen", async () => {
    activeSession = { ...session, display_corner_radius: 40 };
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1",
    );
    const { container } = render(<App />);
    await screen.findByText("Drag to select a region");

    const surface = container.querySelector<HTMLElement>(".capture-surface");
    expect(surface).not.toBeNull();
    surface!.setPointerCapture = vi.fn();
    vi.spyOn(surface!, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 1440,
      bottom: 900,
      width: 1440,
      height: 900,
      toJSON: () => undefined,
    });

    fireEvent.pointerDown(surface!, { pointerId: 2, clientX: 0, clientY: 0 });
    fireEvent.pointerMove(surface!, { pointerId: 2, clientX: 320, clientY: 180 });

    await waitFor(() => {
      const selection = container.querySelector<HTMLElement>(".selection-box");
      expect(selection).toHaveStyle({
        left: "0px",
        top: "0px",
        width: "320px",
        height: "180px",
      });
      expect(surface!.style.borderRadius).toBe("");
      expect(selection!.style.borderRadius).toBe("");
      expect(selection!.querySelector(".selection-dimensions"))
        .toHaveAttribute("data-screen-edge", "top");
    });
  });

  it("keeps the region dim hole aligned with the marquee under Windows DPI scale", async () => {
    // Physical 1920×1080 @ 150% → logical overlay DIPs 1280×720. A mismatched
    // SVG viewBox used to scale the cutout away from the CSS marquee.
    activeSession = {
      ...session,
      display: {
        ...session.display,
        width: 1920,
        height: 1080,
        scale_factor: 1.5,
      },
      window_coordinate_scale: 1.5,
    };
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1",
    );
    const { container } = render(<App />);
    await screen.findByText("Drag to select a region");

    const surface = container.querySelector<HTMLElement>(".capture-surface");
    expect(surface).not.toBeNull();
    surface!.setPointerCapture = vi.fn();
    vi.spyOn(surface!, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 1280,
      bottom: 720,
      width: 1280,
      height: 720,
      toJSON: () => undefined,
    });

    fireEvent.pointerDown(surface!, { pointerId: 1, clientX: 200, clientY: 150 });
    fireEvent.pointerMove(surface!, { pointerId: 1, clientX: 513, clientY: 565 });

    await waitFor(() => {
      expect(container.querySelector(".selection-box")).toHaveStyle({
        left: "200px",
        top: "150px",
        width: "313px",
        height: "415px",
      });
      expect(container.querySelector(".capture-shade-path")).not.toBeInTheDocument();
      expect(container.querySelector(".capture-shade-full")).toHaveStyle({
        clipPath: "polygon(evenodd, 0% 0%, 100% 0%, 100% 100%, 0% 100%, 0% 0%, "
          + "200px 150px, 200px 565px, 513px 565px, 513px 150px, 200px 150px)",
      });
    });
  });

  it("wakes the overlay when a region session is ready without revealing yet", async () => {
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1",
    );
    render(<App />);
    await screen.findByText("Drag to select a region");

    expect(invoke).toHaveBeenCalledWith("show_capture_overlay", { sessionId: "capture-1" });
    expect(vi.mocked(invoke).mock.calls.filter(([command]) => (
      command === "reveal_capture_overlay"
    ))).toHaveLength(0);
  });

  it("reveals the overlay after the frozen snapshot paints", async () => {
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1",
    );
    const { container } = render(<App />);
    await screen.findByText("Drag to select a region");

    const snapshot = container.querySelector(".capture-snapshot");
    expect(snapshot).not.toBeNull();
    fireEvent.load(snapshot!);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("reveal_capture_overlay", { sessionId: "capture-1" });
    });
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("sync_capture_cursor", { sessionId: "capture-1" });
    });
    expect(vi.mocked(invoke).mock.calls.filter(([command]) => (
      command === "show_capture_overlay"
    ))).toHaveLength(1);
  });

  it("reveals a live overlay without a freeze-frame snapshot", async () => {
    activeSession = { ...session, frozen: false, snapshot_url: "" };
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1",
    );
    const { container } = render(<App />);
    await screen.findByText("Drag to select a region");

    expect(container.querySelector(".capture-snapshot")).toBeNull();
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("reveal_capture_overlay", { sessionId: "capture-1" });
    });
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("sync_capture_cursor", { sessionId: "capture-1" });
    });
  });

  it("applies the region cursor class as soon as the session is ready", async () => {
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1",
    );
    render(<App />);
    await screen.findByText("Drag to select a region");
    expect(document.documentElement).toHaveClass("capture-region-cursor");
  });

  it("applies the window cursor class for window capture", async () => {
    activeSession = { ...session, mode: "window" };
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=window&session_id=capture-1",
    );
    render(<App />);
    await screen.findByText("Select a window to continue");
    expect(document.documentElement).toHaveClass("capture-window-cursor");
  });

  it("hit-tests the frontmost window under the pointer instead of CSS hover", async () => {
    activeSession = {
      ...session,
      mode: "window",
      windows: [
        {
          id: "prefs",
          title: "Captures Preferences",
          app_name: "Captures",
          z_order: 30,
          x: 100,
          y: 80,
          width: 800,
          height: 600,
          display_id: "display-1",
          corner_radius: 12,
        },
        {
          id: "notes",
          title: "Notes",
          app_name: "Notes",
          z_order: 10,
          x: 300,
          y: 160,
          width: 900,
          height: 640,
          display_id: "display-1",
        },
      ],
    };
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=window&session_id=capture-1",
    );
    const { container } = render(<App />);
    await screen.findByText("Select a window to continue");

    const surface = container.querySelector<HTMLElement>(".capture-surface");
    expect(surface).not.toBeNull();
    vi.spyOn(surface!, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 1440,
      bottom: 900,
      width: 1440,
      height: 900,
      toJSON: () => undefined,
    } as DOMRect);

    fireEvent.pointerMove(surface!, { clientX: 150, clientY: 100 });
    const prefs = screen.getByTitle("Captures Preferences");
    const notes = screen.getByTitle("Notes");
    expect(prefs).toHaveClass("window-target-hovered");
    expect(notes).not.toHaveClass("window-target-hovered");

    fireEvent.pointerMove(surface!, { clientX: 950, clientY: 400 });
    expect(notes).toHaveClass("window-target-hovered");
    expect(prefs).not.toHaveClass("window-target-hovered");

    fireEvent.pointerUp(surface!, { clientX: 950, clientY: 400 });
    expect(invoke).toHaveBeenCalledWith("dismiss_capture_surface");
    expect(invoke).toHaveBeenCalledWith("commit_window", {
      sessionId: "capture-1",
      windowId: "notes",
    });
  });

  it("captures the display when window mode hits the menu bar or empty desktop", async () => {
    activeSession = {
      ...session,
      mode: "window",
      display_corner_radius: 12,
      windows: [
        {
          id: "notes",
          title: "Notes",
          app_name: "Notes",
          z_order: 10,
          x: 0,
          y: 0,
          width: 1440,
          height: 900,
          display_id: "display-1",
        },
      ],
      shell_chrome: [
        {
          id: "menubar",
          title: "",
          app_name: "Control Center",
          z_order: 50,
          x: 0,
          y: 0,
          width: 1440,
          height: 24,
          display_id: "display-1",
        },
      ],
    };
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=window&session_id=capture-1",
    );
    const { container } = render(<App />);
    await screen.findByText("Select a window to continue");

    const surface = container.querySelector<HTMLElement>(".capture-surface");
    expect(surface).not.toBeNull();
    vi.spyOn(surface!, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 1440,
      bottom: 900,
      width: 1440,
      height: 900,
      toJSON: () => undefined,
    } as DOMRect);

    fireEvent.pointerMove(surface!, { clientX: 20, clientY: 8 });
    expect(await screen.findByText("Click to capture this display")).toBeInTheDocument();
    expect(screen.getByText("Entire display")).toBeInTheDocument();
    expect(container.querySelector(".capture-display-outline")).not.toBeNull();
    expect(screen.getByTitle("Notes")).not.toHaveClass("window-target-hovered");

    fireEvent.pointerUp(surface!, { clientX: 20, clientY: 8 });
    expect(invoke).toHaveBeenCalledWith("dismiss_capture_surface");
    expect(invoke).toHaveBeenCalledWith("commit_display", { sessionId: "capture-1" });
    expect(invoke).not.toHaveBeenCalledWith("commit_window", expect.anything());
  });

  it("does not capture the display while window listing is still deferred", async () => {
    activeSession = {
      ...session,
      mode: "window",
      windows: [],
      shell_chrome: [],
      windows_ready: false,
    };
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=window&session_id=capture-1",
    );
    const { container } = render(<App />);
    await screen.findByText("Select a window to continue");

    const surface = container.querySelector<HTMLElement>(".capture-surface");
    expect(surface).not.toBeNull();
    vi.spyOn(surface!, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 1440,
      bottom: 900,
      width: 1440,
      height: 900,
      toJSON: () => undefined,
    } as DOMRect);

    fireEvent.pointerMove(surface!, { clientX: 20, clientY: 8 });
    expect(screen.queryByText("Click to capture this display")).not.toBeInTheDocument();
    fireEvent.pointerUp(surface!, { clientX: 20, clientY: 8 });
    expect(invoke).not.toHaveBeenCalledWith("commit_display", expect.anything());
    expect(invoke).not.toHaveBeenCalledWith("commit_window", expect.anything());
    expect(hideCurrentWindow).not.toHaveBeenCalled();
  });

  it("keeps a revealed window overlay visible when targets arrive later", async () => {
    let sessionReady: ((event: { payload: ActiveSession }) => void) | null = null;
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      if (event === "capture-session-ready") {
        sessionReady = handler as (event: { payload: ActiveSession }) => void;
      }
      return () => undefined;
    });
    activeSession = { ...session, mode: "window", windows: [], windows_ready: false };
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=window&session_id=capture-1",
    );
    const { container } = render(<App />);
    await screen.findByText("Select a window to continue");
    expect(container.querySelectorAll(".window-target")).toHaveLength(0);

    const snapshot = container.querySelector(".capture-snapshot");
    expect(snapshot).not.toBeNull();
    fireEvent.load(snapshot!);
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("reveal_capture_overlay", { sessionId: "capture-1" });
    });
    await waitFor(() => {
      expect(container.querySelector(".capture-surface")).toHaveClass("capture-visible");
    });
    const wakeCalls = vi.mocked(invoke).mock.calls.filter(([command]) => (
      command === "show_capture_overlay"
    )).length;

    await act(async () => {
      sessionReady?.({
        payload: {
          ...session,
          mode: "window",
          windows_ready: false,
          windows: [{
            id: "notes",
            title: "Notes",
            app_name: "Notes",
            z_order: 10,
            x: 300,
            y: 160,
            width: 900,
            height: 640,
            display_id: "display-1",
            corner_radius: 12,
          }],
        },
      });
    });

    expect(await screen.findByTitle("Notes")).toBeInTheDocument();
    expect(container.querySelector(".capture-surface")).toHaveClass("capture-visible");
    expect(vi.mocked(invoke).mock.calls.filter(([command]) => (
      command === "show_capture_overlay"
    ))).toHaveLength(wakeCalls);
  });

  it("releases a region click so a double-click cannot cover the display", async () => {
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1",
    );
    const { container } = render(<App />);
    await screen.findByText("Drag to select a region");

    const surface = container.querySelector<HTMLElement>(".capture-surface");
    expect(surface).not.toBeNull();
    surface!.setPointerCapture = vi.fn();
    surface!.hasPointerCapture = vi.fn(() => true);
    surface!.releasePointerCapture = vi.fn();
    vi.spyOn(surface!, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 1440,
      bottom: 900,
      width: 1440,
      height: 900,
      toJSON: () => undefined,
    } as DOMRect);

    fireEvent.pointerDown(surface!, { pointerId: 1, clientX: 240, clientY: 180 });
    fireEvent.pointerUp(surface!, { pointerId: 1, clientX: 240, clientY: 180 });
    fireEvent.pointerDown(surface!, { pointerId: 1, clientX: 240, clientY: 180 });
    fireEvent.pointerUp(surface!, { pointerId: 1, clientX: 240, clientY: 180 });
    fireEvent.doubleClick(surface!, { clientX: 240, clientY: 180 });

    expect(await screen.findByText("Click and drag to select a region")).toBeInTheDocument();
    expect(container.querySelector(".selection-box")).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("commit_region", expect.anything());

    fireEvent.pointerMove(surface!, { pointerId: 2, clientX: 1400, clientY: 860 });
    expect(container.querySelector(".selection-box")).not.toBeInTheDocument();
  });

  it("highlights the window under the pointer when window capture starts", async () => {
    capturePointer = { x: 150, y: 100, inside: true };
    activeSession = {
      ...session,
      mode: "window",
      windows: [
        {
          id: "prefs",
          title: "Captures Preferences",
          app_name: "Captures",
          z_order: 30,
          x: 100,
          y: 80,
          width: 800,
          height: 600,
          display_id: "display-1",
          corner_radius: 12,
        },
        {
          id: "notes",
          title: "Notes",
          app_name: "Notes",
          z_order: 10,
          x: 300,
          y: 160,
          width: 900,
          height: 640,
          display_id: "display-1",
        },
      ],
    };
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=window&session_id=capture-1",
    );
    render(<App />);
    await screen.findByText("Select a window to continue");

    const prefs = await screen.findByTitle("Captures Preferences");
    await waitFor(() => {
      expect(prefs).toHaveClass("window-target-hovered");
    });
    expect(screen.getByTitle("Notes")).not.toHaveClass("window-target-hovered");
  });
});
