import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { App } from "./App";
import { isPointerOverCaptureGuidance } from "./lib/captureGuidance";
import type { ActiveSession } from "./types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => false,
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(),
  listen: vi.fn(async () => () => undefined),
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

    fireEvent.pointerMove(window, { clientX: 620, clientY: 150 });
    await waitFor(() => {
      expect(guidance).toHaveAttribute("data-faded", "true");
    });

    fireEvent.pointerMove(window, { clientX: 20, clientY: 20 });
    await waitFor(() => {
      expect(guidance).not.toHaveAttribute("data-faded");
    });
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

    fireEvent.pointerMove(window, { clientX: 620, clientY: 150 });
    await waitFor(() => {
      expect(guidance).toHaveAttribute("data-faded", "true");
    });

    // Just outside the painted box but inside leave slack — stay faded.
    fireEvent.pointerMove(window, { clientX: 768, clientY: 150 });
    await waitFor(() => {
      expect(guidance).toHaveAttribute("data-faded", "true");
    });

    // Clear the slack zone — restore.
    fireEvent.pointerMove(window, { clientX: 800, clientY: 150 });
    await waitFor(() => {
      expect(guidance).not.toHaveAttribute("data-faded");
    });
  });

  it("ignores the painted border edge until the cursor is clearly inside", async () => {
    window.history.replaceState(
      {},
      "",
      "/?view=overlay&mode=region&session_id=capture-1",
    );
    mockGuidanceBounds();
    render(<App />);

    const guidance = (await screen.findByText("Drag to select a region"))
      .closest(".capture-guidance") as HTMLElement;

    // On the exact left edge — enter inset keeps it visible.
    fireEvent.pointerMove(window, { clientX: 500, clientY: 150 });
    await waitFor(() => {
      expect(guidance).not.toHaveAttribute("data-faded");
    });

    // Past the 4px enter inset — fade out of the way.
    fireEvent.pointerMove(window, { clientX: 510, clientY: 150 });
    await waitFor(() => {
      expect(guidance).toHaveAttribute("data-faded", "true");
    });
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

    fireEvent.pointerMove(window, { clientX: 620, clientY: 150 });
    await waitFor(() => {
      expect(guidance).toHaveAttribute("data-faded", "true");
    });
  });

  it("uses enter/leave hysteresis for guidance hit testing", () => {
    const bounds = { left: 500, right: 760, top: 120, bottom: 180 };

    // Enter inset is 4px; edge stays visible, past inset fades.
    expect(isPointerOverCaptureGuidance(500, 150, bounds, false)).toBe(false);
    expect(isPointerOverCaptureGuidance(503, 150, bounds, false)).toBe(false);
    expect(isPointerOverCaptureGuidance(504, 150, bounds, false)).toBe(true);
    expect(isPointerOverCaptureGuidance(510, 150, bounds, false)).toBe(true);
    // Leave slack is 20px beyond the painted box while already faded.
    expect(isPointerOverCaptureGuidance(768, 150, bounds, true)).toBe(true);
    expect(isPointerOverCaptureGuidance(800, 150, bounds, true)).toBe(false);
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
});
