import { fireEvent, render, screen, waitFor } from "@testing-library/react";
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
  let activeSession: ActiveSession;

  beforeEach(() => {
    activeSession = session;
    vi.mocked(listen).mockResolvedValue(() => undefined);
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_active_session" || command === "get_pending_session") {
        return activeSession;
      }
      if (
        command === "show_capture_overlay"
        || command === "reveal_capture_overlay"
        || command === "sync_capture_cursor"
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
    expect(invoke).toHaveBeenCalledWith("commit_region", {
      sessionId: "capture-1",
      rect: { x: 120, y: 80, width: 500, height: 400 },
    });
    const commitCall = vi.mocked(invoke).mock.calls.findIndex(([command]) => (
      command === "commit_region"
    ));
    expect(commitCall).toBeGreaterThanOrEqual(0);
    expect(hideCurrentWindow.mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(invoke).mock.invocationCallOrder[commitCall],
    );
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
});
