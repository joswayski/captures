import { invoke } from "@tauri-apps/api/core";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

import { Preferences } from "./App";
import type { AppSettings } from "./types";

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
  launch_at_login: false,
  last_screen_permission_request_id: null,
  pending_capture_after_restart: null,
  screenshot_countdown_seconds: 0,
  recording: {
    video_shortcut: "Ctrl+Shift+5",
    gif_shortcut: "Ctrl+Shift+6",
    video_fps: 30,
    video_max_resolution: "p1080",
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

describe("Preferences", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_settings") return settings;
      if (command === "get_update_status") {
        return { state: "idle", current_version: "0.1.0", current_display_version: "0.1.0" };
      }
      if (command === "set_shortcut_capture_suppressed") return undefined;
      if (command === "update_settings") return (args as { settings: AppSettings }).settings;
      throw new Error(`unexpected command: ${command}`);
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
    document.documentElement.removeAttribute("data-capture-theme");
    document.documentElement.removeAttribute("style");
    window.localStorage.clear();
  });

  it("automatically persists changes and reports that they were saved", async () => {
    render(<Preferences />);

    const autoCopy = await screen.findByRole("checkbox", {
      name: /Automatically copy captures to the clipboard/,
    });
    expect(autoCopy).toBeChecked();
    expect(screen.queryByRole("button", { name: "Close Preferences" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();

    fireEvent.click(autoCopy);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ auto_copy_to_clipboard: false }),
      });
    });
    expect(await screen.findByText("Changes saved")).toBeInTheDocument();
  });

  it("persists custom screenshot and recording countdown preferences", async () => {
    render(<Preferences />);

    const screenshotCountdown = await screen.findByRole("combobox", {
      name: "Screenshot countdown",
    });
    fireEvent.click(screenshotCountdown);
    fireEvent.click(await screen.findByRole("option", { name: "4 seconds" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ screenshot_countdown_seconds: 4 }),
      });
    });

    const recordingCountdown = screen.getByRole("combobox", { name: "Recording countdown" });
    fireEvent.click(recordingCountdown);
    fireEvent.click(await screen.findByRole("option", { name: "7 seconds" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({
          screenshot_countdown_seconds: 4,
          recording: expect.objectContaining({ countdown_seconds: 7 }),
        }),
      });
    });
  });

  it("can disable quick-access mini previews", async () => {
    render(<Preferences />);

    const miniPreviews = await screen.findByRole("checkbox", {
      name: /Show mini previews after screenshots/,
    });
    expect(miniPreviews).toBeChecked();
    fireEvent.click(miniPreviews);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ show_mini_previews: false }),
      });
    });
  });

  it("can include mini previews in screenshots and recordings", async () => {
    render(<Preferences />);

    const includePreviews = await screen.findByRole("checkbox", {
      name: /Include mini previews in screenshots and recordings/,
    });
    expect(includePreviews).not.toBeChecked();
    expect(includePreviews).toBeEnabled();
    fireEvent.click(includePreviews);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ include_mini_previews_in_captures: true }),
      });
    });
  });

  it("automatically applies a newly recorded shortcut", async () => {
    render(<Preferences />);

    const recorder = await screen.findByRole("button", { name: "Window" });
    fireEvent.click(recorder);
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_shortcut_capture_suppressed", {
        suppressed: true,
      });
    });
    fireEvent.keyDown(recorder, {
      code: "Digit0",
      key: ")",
      ctrlKey: true,
      shiftKey: true,
    });

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ window_shortcut: "Control+Shift+Digit0" }),
      });
    });
    expect(invoke).toHaveBeenCalledWith("set_shortcut_capture_suppressed", {
      suppressed: false,
    });
    expect(await screen.findByText("Changes saved")).toBeInTheDocument();
  });

  it("previews and persists a color theme", async () => {
    render(<Preferences />);

    const cobalt = await screen.findByRole("radio", { name: /Cobalt/ });
    expect(cobalt).not.toBeChecked();

    fireEvent.click(cobalt);

    expect(document.documentElement).toHaveAttribute("data-capture-theme", "cobalt");
    expect(cobalt).toBeChecked();
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ theme: "cobalt" }),
      });
    });
  });

  it("presents the expanded spectrum as compact theme choices", async () => {
    render(<Preferences />);

    expect(await screen.findAllByRole("radio")).toHaveLength(10);
    expect(screen.getByRole("radio", { name: /Violet/ })).toHaveAttribute(
      "data-capture-theme",
      "violet",
    );
    expect(screen.getByRole("radio", { name: /Cobalt/ })).toHaveAttribute(
      "data-capture-theme",
      "cobalt",
    );
    expect(screen.getByRole("radio", { name: /Mono/ })).toHaveAttribute(
      "title",
      "Vercel-like black and white",
    );
    expect(screen.getByRole("radio", { name: /Custom/ })).toHaveClass(
      "theme-option-custom",
    );
  });

  it("builds and persists a custom theme from editable colors", async () => {
    render(<Preferences />);

    fireEvent.click(await screen.findByRole("radio", { name: /Custom/ }));
    const editor = screen.getByRole("group", { name: "Custom theme colors" });
    expect(editor).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Accent color picker"), {
      target: { value: "#123456" },
    });
    const signalHex = screen.getByRole("textbox", {
      name: "Recording signal hex value",
    });
    fireEvent.change(signalHex, { target: { value: "#22AA55" } });
    fireEvent.blur(signalHex);

    expect(document.documentElement).toHaveAttribute("data-capture-theme", "custom");
    expect(document.documentElement.style.getPropertyValue("--theme-accent")).toBe("#123456");
    expect(document.documentElement.style.getPropertyValue("--theme-signal")).toBe("#22aa55");
    expect(document.documentElement.style.getPropertyValue("--theme-accent-hover")).not.toBe("");
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({
          theme: "custom",
          custom_theme: {
            accent: "#123456",
            signal: "#22aa55",
          },
        }),
      });
    });

    fireEvent.click(screen.getByRole("button", { name: "Reset colors" }));
    expect(document.documentElement.style.getPropertyValue("--theme-accent")).toBe("#32d3ff");
    expect(document.documentElement.style.getPropertyValue("--theme-signal")).toBe("#ff4fc3");
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({
          theme: "custom",
          custom_theme: {
            accent: "#32d3ff",
            signal: "#ff4fc3",
          },
        }),
      });
    });
  });

  it("presents the unified New Capture shortcut as the primary launcher", async () => {
    render(<Preferences />);

    const recorder = await screen.findByRole("button", { name: "New Capture" });
    expect(recorder).toHaveTextContent("Ctrl");
    expect(recorder).toHaveTextContent("Shift");
    expect(recorder).toHaveTextContent("Space");
  });

  it("separates recording toggles from the selects above them", async () => {
    render(<Preferences />);

    const mono = await screen.findByRole("checkbox", {
      name: "Export recording audio in mono",
    });
    const showCursor = screen.getByRole("checkbox", {
      name: "Show cursor in recordings",
    });
    const showClicks = screen.getByRole("checkbox", {
      name: "Show clicks in recordings",
    });

    expect(mono.closest("label")).toHaveClass("recording-setting-after-select");
    expect(showCursor.closest("label")).toHaveClass(
      "recording-setting-after-select",
      "recording-behavior-toggle",
    );
    expect(showClicks.closest("label")).toHaveClass("recording-behavior-toggle");
  });

  it("shows the installed version and offers a manual update check", async () => {
    render(<Preferences />);

    expect(await screen.findByText("Version 0.1.0")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Check Now" }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("check_for_updates"));
  });
});
