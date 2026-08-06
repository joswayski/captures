import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { App } from "./App";
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
    expect(guidance).toHaveTextContent("Esc to cancel");
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
          + "200px 150px, 200px 565px, 513px 565px, 513px 150px)",
      });
    });
  });
});
