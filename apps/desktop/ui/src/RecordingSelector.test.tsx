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
  feedback_shortcut: "Ctrl+Shift+F",
  auto_copy_to_clipboard: true,
  show_mini_previews: true,
  include_mini_previews_in_captures: false,
  include_recording_controls_in_captures: false,
  launch_at_login: false,
  last_screen_permission_request_id: null,
  pending_capture_after_restart: null,
  onboarding_completed: true,
  screenshot_countdown_seconds: 0,
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

describe("RecordingSelector", () => {
  let selectorShowError: Error | null;
  let preparedSession: RecordingSelectionSession;
  let recordingSelectionReady:
    | ((event: { payload: RecordingSelectionSession }) => void)
    | null;

  beforeEach(() => {
    selectorShowError = null;
    preparedSession = session;
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
        return [
          { id: "default", name: "Default — Studio Microphone", kind: "default", is_default: true },
          { id: "microphone-2", name: "USB Microphone", kind: "microphone", is_default: false },
        ];
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
        || command === "capture_selection_screenshot"
        || command === "start_recording"
        || command === "cancel_recording_selection"
      ) {
        return undefined;
      }
      if (command === "recording_controls_are_excluded") {
        return preparedSession.recording_capabilities.controls_excluded;
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
    expect(screen.getByRole("combobox", { name: "Region aspect ratio" })).toBeInTheDocument();
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
    fireEvent.click(screen.getByRole("button", { name: "Select Front eligible window" }));
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

  it("confirms the default region with Enter without overriding focused controls", async () => {
    preparedSession = {
      ...session,
      initial_mode: "screenshot",
    };
    render(<RecordingSelector />);

    const screenshotMode = await screen.findByRole("button", {
      name: "Screenshot",
      pressed: true,
    });
    fireEvent.keyDown(screenshotMode, { key: "Enter" });
    expect(invoke).not.toHaveBeenCalledWith("capture_selection_screenshot", expect.anything());

    fireEvent.keyDown(window, { key: "Enter" });

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("capture_selection_screenshot", {
        request: {
          selection_id: preparedSession.id,
          target: {
            type: "region",
            display_id: "display-1",
            rect: { x: 245, y: 171, width: 950, height: 558 },
          },
        },
      });
    });
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

    fireEvent.mouseEnter(rearWindow);
    await waitFor(() => {
      expect(rearWindow).toHaveClass("hovered");
      expect(container.querySelector(".capture-shade-path")).toHaveAttribute(
        "d",
        expect.stringContaining(
          "M445 220H1115A25 25 0 0 1 1140 245V715"
          + "A25 25 0 0 1 1115 740H445",
        ),
      );
    });
    fireEvent.mouseLeave(rearWindow);

    // Preferences uses a measured 12pt radius, not the session-wide 25pt default.
    const preferences = screen.getByRole("button", { name: "Select Captures Preferences" });
    fireEvent.click(preferences);
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

  it("keeps region and display capture available when the desktop cannot enumerate windows", async () => {
    preparedSession = {
      ...session,
      windows: [],
    };
    render(<RecordingSelector />);

    expect(await screen.findByRole("button", { name: "Window" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Region" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Full screen" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Start recording" })).toBeEnabled();
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

    // Click outside the default selection frame to start a create drag.
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

    // Outside the default frame so this is a create drag, not a move.
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

  it("does not discard a prepared selection after a transient reveal failure", async () => {
    selectorShowError = new Error("macOS could not focus the selector");

    render(<RecordingSelector />);

    expect(await screen.findByRole("alert")).toHaveTextContent("macOS could not focus the selector");
    expect(invoke).not.toHaveBeenCalledWith("cancel_recording_selection", expect.anything());
  });
});
