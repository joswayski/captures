import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { RecordingHud } from "./App";
import type { RecordingSessionSnapshot } from "./types";

const { startDragging } = vi.hoisted(() => ({
  startDragging: vi.fn(async () => undefined),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => true,
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(),
  listen: vi.fn(),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ startDragging }),
}));

const baseSnapshot: RecordingSessionSnapshot = {
  id: "recording-1",
  state: "countdown",
  options: {
    kind: "video",
    target: {
      type: "region",
      display_id: "display-1",
      rect: { x: 10, y: 20, width: 800, height: 600 },
    },
    frames_per_second: 60,
    max_resolution: "original",
    countdown_seconds: 3,
    show_cursor: true,
    highlight_clicks: false,
    show_keystrokes: false,
    audio: {
      capture_system_audio: false,
      microphone_device_id: null,
      mono_output: false,
      system_volume_percent: 100,
      microphone_volume_percent: 100,
      microphone_muted: false,
    },
    gif: {
      max_width: 800,
      max_colors: 256,
      optimize: true,
    },
  },
  elapsed_ms: 0,
  countdown_remaining_seconds: 2,
  warning: null,
  error: null,
};

describe("RecordingHud", () => {
  let snapshot: RecordingSessionSnapshot;

  beforeEach(() => {
    snapshot = baseSnapshot;
    vi.mocked(listen).mockRejectedValue(new Error("event bridge unavailable"));
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_recording_snapshot") return snapshot;
      if (command === "start_capture") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("keeps countdown state current even when transient event listeners stall", async () => {
    vi.mocked(listen).mockImplementation(() => new Promise(() => undefined));
    render(<RecordingHud />);

    expect(await screen.findByText("2")).toBeInTheDocument();
    expect(screen.getByText("Starting…")).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("get_recording_snapshot");
  });

  it("offers a screenshot action, a capture-visibility explanation, and a drag handle", async () => {
    snapshot = {
      ...baseSnapshot,
      state: "recording",
      elapsed_ms: 4_000,
      countdown_remaining_seconds: null,
    };
    render(<RecordingHud />);

    const screenshot = await screen.findByRole("button", { name: "Take a region screenshot" });
    fireEvent.click(screenshot);
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_capture", { mode: "region" });
    });

    expect(screen.getByLabelText("Controls are hidden from captures")).toBeInTheDocument();
    fireEvent.pointerDown(screen.getByRole("button", { name: "Move recording controls" }), {
      button: 0,
    });
    expect(startDragging).toHaveBeenCalledOnce();
  });

  it("does not present a failed recording as actively recording", async () => {
    snapshot = {
      ...baseSnapshot,
      state: "failed",
      countdown_remaining_seconds: null,
      error: "ScreenCaptureKit did not deliver a usable video frame",
    };
    const { container } = render(<RecordingHud />);

    expect(await screen.findByText("Failed")).toBeInTheDocument();
    expect(container.querySelector(".recording-hud-failed")).toBeInTheDocument();
  });
});
