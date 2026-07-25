import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { RecordingSelector } from "./App";
import type { AppSettings, RecordingSelectionSession } from "./types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => false,
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(),
  listen: vi.fn(async () => () => undefined),
}));

const settings: AppSettings = {
  output_directory: "/Users/josevalerio/Captures",
  region_shortcut: "Ctrl+Shift+4",
  window_shortcut: "Ctrl+Shift+W",
  display_shortcut: "Ctrl+Shift+3",
  auto_copy_to_clipboard: true,
  launch_at_login: false,
  last_screen_permission_request_id: null,
  pending_capture_after_restart: null,
  recording: {
    video_shortcut: "Ctrl+Shift+5",
    gif_shortcut: "Ctrl+Shift+6",
    video_fps: 60,
    video_max_resolution: "original",
    gif_fps: 15,
    gif_max_width: 800,
    gif_max_colors: 256,
    countdown_seconds: 3,
    show_cursor: true,
    capture_system_audio: false,
    microphone_device_id: null,
    mono_audio: false,
    highlight_clicks: false,
    show_keystrokes: false,
    open_editor_after_recording: true,
  },
};

const session: RecordingSelectionSession = {
  id: "selection-1",
  kind: "video",
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
  snapshot_url: "capture://recording-selection/selection-1",
  windows: [],
};

describe("RecordingSelector", () => {
  let selectorShowError: Error | null;
  let recordingSelectionReady:
    | ((event: { payload: RecordingSelectionSession }) => void)
    | null;

  beforeEach(() => {
    selectorShowError = null;
    recordingSelectionReady = null;
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      if (event === "recording-selection-ready") {
        recordingSelectionReady = handler as (
          event: { payload: RecordingSelectionSession },
        ) => void;
      }
      return () => undefined;
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_recording_selection") return session;
      if (command === "get_settings") return settings;
      if (command === "list_recording_audio_devices") {
        return [{ id: "microphone-1", name: "Studio Microphone", is_default: true }];
      }
      if (command === "show_recording_selector") {
        if (selectorShowError) throw selectorShowError;
        return undefined;
      }
      if (
        command === "reveal_recording_selector"
        || command === "start_recording"
        || command === "cancel_recording_selection"
      ) {
        return undefined;
      }
      throw new Error(`unexpected command: ${command}`);
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it("reveals after the snapshot paints and preloads microphones before first use", async () => {
    const { container } = render(<RecordingSelector />);

    expect(screen.queryByText("Preparing recorder…")).not.toBeInTheDocument();
    const snapshot = await waitFor(() => {
      const image = container.querySelector<HTMLImageElement>(".recording-selector-snapshot");
      expect(image).not.toBeNull();
      return image!;
    });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("list_recording_audio_devices"));
    expect(await screen.findByRole("option", { name: "Studio Microphone" })).toBeInTheDocument();

    fireEvent.load(snapshot);
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("show_recording_selector", {
        selectionId: session.id,
      });
    });
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("reveal_recording_selector", {
        selectionId: session.id,
      });
    });

    const cursorToggle = screen.getByRole("checkbox", { name: "Cursor" });
    expect(cursorToggle.nextElementSibling).toHaveClass("recording-switch");

    fireEvent.click(screen.getByRole("button", { name: "Record" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_recording", {
        request: expect.objectContaining({
          selection_id: session.id,
          options: expect.objectContaining({
            kind: "video",
            frames_per_second: 60,
            max_resolution: "original",
          }),
        }),
      });
    });
  });

  it("reveals through the safety path when a hidden WebView defers image loading", async () => {
    render(<RecordingSelector />);

    expect(await screen.findByRole("button", { name: "Record" })).toBeInTheDocument();
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("show_recording_selector", {
        selectionId: session.id,
      });
    });
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("reveal_recording_selector", {
        selectionId: session.id,
      });
    });
  });

  it("reveals through a deadline when WebKit suspends animation frames", async () => {
    vi.useFakeTimers();
    const animationFrame = vi
      .spyOn(window, "requestAnimationFrame")
      .mockImplementation(() => 1);
    const { container } = render(<RecordingSelector />);

    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    const snapshot = container.querySelector<HTMLImageElement>(".recording-selector-snapshot");
    expect(snapshot).not.toBeNull();

    await act(async () => {
      fireEvent.load(snapshot!);
      await Promise.resolve();
    });

    expect(invoke).toHaveBeenCalledWith("show_recording_selector", {
      selectionId: session.id,
    });
    expect(invoke).not.toHaveBeenCalledWith("reveal_recording_selector", expect.anything());

    await act(async () => {
      vi.advanceTimersByTime(200);
      await Promise.resolve();
    });

    expect(invoke).toHaveBeenCalledWith("reveal_recording_selector", {
      selectionId: session.id,
    });
    animationFrame.mockRestore();
  });

  it("loads the prepared selection while event registration is still pending", async () => {
    vi.mocked(listen).mockImplementationOnce(() => new Promise(() => undefined));

    render(<RecordingSelector />);

    expect(await screen.findByRole("button", { name: "Record" })).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("get_recording_selection");
    expect(invoke).toHaveBeenCalledWith("get_settings");
  });

  it("clears an escaped selector and makes the next region session interactive", async () => {
    const nextSession: RecordingSelectionSession = {
      ...session,
      id: "selection-2",
      snapshot_url: "capture://recording-selection/selection-2",
    };
    const { container } = render(<RecordingSelector />);

    expect(await screen.findByRole("button", { name: "Record" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });

    expect(screen.queryByRole("button", { name: "Record" })).not.toBeInTheDocument();
    expect(container.querySelector(".recording-selector-idle")).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("cancel_recording_selection", {
      selectionId: session.id,
    });

    await act(async () => {
      recordingSelectionReady?.({ payload: nextSession });
    });

    expect(await screen.findByRole("button", { name: "Record" })).toBeInTheDocument();
    const surface = container.querySelector<HTMLElement>(".recording-selector");
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

    fireEvent.pointerDown(surface!, { pointerId: 1, clientX: 100, clientY: 120 });
    fireEvent.pointerMove(surface!, { pointerId: 1, clientX: 400, clientY: 340 });
    fireEvent.pointerUp(surface!, { pointerId: 1 });

    await waitFor(() => {
      expect(container.querySelector(".recording-selection-frame")).toHaveStyle({
        left: "100px",
        top: "120px",
        width: "300px",
        height: "220px",
      });
    });
  });

  it("does not discard a prepared selection after a transient reveal failure", async () => {
    selectorShowError = new Error("macOS could not focus the selector");

    render(<RecordingSelector />);

    expect(await screen.findByRole("alert")).toHaveTextContent("macOS could not focus the selector");
    expect(invoke).not.toHaveBeenCalledWith("cancel_recording_selection", expect.anything());
  });
});
