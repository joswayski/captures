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
  windows: [
    {
      id: "front-window",
      title: "Captures window",
      app_name: "Captures",
      x: 100,
      y: 80,
      width: 800,
      height: 600,
      display_id: "display-1",
    },
    {
      id: "back-window",
      title: "Front eligible window",
      app_name: "Browser",
      x: 300,
      y: 160,
      width: 900,
      height: 640,
      display_id: "display-1",
    },
    {
      id: "rear-window",
      title: "Rear window",
      app_name: "Notes",
      x: 420,
      y: 220,
      width: 720,
      height: 520,
      display_id: "display-1",
    },
  ],
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
        return [
          { id: "default", name: "Default — Studio Microphone", kind: "default", is_default: true },
          { id: "microphone-2", name: "USB Microphone", kind: "microphone", is_default: false },
        ];
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
    fireEvent.click(screen.getByRole("combobox", { name: "Microphone" }));
    expect(await screen.findByRole("option", { name: "Default — Studio Microphone" })).toBeInTheDocument();
    expect(screen.queryByText("Studio Microphone")).not.toBeInTheDocument();

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

    const cursorToggle = screen.getByRole("checkbox", { name: "Show cursor" });
    expect(cursorToggle.nextElementSibling).toHaveClass("recording-switch");
    expect(container.querySelectorAll(".recording-options-row > .recording-field")).toHaveLength(6);

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

  it("selects the frontmost eligible window and previews hovered windows with rounded geometry", async () => {
    const { container } = render(<RecordingSelector />);

    fireEvent.click(await screen.findByRole("button", { name: "Window" }));

    expect(screen.getByText(/Window selected/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Select Captures window" })).not.toBeInTheDocument();
    const selected = screen.getByRole("button", { name: "Select Front eligible window" });
    expect(selected).toHaveClass("selected");
    expect(selected).toHaveStyle({
      left: "300px",
      top: "160px",
      width: "900px",
      height: "640px",
    });
    expect(container.querySelector(".recording-selection-window")).not.toBeInTheDocument();
    expect(container.querySelector(".capture-shade-path")).toHaveAttribute(
      "d",
      expect.stringContaining("M300 160H1200V800H300Z"),
    );

    fireEvent.mouseEnter(screen.getByRole("button", { name: "Select Rear window" }));
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Select Rear window" })).toHaveClass("hovered");
      expect(container.querySelector(".capture-shade-path")).toHaveAttribute(
        "d",
        expect.stringContaining("M420 220H1140V740H420Z"),
      );
    });
  });

  it("keeps the selector surface fixed while the controls are dragged within the display", async () => {
    const { container } = render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Record" });
    const panel = container.querySelector<HTMLElement>(".recording-selector-panel");
    expect(panel).not.toBeNull();
    expect(screen.queryByRole("button", { name: "Move recording controls" })).not.toBeInTheDocument();
    vi.spyOn(panel!, "getBoundingClientRect").mockReturnValue({
      x: 200,
      y: 700,
      top: 700,
      left: 200,
      right: 1_200,
      bottom: 850,
      width: 1_000,
      height: 150,
      toJSON: () => undefined,
    });
    Object.defineProperties(panel!, {
      offsetWidth: { configurable: true, value: 1_000 },
      offsetHeight: { configurable: true, value: 150 },
    });
    panel!.setPointerCapture = vi.fn();
    panel!.hasPointerCapture = vi.fn(() => true);
    panel!.releasePointerCapture = vi.fn();

    fireEvent.pointerDown(panel!, { pointerId: 4, clientX: 620, clientY: 760 });
    expect(panel).toHaveClass("dragging");
    fireEvent.pointerMove(panel!, { pointerId: 4, clientX: 100, clientY: 300 });
    fireEvent.pointerUp(panel!, { pointerId: 4 });

    expect(panel).toHaveStyle({
      left: "8px",
      top: "240px",
      bottom: "auto",
      transform: "none",
    });
    expect(panel).not.toHaveClass("dragging");
    expect(container.querySelector(".recording-selector")).not.toHaveAttribute("style");
  });

  it("lets one Escape close an open control and cancel the selector exactly once", async () => {
    render(<RecordingSelector />);
    const microphone = await screen.findByRole("combobox", { name: "Microphone" });
    fireEvent.click(microphone);
    expect(microphone).toHaveAttribute("aria-expanded", "true");

    fireEvent.keyDown(microphone, { key: "Escape" });
    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.keyDown(window, { key: "Escape" });

    expect(screen.queryByRole("button", { name: "Record" })).not.toBeInTheDocument();
    expect(vi.mocked(invoke).mock.calls.filter(([command]) => (
      command === "cancel_recording_selection"
    ))).toHaveLength(1);
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
    for (let offset = 0; offset <= 300; offset += 15) {
      fireEvent.pointerMove(surface!, {
        pointerId: 1,
        clientX: 100 + offset,
        clientY: 120 + Math.round(offset * 0.73),
      });
    }
    fireEvent.pointerMove(surface!, { pointerId: 1, clientX: 400, clientY: 340 });
    fireEvent.pointerUp(surface!, { pointerId: 1 });

    await waitFor(() => {
      expect(container.querySelector(".recording-selection-frame")).toHaveStyle({
        left: "100px",
        top: "120px",
        width: "300px",
        height: "220px",
      });
      expect(container.querySelectorAll(".capture-shade-path")).toHaveLength(1);
      expect(container.querySelector(".capture-shade-path")).toHaveAttribute(
        "d",
        expect.stringContaining("M100 120H400V340H100Z"),
      );
    });
  });

  it("does not discard a prepared selection after a transient reveal failure", async () => {
    selectorShowError = new Error("macOS could not focus the selector");

    render(<RecordingSelector />);

    expect(await screen.findByRole("alert")).toHaveTextContent("macOS could not focus the selector");
    expect(invoke).not.toHaveBeenCalledWith("cancel_recording_selection", expect.anything());
  });
});
