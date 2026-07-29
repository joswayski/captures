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
  auto_copy_to_clipboard: true,
  launch_at_login: false,
  last_screen_permission_request_id: null,
  pending_capture_after_restart: null,
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
