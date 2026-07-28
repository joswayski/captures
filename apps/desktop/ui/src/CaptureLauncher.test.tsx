import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";

import { CaptureLauncher, RecordingSavedNotice } from "./App";
import type { AppSettings } from "./types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => false,
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(),
  listen: vi.fn(async () => () => undefined),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
  message: vi.fn(),
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

describe("CaptureLauncher", () => {
  beforeEach(() => {
    window.history.replaceState({}, "", "/?view=launcher");
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_settings") return settings;
      if (
        command === "start_capture_from_launcher"
        || command === "start_recording_from_launcher"
        || command === "open_capture_history"
        || command === "open_captures_folder"
        || command === "open_preferences"
      ) {
        return undefined;
      }
      throw new Error(`unexpected command: ${command}`);
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("groups screenshots and recording in one actionable launcher", async () => {
    render(<CaptureLauncher />);

    expect(screen.getByRole("heading", { name: "Capture" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Record" })).toBeInTheDocument();
    expect(await screen.findByLabelText("Shortcut Ctrl+Shift+4")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Region/ }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_capture_from_launcher", { mode: "region" });
    });

    await waitFor(() => {
      expect(screen.getByRole("button", { name: /Screen recording/ })).toBeEnabled();
    });
    fireEvent.click(screen.getByRole("button", { name: /Screen recording/ }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_recording_from_launcher");
    });

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "History" })).toBeEnabled();
    });
    fireEvent.click(screen.getByRole("button", { name: "History" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("open_capture_history");
    });
  });
});

describe("RecordingSavedNotice", () => {
  beforeEach(() => {
    window.history.replaceState(
      {},
      "",
      "/?view=recording-saved&artifact_id=recording-1&notice_id=7e3191ca-8596-4d22-a6e1-4b57a64f00cb",
    );
    vi.mocked(invoke).mockResolvedValue(undefined);
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("reassures the user and can reveal the saved recording", async () => {
    render(<RecordingSavedNotice />);

    expect(screen.getByText("Recording saved")).toBeInTheDocument();
    expect(screen.getByText("Saved to your Captures folder.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show in Finder" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("reveal_recording_artifact", {
        artifactId: "recording-1",
      });
      expect(invoke).toHaveBeenCalledWith("dismiss_recording_saved_notice", {
        noticeId: "7e3191ca-8596-4d22-a6e1-4b57a64f00cb",
      });
    });
  });
});
