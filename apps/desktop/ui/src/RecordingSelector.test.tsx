import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { RecordingSelector } from "./App";
import type { AppSettings, AudioDevice, RecordingSelectionSession } from "./types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => false,
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(),
  listen: vi.fn(async () => () => undefined),
}));

const settings: AppSettings = {
  appearance: "system",
  theme: "mustard",
  custom_theme: {
    accent: "#ffca28",
    signal: "#ef4650",
  },
  output_directory: "/Users/josevalerio/Captures",
  new_capture_shortcut: "Ctrl+Shift+Space",
  region_shortcut: "Ctrl+Shift+4",
  window_shortcut: "Ctrl+Shift+W",
  display_shortcut: "Ctrl+Shift+3",
  auto_copy_to_clipboard: true,
  auto_start_on_selection: false,
  show_mini_previews: true,
  mini_preview_placement: "bottom_left",
  include_mini_previews_in_captures: false,
  include_recording_controls_in_captures: false,
  launch_at_login: false,
  last_screen_permission_request_id: null,
  pending_capture_after_restart: null,
  onboarding_completed: true,
  screenshot_countdown_seconds: 0,
  freeze_screen: true,
  show_cursor_in_screenshots: true,
  screenshot_format: "png",
  show_update_changelog: true,
  recording: {
    video_shortcut: "Ctrl+Shift+5",
    gif_shortcut: "Ctrl+Shift+6",
    video_format: "mp4",
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
  initial_mode: "recording",
  initial_target: "region",
  recording_available: true,
  recording_capabilities: {
    system_audio: true,
    microphone: true,
    cursor_control: true,
    click_highlights: true,
    controls_excluded: true,
  },
  display: {
    id: "display-1",
    name: "Built-in Retina Display",
    x: 0,
    y: 0,
    width: 1440,
    height: 900,
    scale_factor: 2,
    is_primary: true,
  },
  displays: [
    {
      id: "display-1",
      name: "Built-in Retina Display",
      x: 0,
      y: 0,
      width: 1440,
      height: 900,
      scale_factor: 2,
      is_primary: true,
    },
    {
      id: "display-2",
      name: "Studio Display",
      x: 1440,
      y: 0,
      width: 2560,
      height: 1440,
      scale_factor: 2,
      is_primary: false,
    },
  ],
  window_coordinate_scale: 1,
  window_corner_radius: 25,
  frozen: true,
  snapshot_url: "capture://recording-selection/selection-1",
  windows: [
    {
      id: "front-window",
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
      id: "rear-window",
      title: "Rear window",
      app_name: "Notes",
      z_order: 10,
      x: 420,
      y: 220,
      width: 720,
      height: 520,
      display_id: "display-1",
      // Falls back to session.window_corner_radius (25).
    },
    {
      id: "back-window",
      title: "Front eligible window",
      app_name: "Browser",
      z_order: 20,
      x: 300,
      y: 160,
      width: 900,
      height: 640,
      display_id: "display-1",
      corner_radius: 25,
    },
  ],
};

const selectorSurfaceRect = {
  x: 0,
  y: 0,
  top: 0,
  left: 0,
  right: 1440,
  bottom: 900,
  width: 1440,
  height: 900,
  toJSON: () => undefined,
} as DOMRect;

function mockSelectorSurface(container: HTMLElement) {
  const surface = container.querySelector<HTMLElement>(".recording-selector");
  expect(surface).not.toBeNull();
  vi.spyOn(surface!, "getBoundingClientRect").mockReturnValue(selectorSurfaceRect);
  return surface!;
}

function mockSelectorSurfaceForDrag(container: HTMLElement) {
  const surface = mockSelectorSurface(container);
  surface.setPointerCapture = vi.fn();
  surface.hasPointerCapture = vi.fn(() => true);
  surface.releasePointerCapture = vi.fn();
  return surface;
}

/** WebKit does not activate `display: none` buttons via `HTMLElement.click()`. */
function ignoreClicksOnHiddenButtons() {
  const nativeClick = HTMLButtonElement.prototype.click;
  return vi.spyOn(HTMLButtonElement.prototype, "click").mockImplementation(
    function mockHiddenSafeClick(this: HTMLButtonElement) {
      if (this.hidden) return;
      nativeClick.call(this);
    },
  );
}

async function enableAutoStartPreference() {
  const defaultInvoke = vi.mocked(invoke).getMockImplementation();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "get_settings") {
      return { ...settings, auto_start_on_selection: true };
    }
    return defaultInvoke?.(command, args);
  });
}

