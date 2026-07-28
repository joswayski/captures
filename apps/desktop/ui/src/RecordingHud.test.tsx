import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { RecordingHud } from "./App";
import type { RecordingSessionSnapshot } from "./types";

const { startDragging } = vi.hoisted(() => ({
  startDragging: vi.fn(async () => undefined),
}));
const { nativeMessage } = vi.hoisted(() => ({
  nativeMessage: vi.fn(async () => "Cancel"),
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

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
  message: nativeMessage,
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
      if (command === "set_recording_hud_collapsed") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("keeps status state current even when transient event listeners stall", async () => {
    vi.mocked(listen).mockImplementation(() => new Promise(() => undefined));
    render(<RecordingHud />);

    expect(await screen.findByText("0:00")).toBeInTheDocument();
    expect(screen.getByText("Starting…")).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("get_recording_snapshot");
  });

  it("offers a screenshot action, a compact capture-visibility tooltip, and a drag handle", async () => {
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

    expect(screen.getByText("Recording")).toBeInTheDocument();
    expect(screen.queryByText("Not in recording")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Controls are not included in the recording")).toBeInTheDocument();
    expect(screen.getAllByRole("tooltip").map((tooltip) => tooltip.textContent)).toEqual(expect.arrayContaining([
      "Stop and save",
      "Pause recording",
      "Restart recording",
      "Take a region screenshot",
      "Delete recording",
      "Controls are not included in the recording",
      "Drag to move controls",
      "Minimize controls",
    ]));
    fireEvent.pointerDown(screen.getByRole("button", { name: "Move recording controls" }), {
      button: 0,
    });
    expect(startDragging).toHaveBeenCalledOnce();
  });

  it("collapses to the recording status and can expand again", async () => {
    snapshot = {
      ...baseSnapshot,
      state: "recording",
      elapsed_ms: 4_000,
      countdown_remaining_seconds: null,
    };
    const { container } = render(<RecordingHud />);

    fireEvent.click(await screen.findByRole("button", { name: "Minimize recording controls" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_recording_hud_collapsed", {
        sessionId: snapshot.id,
        collapsed: true,
      });
      expect(container.querySelector(".recording-hud-collapsed")).toBeInTheDocument();
    });
    expect(screen.queryByRole("button", { name: "Stop recording" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Move recording controls" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Expand recording controls" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_recording_hud_collapsed", {
        sessionId: snapshot.id,
        collapsed: false,
      });
      expect(screen.getByRole("button", { name: "Stop recording" })).toBeInTheDocument();
    });
  });

  it("uses a native Delete recording dialog before discarding", async () => {
    snapshot = {
      ...baseSnapshot,
      state: "recording",
      countdown_remaining_seconds: null,
    };
    nativeMessage.mockResolvedValueOnce("Delete");
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_recording_snapshot") return snapshot;
      if (command === "discard_recording") return { ...snapshot, state: "discarded" };
      throw new Error(`unexpected command: ${command}`);
    });

    render(<RecordingHud />);
    fireEvent.click(await screen.findByRole("button", { name: "Delete recording" }));

    await waitFor(() => {
      expect(nativeMessage).toHaveBeenCalledWith(
        "This recording will be deleted permanently.",
        expect.objectContaining({
          title: "Delete recording?",
          buttons: { ok: "Delete", cancel: "Cancel" },
        }),
      );
      expect(invoke).toHaveBeenCalledWith("discard_recording", {
        sessionId: snapshot.id,
      });
    });
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
