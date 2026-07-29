import { render, screen } from "@testing-library/react";
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
});