describe("RecordingSelector", () => {
  let selectorShowError: Error | null;
  let preparedSession: RecordingSelectionSession;
  let audioDevices: AudioDevice[];
  let recordingSelectionReady:
    | ((event: { payload: RecordingSelectionSession }) => void)
    | null;
  let capturePointer: { x: number; y: number; inside: boolean } | null;

  beforeEach(() => {
    selectorShowError = null;
    preparedSession = session;
    capturePointer = null;
    audioDevices = [
      { id: "default", name: "Default — Studio Microphone", kind: "default", is_default: true },
      { id: "microphone-2", name: "USB Microphone", kind: "microphone", is_default: false },
    ];
    recordingSelectionReady = null;
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      if (event === "recording-selection-ready") {
        recordingSelectionReady = handler as (
          event: { payload: RecordingSelectionSession },
        ) => void;
      }
      return () => undefined;
    });
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_recording_selection") return preparedSession;
      if (command === "get_settings") return settings;
      if (command === "list_recording_audio_devices") {
        return audioDevices;
      }
      if (command === "show_recording_selector") {
        if (selectorShowError) throw selectorShowError;
        return undefined;
      }
      if (command === "select_capture_display") {
        const displayId = (args as { displayId: string }).displayId;
        const display = preparedSession.displays.find((candidate) => candidate.id === displayId);
        if (!display) throw new Error("display is unavailable");
        return {
          ...preparedSession,
          display,
          snapshot_url: `${preparedSession.snapshot_url}?display=${display.id}`,
          windows: [],
        };
      }
      if (
        command === "reveal_recording_selector"
        || command === "sync_selector_cursor"
        || command === "capture_selection_screenshot"
        || command === "start_recording"
        || command === "start_capture"
        || command === "cancel_recording_selection"
        || command === "open_preferences"
      ) {
        return undefined;
      }
      if (command === "recording_controls_are_excluded") {
        return preparedSession.recording_capabilities.controls_excluded;
      }
      if (command === "get_capture_pointer_position") {
        return capturePointer;
      }
      throw new Error(`unexpected command: ${command}`);
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
    document.documentElement.classList.remove(
      "capture-selector-region",
      "capture-selector-window",
      "capture-selector-display",
    );
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
    const surface = container.querySelector(".recording-selector");
    await waitFor(() => expect(surface).toHaveClass("recording-focus-visible"));

    await act(async () => {
      recordingSelectionReady?.({ payload: session });
    });
    await waitFor(() => expect(surface).toHaveClass("recording-focus-visible"));

    const cursorToggle = screen.getByRole("checkbox", { name: "Show cursor" });
    const clicksToggle = screen.getByRole("checkbox", { name: "Show clicks" });
    expect(cursorToggle.nextElementSibling).toHaveClass("recording-switch");
    expect(container.querySelectorAll(".recording-options-row > .recording-field")).toHaveLength(6);
    fireEvent.click(cursorToggle);
    expect(cursorToggle).not.toBeChecked();
    expect(clicksToggle).not.toBeChecked();
    fireEvent.click(clicksToggle);
    expect(clicksToggle).toBeChecked();
    expect(cursorToggle).toBeChecked();

    // Region starts empty — use full screen so the primary action can run.
    fireEvent.click(screen.getByRole("button", { name: "Full screen" }));
    fireEvent.click(screen.getByRole("button", { name: "Start recording" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_recording", {
        request: expect.objectContaining({
          selection_id: session.id,
          options: expect.objectContaining({
            kind: "video",
            frames_per_second: 60,
            max_resolution: "original",
            show_cursor: true,
            highlight_clicks: true,
          }),
        }),
      });
    });
  });

  it("applies a capture cursor for the current target without waiting for mouse movement", async () => {
    const { container } = render(<RecordingSelector />);
    const snapshot = await waitFor(() => {
      const image = container.querySelector<HTMLImageElement>(".recording-selector-snapshot");
      expect(image).not.toBeNull();
      return image!;
    });
    await waitFor(() => {
      expect(document.documentElement).toHaveClass("capture-selector-region");
    });
    fireEvent.load(snapshot);
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("reveal_recording_selector", {
        selectionId: session.id,
      });
    });
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("sync_selector_cursor", {
        selectionId: session.id,
        mode: "region",
      });
    });

    fireEvent.click(screen.getByRole("button", { name: "Window" }));
    await waitFor(() => {
      expect(document.documentElement).toHaveClass("capture-selector-window");
    });
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("sync_selector_cursor", {
        selectionId: session.id,
        mode: "window",
      });
    });

    fireEvent.click(screen.getByRole("button", { name: "Full screen" }));
    await waitFor(() => {
      expect(document.documentElement).toHaveClass("capture-selector-display");
    });
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("sync_selector_cursor", {
        selectionId: session.id,
        mode: "display",
      });
    });
  });

  it("disables unsupported recording options without sending stale settings", async () => {
    preparedSession = {
      ...session,
      recording_capabilities: {
        system_audio: false,
        microphone: false,
        cursor_control: false,
        click_highlights: false,
        controls_excluded: false,
      },
    };
    const { container } = render(<RecordingSelector />);

    const cursorToggle = await screen.findByRole("checkbox", { name: "Show cursor" });
    const clicksToggle = screen.getByRole("checkbox", { name: "Show clicks" });
    const audioToggle = screen.getByRole("checkbox", { name: "Record desktop audio" });
    const microphone = screen.getByRole("combobox", { name: "Microphone" });
    expect(cursorToggle).toBeDisabled();
    expect(clicksToggle).toBeDisabled();
    expect(audioToggle).toBeDisabled();
    expect(microphone).toBeDisabled();
    expect(container.querySelector(".capture-selector-note")).toHaveTextContent(
      "These controls will show in recordings",
    );
    expect(invoke).not.toHaveBeenCalledWith("list_recording_audio_devices");

    fireEvent.click(screen.getByRole("button", { name: "Full screen" }));
    fireEvent.click(screen.getByRole("button", { name: "Start recording" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_recording", {
        request: expect.objectContaining({
          options: expect.objectContaining({
            show_cursor: false,
            highlight_clicks: false,
            audio: expect.objectContaining({
              capture_system_audio: false,
              microphone_device_id: null,
            }),
          }),
        }),
      });
    });
  });

  it("switches between screenshot and recording in one selector and captures the chosen target", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
    };
    const { container } = render(<RecordingSelector />);

    const screenshotMode = await screen.findByRole("button", {
      name: "Screenshot",
      pressed: true,
    });
    const actionSwitch = screenshotMode.closest(".capture-action-switch");
    const targetSwitch = screen.getByRole("button", { name: "Region" })
      .closest(".recording-target-switch");
    expect(actionSwitch).toHaveAttribute("data-active", "screenshot");
    expect(actionSwitch?.querySelector(".capture-segmented-indicator")).not.toBeNull();
    expect(screenshotMode.querySelector(".capture-icon-spark")).not.toBeNull();
    expect(targetSwitch).toHaveAttribute("data-active", "region");
    expect(targetSwitch?.querySelector(".capture-segmented-indicator")).not.toBeNull();
    const regionGuidance = screen.getByText("Drag to select a region").closest(".capture-guidance");
    expect(regionGuidance).toHaveTextContent("Shift for square · Esc to cancel");
    const aspectPicker = screen.getByRole("combobox", { name: "Region aspect ratio" });
    expect(aspectPicker).toBeInTheDocument();
    expect(aspectPicker.closest(".recording-region-aspect-picker"))
      .toHaveTextContent(/Aspect/);
    expect(container.querySelector(".capture-selector-note")).toHaveTextContent(
      "These controls won’t show in screenshots · Press Enter to confirm",
    );
    expect(screen.getByRole("button", { name: "Take screenshot" }))
      .toHaveAttribute("aria-keyshortcuts", "Enter");
    expect(screen.queryByRole("combobox", { name: "Frames per second" })).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("list_recording_audio_devices");

    fireEvent.click(screen.getByRole("button", { name: "Window" }));
    expect(targetSwitch).toHaveAttribute("data-active", "window");
    expect(screen.queryByRole("combobox", { name: "Region aspect ratio" })).not.toBeInTheDocument();
    const windowGuidance = screen.getByText("Select a window to continue").closest(".capture-guidance");
    expect(windowGuidance).toHaveTextContent("Esc to cancel");
    const surface = mockSelectorSurface(container);
    // (950, 400) is inside Front eligible window and outside Preferences.
    fireEvent.pointerDown(surface, { pointerId: 1, clientX: 950, clientY: 400 });
    expect(screen.queryByText("Select a window to continue")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Record", pressed: false }));
    expect(actionSwitch).toHaveAttribute("data-active", "recording");
    expect(screen.getByRole("button", { name: "Record", pressed: true })).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Frames per second" })).toBeInTheDocument();
    expect(container.querySelector(".capture-selector-note")).toHaveTextContent(
      "These controls won’t show in recordings · Press Enter to confirm",
    );
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("list_recording_audio_devices");
    });

    fireEvent.click(screenshotMode);
    expect(container.querySelector(".capture-selector-note")).toHaveTextContent(
      "These controls won’t show in screenshots · Press Enter to confirm",
    );
    expect(screen.getByRole("button", { name: "Take screenshot" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Take screenshot" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("capture_selection_screenshot", {
        request: {
          selection_id: preparedSession.id,
          target: { type: "window", window_id: "back-window" },
        },
      });
    });
  });

  it("keeps the selector rectangular while rounding the full-display outline", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
      initial_target: "display",
      display_corner_radius: 40,
    };
    const { container } = render(<RecordingSelector />);

    expect(await screen.findByRole("button", { name: "Full screen", pressed: true }))
      .toBeInTheDocument();
    expect((container.querySelector<HTMLElement>(".recording-selector"))?.style.borderRadius)
      .toBe("");
    expect(container.querySelector(".recording-display-outline")).toHaveStyle({
      borderRadius: "40px",
    });
  });

  it("draws from the top-left corner with a square frame and visible dimensions", async () => {
    preparedSession = {
      ...session,
      initial_mode: "recording",
      display_corner_radius: 40,
    };
    const { container } = render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Record", pressed: true });

    const surface = container.querySelector<HTMLElement>(".recording-selector");
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

    fireEvent.pointerDown(surface!, { pointerId: 20, clientX: 0, clientY: 0 });
    fireEvent.pointerMove(surface!, { pointerId: 20, clientX: 400, clientY: 240 });
    await act(async () => {
      await new Promise<void>((resolve) => {
        requestAnimationFrame(() => resolve());
      });
    });
    fireEvent.pointerUp(surface!, { pointerId: 20, clientX: 400, clientY: 240 });

    const selection = container.querySelector<HTMLElement>(".recording-selection-frame");
    expect(selection).toHaveStyle({
      left: "0px",
      top: "0px",
      width: "400px",
      height: "240px",
    });
    expect(surface!.style.borderRadius).toBe("");
    expect(selection!.style.borderRadius).toBe("");
    expect(selection!.querySelector(".selection-dimensions"))
      .toHaveAttribute("data-screen-edge", "top");
  });

  it("starts full-screen capture on the current display and can switch displays before capture", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
      initial_target: "display",
    };
    const { container } = render(<RecordingSelector />);

    expect(await screen.findByRole("button", { name: "Full screen", pressed: true }))
      .toBeInTheDocument();
    expect(container.querySelector(".recording-display-outline")).toBeInTheDocument();
    expect(container.querySelector(".recording-display-outline")).not.toHaveStyle({
      borderRadius: "40px",
    });
    expect(container.querySelector(".recording-display-identity")).toHaveTextContent(
      "Built-in Retina Display1440 × 900",
    );
    expect(container.querySelector(".capture-shade-full")).toBeInTheDocument();
    expect(container.querySelector(".recording-selection-display")).not.toBeInTheDocument();

    const displayPicker = screen.getByRole("combobox", { name: "Display" });
    expect(displayPicker).toHaveTextContent("Built-in Retina Display");
    fireEvent.click(displayPicker);
    fireEvent.click(screen.getByRole("option", { name: /Studio Display/ }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("select_capture_display", {
        selectionId: session.id,
        displayId: "display-2",
      });
      expect(container.querySelector(".recording-display-identity")).toHaveTextContent(
        "Studio Display2560 × 1440",
      );
    });

    fireEvent.click(screen.getByRole("button", { name: "Take screenshot" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("capture_selection_screenshot", {
        request: {
          selection_id: preparedSession.id,
          target: { type: "display", display_id: "display-2" },
        },
      });
    });
  });

  it("keeps the selector usable when switching displays returns nothing", async () => {
    preparedSession = {
      ...session,
      initial_mode: "recording",
      initial_target: "display",
    };
    const defaultInvoke = vi.mocked(invoke).getMockImplementation();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "select_capture_display") return undefined;
      return defaultInvoke?.(command, args);
    });

    render(<RecordingSelector />);
    expect(await screen.findByRole("button", { name: "Full screen", pressed: true }))
      .toBeInTheDocument();

    fireEvent.click(screen.getByRole("combobox", { name: "Display" }));
    fireEvent.click(screen.getByRole("option", { name: /Studio Display/ }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Could not switch displays");
    expect(screen.getByRole("combobox", { name: "Display" })).toHaveTextContent(
      "Built-in Retina Display",
    );
    expect(screen.queryByText(/Cannot read properties of undefined/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start recording" })).toBeEnabled();
  });

  it("labels the recording action Start recording so it is distinct from Record mode", async () => {
    render(<RecordingSelector />);

    const start = await screen.findByRole("button", { name: "Start recording" });
    const recordMode = screen.getByRole("button", { name: "Record", pressed: true });
    expect(start).toHaveTextContent("Start recording");
    expect(start).toHaveClass("capture-selector-primary-recording");
    expect(start.querySelector(".capture-record-dot")).not.toBeNull();
    expect(recordMode).toHaveTextContent(/^Record$/);
    expect(recordMode.querySelector(".capture-record-dot")).not.toBeNull();
    expect(start).not.toBe(recordMode);
  });

  it("starts with no region so the user can draw mid-screen", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
    };
    const { container } = render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Screenshot", pressed: true });

    expect(container.querySelector(".recording-selection-frame")).not.toBeInTheDocument();
    const shade = container.querySelector<HTMLElement>(".capture-shade-full");
    expect(shade).toBeInTheDocument();
    expect(shade?.style.clipPath || "").toBe("");
    expect(screen.getByRole("button", { name: "Take screenshot" })).toBeDisabled();
    expect(screen.getByText("Drag to select a region")).toBeInTheDocument();
  });

  it("confirms a drawn region with Enter even if a toolbar control is focused", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
    };
    const { container } = render(<RecordingSelector />);

    const screenshotMode = await screen.findByRole("button", {
      name: "Screenshot",
      pressed: true,
    });
    const surface = container.querySelector<HTMLElement>(".recording-selector");
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

    fireEvent.pointerDown(surface!, { pointerId: 21, clientX: 100, clientY: 120 });
    fireEvent.pointerMove(surface!, { pointerId: 21, clientX: 400, clientY: 340 });
    await act(async () => {
      await new Promise<void>((resolve) => {
        requestAnimationFrame(() => resolve());
      });
    });
    fireEvent.pointerUp(surface!, { pointerId: 21, clientX: 400, clientY: 340 });

    fireEvent.keyDown(screenshotMode, { key: "Enter" });

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("capture_selection_screenshot", {
        request: {
          selection_id: preparedSession.id,
          target: {
            type: "region",
            display_id: "display-1",
            rect: { x: 100, y: 120, width: 300, height: 220 },
          },
        },
      });
    });
  });

  it("takes a full-screen screenshot when Enter is pressed in the capture menu", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
      initial_target: "display",
    };
    render(<RecordingSelector />);

    expect(await screen.findByRole("button", { name: "Full screen", pressed: true }))
      .toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Take screenshot" })).toBeEnabled();

    fireEvent.keyDown(window, { key: "Enter" });

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("capture_selection_screenshot", {
        request: {
          selection_id: preparedSession.id,
          target: { type: "display", display_id: "display-1" },
        },
      });
    });
  });

  it("does not capture with Enter while a dropdown is focused", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
      initial_target: "display",
    };
    render(<RecordingSelector />);

    const displayPicker = await screen.findByRole("combobox", { name: "Display" });
    fireEvent.keyDown(displayPicker, { key: "Enter" });
    expect(invoke).not.toHaveBeenCalledWith("capture_selection_screenshot", expect.anything());
  });

  it("does not capture with Enter on Close capture controls", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
      initial_target: "display",
    };
    render(<RecordingSelector />);

    const close = await screen.findByRole("button", { name: "Close capture controls" });
    fireEvent.keyDown(close, { key: "Enter" });
    expect(invoke).not.toHaveBeenCalledWith("capture_selection_screenshot", expect.anything());
  });

  it("does not capture with Enter on the auto-start Change link", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
      initial_target: "display",
    };
    const defaultInvoke = vi.mocked(invoke).getMockImplementation();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_settings") {
        return { ...settings, auto_start_on_selection: true };
      }
      return defaultInvoke?.(command, args);
    });

    render(<RecordingSelector />);
    const change = await screen.findByRole("button", { name: "Change…" });
    fireEvent.keyDown(change, { key: "Enter" });
    expect(invoke).not.toHaveBeenCalledWith("capture_selection_screenshot", expect.anything());
    expect(invoke).not.toHaveBeenCalledWith("open_preferences", expect.anything());
  });

  it("auto-starts a screenshot after drawing a region when the preference is on", async () => {
    const hiddenClick = ignoreClicksOnHiddenButtons();
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
    };
    await enableAutoStartPreference();

    const { container } = render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Screenshot", pressed: true });
    expect(container.querySelector(".capture-selector-note")).toHaveTextContent(
      "Auto-capture is on. Selecting a target starts immediately.",
    );
    expect(screen.queryByRole("button", { name: "Take screenshot" })).not.toBeInTheDocument();

    const surface = container.querySelector<HTMLElement>(".recording-selector");
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

    fireEvent.pointerDown(surface!, { pointerId: 22, clientX: 80, clientY: 90 });
    fireEvent.pointerMove(surface!, { pointerId: 22, clientX: 280, clientY: 250 });
    await act(async () => {
      await new Promise<void>((resolve) => {
        requestAnimationFrame(() => resolve());
      });
    });
    fireEvent.pointerUp(surface!, { pointerId: 22, clientX: 280, clientY: 250 });

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("capture_selection_screenshot", {
        request: {
          selection_id: preparedSession.id,
          target: {
            type: "region",
            display_id: "display-1",
            rect: { x: 80, y: 90, width: 200, height: 160 },
          },
        },
      });
    });
    hiddenClick.mockRestore();
  });

  it("opens and targets the auto-capture preference from the selector note", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
    };
    const defaultInvoke = vi.mocked(invoke).getMockImplementation();
    const resolvePreferences: { current: () => void } = { current: () => undefined };
    const preferencesOpened = new Promise<void>((resolve) => {
      resolvePreferences.current = resolve;
    });
    vi.mocked(invoke).mockImplementation((command, args) => {
      if (command === "get_settings") {
        return Promise.resolve({ ...settings, auto_start_on_selection: true });
      }
      if (command === "open_preferences") return preferencesOpened;
      return defaultInvoke
        ? defaultInvoke(command, args)
        : Promise.resolve(undefined);
    });

    render(<RecordingSelector />);
    const change = await screen.findByRole("button", { name: "Change…" });
    fireEvent.click(change);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("open_preferences", {
        target: "auto-start-on-selection",
      });
    });
    expect(invoke).not.toHaveBeenCalledWith("cancel_recording_selection", {
      selectionId: preparedSession.id,
    });

    resolvePreferences.current();

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("cancel_recording_selection", {
        selectionId: preparedSession.id,
      });
    });
    const actionCalls = vi.mocked(invoke).mock.calls
      .map(([command]) => command)
      .filter((command) => command === "open_preferences" || command === "cancel_recording_selection");
    expect(actionCalls).toEqual(["open_preferences", "cancel_recording_selection"]);
  });

  it("auto-starts after picking a window when the preference is on", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
      initial_target: "window",
    };
    const defaultInvoke = vi.mocked(invoke).getMockImplementation();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_settings") {
        return { ...settings, auto_start_on_selection: true };
      }
      return defaultInvoke?.(command, args);
    });

    const { container } = render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Window", pressed: true });
    const surface = mockSelectorSurface(container);
    // Title is "Front eligible window"; id is back-window (mid z-order).
    fireEvent.pointerDown(surface, { pointerId: 1, clientX: 950, clientY: 400 });

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("capture_selection_screenshot", {
        request: {
          selection_id: preparedSession.id,
          target: { type: "window", window_id: "back-window" },
        },
      });
    });
  });

  it("auto-starts a full-screen screenshot from its target button", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
    };
    const defaultInvoke = vi.mocked(invoke).getMockImplementation();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_settings") {
        return { ...settings, auto_start_on_selection: true };
      }
      return defaultInvoke?.(command, args);
    });

    render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Screenshot", pressed: true });
    fireEvent.click(screen.getByRole("button", { name: "Full screen" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("capture_selection_screenshot", {
        request: {
          selection_id: preparedSession.id,
          target: { type: "display", display_id: preparedSession.display.id },
        },
      });
    });
  });

  it("auto-starts when Full screen is already selected", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
      initial_target: "display",
    };
    const defaultInvoke = vi.mocked(invoke).getMockImplementation();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_settings") {
        return { ...settings, auto_start_on_selection: true };
      }
      return defaultInvoke?.(command, args);
    });

    render(<RecordingSelector />);
    const fullScreen = await screen.findByRole("button", {
      name: "Full screen",
      pressed: true,
    });
    fireEvent.click(fullScreen);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("capture_selection_screenshot", {
        request: {
          selection_id: preparedSession.id,
          target: { type: "display", display_id: preparedSession.display.id },
        },
      });
    });
  });

  it("auto-starts after choosing another full-screen display", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
      initial_target: "display",
    };
    const defaultInvoke = vi.mocked(invoke).getMockImplementation();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_settings") {
        return { ...settings, auto_start_on_selection: true };
      }
      return defaultInvoke?.(command, args);
    });

    render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Full screen", pressed: true });
    fireEvent.click(screen.getByRole("combobox", { name: "Display" }));
    fireEvent.click(screen.getByRole("option", { name: /Studio Display/ }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("capture_selection_screenshot", {
        request: {
          selection_id: preparedSession.id,
          target: { type: "display", display_id: "display-2" },
        },
      });
    });
  });

  it("reveals a retry action when auto-capture fails", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
    };
    const defaultInvoke = vi.mocked(invoke).getMockImplementation();
    let captureAttempts = 0;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_settings") {
        return { ...settings, auto_start_on_selection: true };
      }
      if (command === "capture_selection_screenshot") {
        captureAttempts += 1;
        if (captureAttempts === 1) throw new Error("capture failed");
        return undefined;
      }
      return defaultInvoke?.(command, args);
    });

    render(<RecordingSelector />);
    fireEvent.click(await screen.findByRole("button", { name: "Full screen" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("capture failed");
    fireEvent.click(screen.getByRole("button", { name: "Retry capture" }));
    await waitFor(() => expect(captureAttempts).toBe(2));
  });

  it("auto-starts a recording after drawing a region when the preference is on", async () => {
    const hiddenClick = ignoreClicksOnHiddenButtons();
    await enableAutoStartPreference();
    const { container } = render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Record", pressed: true });
    expect(screen.queryByRole("button", { name: "Start recording" })).not.toBeInTheDocument();

    const surface = mockSelectorSurfaceForDrag(container);
    fireEvent.pointerDown(surface, { pointerId: 22, clientX: 80, clientY: 90 });
    fireEvent.pointerMove(surface, { pointerId: 22, clientX: 280, clientY: 250 });
    await act(async () => {
      await new Promise<void>((resolve) => {
        requestAnimationFrame(() => resolve());
      });
    });
    fireEvent.pointerUp(surface, { pointerId: 22, clientX: 280, clientY: 250 });

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_recording", {
        request: {
          selection_id: preparedSession.id,
          options: expect.objectContaining({
            kind: "video",
            target: {
              type: "region",
              display_id: "display-1",
              rect: { x: 80, y: 90, width: 200, height: 160 },
            },
          }),
        },
      });
    });
    expect(invoke).not.toHaveBeenCalledWith("capture_selection_screenshot", expect.anything());
    expect(invoke).not.toHaveBeenCalledWith("cancel_recording_selection", expect.anything());
    hiddenClick.mockRestore();
  });

  it("auto-starts a recording after picking a window when the preference is on", async () => {
    const hiddenClick = ignoreClicksOnHiddenButtons();
    preparedSession = {
      ...session,
      initial_target: "window",
    };
    await enableAutoStartPreference();
    const { container } = render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Window", pressed: true });
    const surface = mockSelectorSurface(container);
    fireEvent.pointerDown(surface, { pointerId: 1, clientX: 950, clientY: 400 });

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_recording", {
        request: {
          selection_id: preparedSession.id,
          options: expect.objectContaining({
            kind: "video",
            target: { type: "window", window_id: "back-window" },
          }),
        },
      });
    });
    expect(invoke).not.toHaveBeenCalledWith("capture_selection_screenshot", expect.anything());
    expect(invoke).not.toHaveBeenCalledWith("cancel_recording_selection", expect.anything());
    hiddenClick.mockRestore();
  });

  it("keeps the recording menu open when choosing Region or Window with auto-start on", async () => {
    await enableAutoStartPreference();
    render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Record", pressed: true });
    expect(screen.getByRole("button", { name: "Region", pressed: true })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Window" }));
    expect(await screen.findByRole("button", { name: "Window", pressed: true })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Record", pressed: true })).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("start_recording", expect.anything());
    expect(invoke).not.toHaveBeenCalledWith("cancel_recording_selection", expect.anything());

    fireEvent.click(screen.getByRole("button", { name: "Region" }));
    expect(await screen.findByRole("button", { name: "Region", pressed: true })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Record", pressed: true })).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("start_recording", expect.anything());
    expect(invoke).not.toHaveBeenCalledWith("cancel_recording_selection", expect.anything());
    expect(screen.getByText("Drag to select a region")).toBeInTheDocument();
  });

  it("animates the controls panel to its recording dimensions", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
    };
    const { container } = render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Screenshot", pressed: true });

    const panel = container.querySelector<HTMLElement>(".recording-selector-panel");
    expect(panel).not.toBeNull();
    const bounds = [
      { width: 590, height: 85 },
      { width: 790, height: 161 },
    ];
    vi.spyOn(panel!, "getBoundingClientRect").mockImplementation(() => {
      const { width, height } = bounds.shift() ?? { width: 790, height: 161 };
      return {
        x: 0,
        y: 0,
        top: 0,
        left: 0,
        right: width,
        bottom: height,
        width,
        height,
        toJSON: () => undefined,
      };
    });
    const animation = {
      addEventListener: vi.fn(),
      cancel: vi.fn(),
    } as unknown as Animation;
    const animate = vi.fn(() => animation);
    Object.defineProperty(panel, "animate", {
      configurable: true,
      value: animate,
    });

    fireEvent.click(screen.getByRole("button", { name: "Record", pressed: false }));

    expect(animate).toHaveBeenCalledWith([
      { width: "590px", height: "85px" },
      { width: "790px", height: "161px" },
    ], {
      duration: 280,
      easing: "cubic-bezier(.2,.8,.2,1)",
    });
    expect(panel).toHaveAttribute("data-resizing", "true");
  });

  it("starts Window mode unselected and previews hovered windows with rounded geometry", async () => {
    const { container } = render(<RecordingSelector />);

    fireEvent.click(await screen.findByRole("button", { name: "Window" }));

    const surface = mockSelectorSurface(container);
    expect(screen.queryByText(/Window selected/)).not.toBeInTheDocument();
    const windowGuidance = screen.getByText("Select a window to continue").closest(".capture-guidance");
    expect(windowGuidance).toHaveTextContent("Esc to cancel");
    expect(screen.queryByText(/enable (Capture|Record)/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Select Captures Preferences" })).toBeInTheDocument();
    const frontWindow = screen.getByRole("button", { name: "Select Front eligible window" });
    const rearWindow = screen.getByRole("button", { name: "Select Rear window" });
    expect(frontWindow).not.toHaveClass("selected");
    expect(Number(frontWindow.style.zIndex)).toBeGreaterThan(Number(rearWindow.style.zIndex));
    expect(frontWindow).toHaveStyle({
      left: "300px",
      top: "160px",
      width: "900px",
      height: "640px",
      borderRadius: "25px",
    });
    expect(screen.getByRole("button", { name: "Select Captures Preferences" })).toHaveStyle({
      borderRadius: "12px",
    });
    expect(screen.getByRole("button", { name: "Start recording" })).toBeDisabled();
    expect(container.querySelector(".recording-selection-window")).not.toBeInTheDocument();
    expect(container.querySelector(".capture-shade-path")).not.toBeInTheDocument();
    expect(container.querySelector(".capture-shade-full")).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Enter" });
    expect(invoke).not.toHaveBeenCalledWith("start_recording", expect.anything());

    // Rear sits entirely under Front eligible; hit-test the unique Front strip.
    fireEvent.pointerMove(surface, { pointerId: 1, clientX: 950, clientY: 400 });
    await waitFor(() => {
      expect(frontWindow).toHaveClass("hovered");
      expect(container.querySelector(".capture-shade-path")).toHaveAttribute(
        "d",
        expect.stringContaining(
          "M325 160H1175A25 25 0 0 1 1200 185V775"
          + "A25 25 0 0 1 1175 800H325",
        ),
      );
    });
    fireEvent.pointerMove(surface, { pointerId: 1, clientX: 10, clientY: 10 });
    await waitFor(() => {
      expect(frontWindow).not.toHaveClass("hovered");
    });

    // Preferences is frontmost (z 30). A point only it covers selects it, not the windows underneath.
    fireEvent.pointerMove(surface, { pointerId: 1, clientX: 150, clientY: 100 });
    fireEvent.pointerDown(surface, { pointerId: 1, clientX: 150, clientY: 100 });
    const preferences = screen.getByRole("button", { name: "Select Captures Preferences" });
    expect(preferences).toHaveClass("selected");
    expect(screen.queryByText("Select a window")).not.toBeInTheDocument();
    expect(screen.queryByText("Esc to cancel")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start recording" })).toBeEnabled();
    expect(container.querySelector(".capture-shade-path")).toHaveAttribute(
      "d",
      expect.stringContaining(
        "M112 80H888A12 12 0 0 1 900 92V668"
        + "A12 12 0 0 1 888 680H112",
      ),
    );
  });

  it("treats the menu bar and empty desktop as a full-display target in Window mode", async () => {
    preparedSession = {
      ...session,
      windows: [
        {
          id: "fullscreen-app",
          title: "Safari",
          app_name: "Safari",
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
    const { container } = render(<RecordingSelector />);
    fireEvent.click(await screen.findByRole("button", { name: "Window" }));
    const surface = mockSelectorSurface(container);

    fireEvent.pointerMove(surface, { pointerId: 1, clientX: 20, clientY: 8 });
    expect(await screen.findByText("Click to capture this display")).toBeInTheDocument();
    expect(container.querySelector(".recording-display-outline")).not.toBeNull();
    expect(screen.getByRole("button", { name: "Window" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Select Safari" })).not.toHaveClass("hovered");

    fireEvent.pointerDown(surface, { pointerId: 1, clientX: 20, clientY: 8 });
    expect(screen.getByRole("button", { name: "Full screen" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Start recording" })).toBeEnabled();
    expect(container.querySelector(".recording-display-identity")?.textContent)
      .toContain("Built-in Retina Display");
  });

  it("does not treat a miss as full screen while window listing is still deferred", async () => {
    preparedSession = {
      ...session,
      windows: [],
      shell_chrome: [],
      windows_ready: false,
    };
    const { container } = render(<RecordingSelector />);
    fireEvent.click(await screen.findByRole("button", { name: "Window" }));
    const surface = mockSelectorSurface(container);

    fireEvent.pointerMove(surface, { pointerId: 1, clientX: 20, clientY: 8 });
    expect(screen.queryByText("Click to capture this display")).not.toBeInTheDocument();
    fireEvent.pointerDown(surface, { pointerId: 1, clientX: 20, clientY: 8 });
    expect(screen.getByRole("button", { name: "Window" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Full screen" })).toHaveAttribute("aria-pressed", "false");
  });

  it("keeps region and display capture available when the desktop cannot enumerate windows", async () => {
    preparedSession = {
      ...session,
      windows: [],
    };
    render(<RecordingSelector />);

    expect(await screen.findByRole("button", { name: "Window" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Region" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Full screen" })).toBeEnabled();
    // Region starts empty — capture waits until the user draws a selection.
    expect(screen.getByRole("button", { name: "Start recording" })).toBeDisabled();
  });

  it("keeps the selector surface fixed while the controls are dragged within the display", async () => {
    const { container } = render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Start recording" });
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

  it("enables recording for any region large enough to encode", async () => {
    const { container } = render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Start recording" });
    const surface = container.querySelector<HTMLElement>(".recording-selector");
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

    fireEvent.pointerDown(surface!, { pointerId: 7, clientX: 100, clientY: 100 });
    fireEvent.pointerMove(surface!, { pointerId: 7, clientX: 101, clientY: 101 });
    fireEvent.pointerUp(surface!, { pointerId: 7, clientX: 101, clientY: 101 });
    expect(screen.getByRole("button", { name: "Start recording" })).toBeDisabled();

    fireEvent.pointerDown(surface!, { pointerId: 8, clientX: 100, clientY: 100 });
    fireEvent.pointerMove(surface!, { pointerId: 8, clientX: 102, clientY: 102 });
    fireEvent.pointerUp(surface!, { pointerId: 8, clientX: 102, clientY: 102 });
    expect(screen.getByRole("button", { name: "Start recording" })).toBeEnabled();
  });

  it("hides region guidance while creating a new region selection", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
    };
    const { container } = render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Screenshot", pressed: true });
    const guidance = screen.getByText("Drag to select a region").closest(".capture-guidance");
    expect(guidance).not.toHaveAttribute("data-faded");

    const surface = container.querySelector<HTMLElement>(".recording-selector");
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

    // Drag anywhere on the empty surface to start a create selection.
    fireEvent.pointerDown(surface!, { pointerId: 9, clientX: 20, clientY: 20 });
    expect(guidance).toHaveAttribute("data-faded", "true");

    fireEvent.pointerMove(surface!, { pointerId: 9, clientX: 120, clientY: 140 });
    fireEvent.pointerUp(surface!, { pointerId: 9, clientX: 120, clientY: 140 });
    expect(guidance).not.toHaveAttribute("data-faded");
  });

  it("locks a create drag to the selected aspect ratio and Shift square", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
    };
    const { container } = render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Screenshot", pressed: true });

    const aspect = screen.getByRole("combobox", { name: "Region aspect ratio" });
    fireEvent.click(aspect);
    fireEvent.click(screen.getByRole("option", { name: "16 : 9" }));
    expect(aspect).toHaveTextContent("16 : 9");

    const surface = container.querySelector<HTMLElement>(".recording-selector");
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

    // Empty surface: any drag creates a new region.
    fireEvent.pointerDown(surface!, { pointerId: 11, clientX: 20, clientY: 20 });
    fireEvent.pointerMove(surface!, { pointerId: 11, clientX: 340, clientY: 300 });
    // Flush the rAF used to batch region pointer moves.
    await act(async () => {
      await new Promise<void>((resolve) => {
        requestAnimationFrame(() => resolve());
      });
    });
    fireEvent.pointerUp(surface!, { pointerId: 11, clientX: 340, clientY: 300 });

    const frame = container.querySelector<HTMLElement>(".recording-selection-frame");
    expect(frame).not.toBeNull();
    const width = Number.parseFloat(frame!.style.width);
    const height = Number.parseFloat(frame!.style.height);
    expect(width / height).toBeCloseTo(16 / 9, 2);

    fireEvent.pointerDown(surface!, { pointerId: 12, clientX: 20, clientY: 20, shiftKey: true });
    fireEvent.pointerMove(surface!, {
      pointerId: 12,
      clientX: 220,
      clientY: 300,
      shiftKey: true,
    });
    await act(async () => {
      await new Promise<void>((resolve) => {
        requestAnimationFrame(() => resolve());
      });
    });
    fireEvent.pointerUp(surface!, {
      pointerId: 12,
      clientX: 220,
      clientY: 300,
      shiftKey: true,
    });

    const squareFrame = container.querySelector<HTMLElement>(".recording-selection-frame");
    expect(squareFrame).not.toBeNull();
    const squareWidth = Number.parseFloat(squareFrame!.style.width);
    const squareHeight = Number.parseFloat(squareFrame!.style.height);
    expect(squareWidth).toBeCloseTo(squareHeight, 0);
  });

  it("snaps an existing region as soon as the aspect preset changes", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
    };
    const { container } = render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Screenshot", pressed: true });

    const surface = container.querySelector<HTMLElement>(".recording-selector");
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

    // Empty-start: draw a free region first so there is something to snap.
    fireEvent.pointerDown(surface!, { pointerId: 13, clientX: 40, clientY: 40 });
    fireEvent.pointerMove(surface!, { pointerId: 13, clientX: 400, clientY: 280 });
    await act(async () => {
      await new Promise<void>((resolve) => {
        requestAnimationFrame(() => resolve());
      });
    });
    fireEvent.pointerUp(surface!, { pointerId: 13, clientX: 400, clientY: 280 });
    expect(container.querySelector(".recording-selection-frame")).not.toBeNull();

    const aspect = screen.getByRole("combobox", { name: "Region aspect ratio" });
    fireEvent.click(aspect);
    fireEvent.click(screen.getByRole("option", { name: "16 : 9" }));

    const wideFrame = container.querySelector<HTMLElement>(".recording-selection-frame");
    expect(wideFrame).not.toBeNull();
    const wideWidth = Number.parseFloat(wideFrame!.style.width);
    const wideHeight = Number.parseFloat(wideFrame!.style.height);
    expect(wideWidth / wideHeight).toBeCloseTo(16 / 9, 2);

    fireEvent.click(aspect);
    fireEvent.click(screen.getByRole("option", { name: "1 : 1" }));

    const squareFrame = container.querySelector<HTMLElement>(".recording-selection-frame");
    expect(squareFrame).not.toBeNull();
    const squareWidth = Number.parseFloat(squareFrame!.style.width);
    const squareHeight = Number.parseFloat(squareFrame!.style.height);
    expect(squareWidth / squareHeight).toBeCloseTo(1, 2);
    // Inscribed in the previous 16:9 box — side matches the previous height.
    expect(squareWidth).toBeCloseTo(wideHeight, 0);
    expect(squareHeight).toBeCloseTo(wideHeight, 0);
  });

  it("fades window guidance when the cursor enters its bounds", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
    };
    render(<RecordingSelector />);
    fireEvent.click(await screen.findByRole("button", { name: "Window" }));

    const guidance = screen.getByText("Select a window to continue")
      .closest(".capture-guidance") as HTMLElement;
    vi.spyOn(guidance, "getBoundingClientRect").mockReturnValue({
      x: 500,
      y: 120,
      top: 120,
      left: 500,
      right: 760,
      bottom: 180,
      width: 260,
      height: 60,
      toJSON: () => undefined,
    });

    fireEvent.pointerMove(window, { clientX: 620, clientY: 150 });
    expect(guidance).toHaveAttribute("data-faded", "true");

    fireEvent.pointerMove(window, { clientX: 10, clientY: 10 });
    expect(guidance).not.toHaveAttribute("data-faded");
  });

  it("lets one Escape close an open control and cancel the selector exactly once", async () => {
    render(<RecordingSelector />);
    const microphone = await screen.findByRole("combobox", { name: "Microphone" });
    fireEvent.click(microphone);
    expect(microphone).toHaveAttribute("aria-expanded", "true");

    fireEvent.keyDown(microphone, { key: "Escape" });
    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.keyDown(window, { key: "Escape" });

    expect(screen.queryByRole("button", { name: "Start recording" })).not.toBeInTheDocument();
    expect(vi.mocked(invoke).mock.calls.filter(([command]) => (
      command === "cancel_recording_selection"
    ))).toHaveLength(1);
  });

  it("reveals through the safety path when a hidden WebView defers image loading", async () => {
    render(<RecordingSelector />);

    expect(await screen.findByRole("button", { name: "Start recording" })).toBeInTheDocument();
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

  it("reveals a live selector without a freeze-frame snapshot", async () => {
    preparedSession = { ...session, frozen: false, snapshot_url: "" };
    const { container } = render(<RecordingSelector />);

    expect(await screen.findByRole("button", { name: "Start recording" })).toBeInTheDocument();
    expect(container.querySelector(".recording-selector-snapshot")).toBeNull();
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("reveal_recording_selector", {
        selectionId: session.id,
      });
    });
  });

  it("loads the prepared selection while event registration is still pending", async () => {
    vi.mocked(listen).mockImplementationOnce(() => new Promise(() => undefined));

    render(<RecordingSelector />);

    expect(await screen.findByRole("button", { name: "Start recording" })).toBeInTheDocument();
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

    expect(await screen.findByRole("button", { name: "Start recording" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });

    expect(screen.queryByRole("button", { name: "Start recording" })).not.toBeInTheDocument();
    expect(container.querySelector(".recording-selector-idle")).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("cancel_recording_selection", {
      selectionId: session.id,
    });

    await act(async () => {
      recordingSelectionReady?.({ payload: nextSession });
    });

    expect(await screen.findByRole("button", { name: "Start recording" })).toBeInTheDocument();
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
      // Region cutouts use CSS clip-path in marquee CSS pixels (not SVG viewBox).
      expect(container.querySelector(".capture-shade-path")).not.toBeInTheDocument();
      expect(container.querySelector(".capture-shade-full")).toHaveStyle({
        clipPath: "polygon(evenodd, 0% 0%, 100% 0%, 100% 100%, 0% 100%, 0% 0%, "
          + "100px 120px, 100px 340px, 400px 340px, 400px 120px, 100px 120px)",
      });
    });
  });

  it("starts a screenshot from the matching shortcut while the capture menu is open", async () => {
    render(<RecordingSelector />);
    expect(await screen.findByRole("button", { name: "Close capture controls" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Screenshot" }));
    fireEvent.click(screen.getByRole("button", { name: "Window" }));
    expect(screen.getByRole("button", { name: "Window", pressed: true })).toBeInTheDocument();

    fireEvent.keyDown(window, {
      key: "4",
      code: "Digit4",
      ctrlKey: true,
      shiftKey: true,
    });

    expect(await screen.findByRole("button", { name: "Region", pressed: true })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Screenshot", pressed: true })).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("start_capture", expect.anything());
  });

  it("switches to Window screenshot from its shortcut without rebuilding the capture menu", async () => {
    render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Close capture controls" });
    expect(screen.getByRole("button", { name: "Region", pressed: true })).toBeInTheDocument();

    fireEvent.keyDown(window, {
      key: "w",
      code: "KeyW",
      ctrlKey: true,
      shiftKey: true,
    });

    expect(await screen.findByRole("button", { name: "Window", pressed: true })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Screenshot", pressed: true })).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("start_capture", expect.anything());
  });

  it("switches to Record mode from the recording shortcut while the capture menu is open", async () => {
    render(<RecordingSelector />);
    fireEvent.click(await screen.findByRole("button", { name: "Screenshot" }));
    expect(screen.getByRole("button", { name: "Screenshot", pressed: true })).toBeInTheDocument();

    fireEvent.keyDown(window, {
      key: "5",
      code: "Digit5",
      ctrlKey: true,
      shiftKey: true,
    });

    expect(await screen.findByRole("button", { name: "Record", pressed: true })).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("start_recording", expect.anything());
    expect(invoke).not.toHaveBeenCalledWith("start_capture", expect.anything());
  });

  it("does not discard a prepared selection after a transient reveal failure", async () => {
    selectorShowError = new Error("macOS could not focus the selector");

    render(<RecordingSelector />);

    expect(await screen.findByRole("alert")).toHaveTextContent("macOS could not focus the selector");
    expect(invoke).not.toHaveBeenCalledWith("cancel_recording_selection", expect.anything());
  });

  it("adds window targets without re-revealing an already visible selector", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
      initial_target: "window",
      windows: [],
    };
    const { container } = render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Window", pressed: true });
    expect(container.querySelectorAll(".recording-window-target")).toHaveLength(0);

    const snapshot = container.querySelector(".recording-selector-snapshot");
    expect(snapshot).not.toBeNull();
    fireEvent.load(snapshot!);
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("reveal_recording_selector", {
        selectionId: preparedSession.id,
      });
    });
    const revealCalls = vi.mocked(invoke).mock.calls.filter(([command]) => (
      command === "reveal_recording_selector"
    )).length;
    const showCalls = vi.mocked(invoke).mock.calls.filter(([command]) => (
      command === "show_recording_selector"
    )).length;

    await act(async () => {
      recordingSelectionReady?.({
        payload: {
          ...preparedSession,
          windows: session.windows,
        },
      });
    });

    expect(await screen.findByRole("button", { name: "Select Captures Preferences" }))
      .toBeInTheDocument();
    expect(vi.mocked(invoke).mock.calls.filter(([command]) => (
      command === "reveal_recording_selector"
    ))).toHaveLength(revealCalls);
    expect(vi.mocked(invoke).mock.calls.filter(([command]) => (
      command === "show_recording_selector"
    ))).toHaveLength(showCalls);
  });

  it("reloads microphones when a reused selector starts a new session", async () => {
    render(<RecordingSelector />);
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("list_recording_audio_devices");
    });

    audioDevices = [
      { id: "default", name: "Default — Studio Microphone", kind: "default", is_default: true },
      { id: "microphone-3", name: "Headset Microphone", kind: "microphone", is_default: false },
    ];
    const nextSession = {
      ...session,
      id: "selection-2",
      snapshot_url: "capture://recording-selection/selection-2",
    };

    await act(async () => {
      recordingSelectionReady?.({ payload: nextSession });
    });

    await waitFor(() => {
      expect(vi.mocked(invoke).mock.calls.filter(([command]) => (
        command === "list_recording_audio_devices"
      ))).toHaveLength(2);
    });
    fireEvent.click(screen.getByRole("combobox", { name: "Microphone" }));
    expect(await screen.findByRole("option", { name: "Headset Microphone" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "USB Microphone" })).not.toBeInTheDocument();
  });

  it("highlights the window under the pointer when Window capture starts", async () => {
    capturePointer = { x: 150, y: 100, inside: true };
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
      initial_target: "window",
    };
    const { container } = render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Window", pressed: true });
    mockSelectorSurface(container);

    const preferences = screen.getByRole("button", { name: "Select Captures Preferences" });
    await waitFor(() => {
      expect(preferences).toHaveClass("hovered");
    });
  });

  it("re-polls the pointer when a new selector session starts", async () => {
    capturePointer = { x: 150, y: 100, inside: true };
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
      initial_target: "window",
    };
    const { container } = render(<RecordingSelector />);
    await screen.findByRole("button", { name: "Window", pressed: true });
    mockSelectorSurface(container);
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Select Captures Preferences" }))
        .toHaveClass("hovered");
    });

    capturePointer = { x: 950, y: 400, inside: true };
    await act(async () => {
      recordingSelectionReady?.({
        payload: {
          ...preparedSession,
          id: "selection-2",
          snapshot_url: "capture://recording-selection/selection-2",
        },
      });
    });

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Select Front eligible window" }))
        .toHaveClass("hovered");
    });
    expect(screen.getByRole("button", { name: "Select Captures Preferences" }))
      .not.toHaveClass("hovered");
  });
});
