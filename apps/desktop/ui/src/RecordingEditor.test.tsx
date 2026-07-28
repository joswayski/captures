import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

import { RecordingEditor } from "./App";
import { editorCropAfterDrag, recordingFilenameError } from "./lib/recordingEditor";
import type {
  AppSettings,
  RecordingArtifact,
  RecordingTimelinePreview,
} from "./types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => false,
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(),
  listen: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
  message: vi.fn(),
}));

const artifact: RecordingArtifact = {
  id: "recording-1",
  kind: "video",
  path: "/Users/josevalerio/Captures/Captures_1140x692.mp4",
  media_url: "captures-capture://localhost/media/recording-1",
  poster_url: "captures-capture://localhost/poster/recording-1",
  mime_type: "video/mp4",
  duration_ms: 8_750,
  width: 1_140,
  height: 692,
  size_bytes: 4_200_000,
  dropped_frames: 0,
  has_system_audio: false,
  has_microphone_audio: false,
  created_at: "2026-07-26T16:45:01Z",
  target: { type: "display", display_id: "display-1" },
  missing: false,
};

const timeline: RecordingTimelinePreview = {
  url: "captures-capture://localhost/timeline/recording-1",
  frame_count: 12,
  frame_width: 160,
  frame_height: 90,
  sprite_width: 1_920,
  sprite_height: 90,
};

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

describe("RecordingEditor", () => {
  const eventHandlers = new Map<string, (event: { payload: unknown }) => void>();

  beforeEach(() => {
    window.history.replaceState({}, "", "/?view=recording-editor&artifact_id=recording-1");
    eventHandlers.clear();
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      eventHandlers.set(event, handler as (event: { payload: unknown }) => void);
      return () => undefined;
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_recording_artifact") return artifact;
      if (command === "get_settings") return settings;
      if (command === "prepare_recording_timeline_preview") return timeline;
      if (command === "start_recording_export") return "export-1";
      if (command === "cancel_recording_export" || command === "reveal_recording_artifact") {
        return undefined;
      }
      throw new Error(`unexpected command: ${command}`);
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("uses the real media aspect ratio, a 12-frame track, precise trim labels, and no empty audio card", async () => {
    const { container } = render(<RecordingEditor />);

    expect(await screen.findByRole("heading", { name: "Edit recording" })).toBeInTheDocument();
    const preview = container.querySelector<HTMLElement>(".recording-preview-media");
    expect(preview).toHaveStyle({ aspectRatio: "1140 / 692" });
    expect(container.querySelectorAll(".timeline-filmstrip i")).toHaveLength(12);
    expect(screen.getByRole("slider", { name: "Trim start" })).toHaveAttribute(
      "aria-valuetext",
      "0:00.000",
    );
    expect(screen.getByRole("slider", { name: "Trim end" })).toHaveAttribute(
      "aria-valuetext",
      "0:08.750",
    );
    const track = container.querySelector<HTMLElement>(".timeline-track");
    expect(track).not.toBeNull();
    track!.setPointerCapture = vi.fn();
    vi.spyOn(track!, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 1_000,
      bottom: 78,
      width: 1_000,
      height: 78,
      toJSON: () => undefined,
    });
    fireEvent.pointerDown(track!, { pointerId: 1, clientX: 500 });
    expect(container.querySelector(".timeline-playhead")).toHaveStyle({ left: "50%" });
    fireEvent.keyDown(screen.getByRole("slider", { name: "Trim start" }), { key: "ArrowRight" });
    expect(screen.getByRole("slider", { name: "Trim start" })).toHaveAttribute(
      "aria-valuetext",
      "0:00.001",
    );
    expect(screen.queryByRole("heading", { name: "Audio" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("checkbox", { name: "Crop recording" }));
    expect(screen.getAllByRole("button", { name: /Resize crop/ })).toHaveLength(8);
    fireEvent.click(screen.getByRole("button", { name: "100%" }));
    expect(preview).toHaveStyle({ width: "1140px", height: "692px" });

    const video = container.querySelector<HTMLVideoElement>("video");
    expect(video).not.toBeNull();
    const play = vi.spyOn(video!, "play").mockResolvedValue();
    fireEvent.click(screen.getByRole("button", { name: "Play preview" }));
    expect(play).toHaveBeenCalledOnce();
  });

  it("defaults to preserve quality and saves a safe editable filename from the sticky footer", async () => {
    render(<RecordingEditor />);

    const filename = await screen.findByRole("textbox", { name: "Saved filename" });
    expect(filename).toHaveValue("Captures_1140x692-edited");
    expect(screen.getByRole("combobox", { name: "Save quality" })).toHaveTextContent(
      "Preserve quality",
    );
    expect(screen.getByLabelText("Save location")).toHaveTextContent(
      "/Users/josevalerio/Captures",
    );
    expect(screen.getByText("Ready to save beside the source recording.")).toBeInTheDocument();

    fireEvent.change(filename, { target: { value: "Demo recording" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_recording_export", {
        request: expect.objectContaining({
          artifact_id: artifact.id,
          file_stem: "Demo recording",
          destination_directory: "/Users/josevalerio/Captures",
          edit: expect.objectContaining({ trim_start_ms: 0, trim_end_ms: null }),
          export: expect.objectContaining({ format: "mp4", quality: "preserve" }),
        }),
      });
    });

    await act(async () => {
      eventHandlers.get("recording-export-complete")?.({
        payload: {
          export_id: "export-1",
          artifact: { ...artifact, id: "saved-1", size_bytes: 40_700 },
          finder_error: null,
        },
      });
    });
    expect(screen.getByText("Saved 40.7 KB (40,700 bytes).")).toHaveClass("recording-save-success");
    expect(screen.queryByText(/Reveal Export/)).not.toBeInTheDocument();
  });

  it("changes the local destination and accepts KB, MB, or GB maximum-size units", async () => {
    vi.mocked(open).mockResolvedValue("/Users/josevalerio/Desktop/Exports");
    render(<RecordingEditor />);

    fireEvent.click(await screen.findByRole("button", { name: "Change save location" }));
    await waitFor(() => {
      expect(screen.getByLabelText("Save location")).toHaveTextContent(
        "/Users/josevalerio/Desktop/Exports",
      );
    });
    expect(screen.getByText("Ready to save in the selected folder.")).toBeInTheDocument();

    const qualityMode = screen.getByRole("combobox", { name: "Save quality" });
    fireEvent.click(qualityMode);
    fireEvent.click(screen.getByRole("option", { name: /Maximum file size/ }));
    const maximum = screen.getByRole("spinbutton", { name: "Maximum file size" });
    fireEvent.change(maximum, { target: { value: "10" } });
    const unit = screen.getByRole("combobox", { name: "File size unit" });
    fireEvent.click(unit);
    fireEvent.click(screen.getByRole("option", { name: "GB" }));
    expect(maximum).toHaveValue(0.01);

    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_recording_export", {
        request: expect.objectContaining({
          destination_directory: "/Users/josevalerio/Desktop/Exports",
          export: expect.objectContaining({ max_size_bytes: 10_000_000 }),
        }),
      });
    });
  });

  it("shows a GIF audio warning only when the source actually recorded audio", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_recording_artifact") {
        return { ...artifact, has_system_audio: true };
      }
      if (command === "get_settings") return settings;
      if (command === "prepare_recording_timeline_preview") return timeline;
      throw new Error(`unexpected command: ${command}`);
    });
    render(<RecordingEditor />);

    fireEvent.click(await screen.findByRole("button", { name: "Animated GIF" }));
    expect(screen.getByRole("heading", { name: "Audio" })).toBeInTheDocument();
    expect(screen.getByText("GIFs do not include recorded audio.")).toBeInTheDocument();
  });

  it("treats an explicit save cancellation as status rather than an error", async () => {
    render(<RecordingEditor />);

    fireEvent.click(await screen.findByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(invoke).toHaveBeenCalledWith("cancel_recording_export", { exportId: "export-1" });

    await act(async () => {
      eventHandlers.get("recording-export-failed")?.({
        payload: {
          export_id: "export-1",
          message: "media operation cancelled",
          cancelled: true,
        },
      });
    });
    expect(screen.getByText("Save cancelled.")).not.toHaveClass("recording-save-success");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});

describe("recording editor geometry", () => {
  it("moves and resizes crop bounds without escaping the source", () => {
    expect(editorCropAfterDrag(
      { x: 100, y: 50, width: 400, height: 200 },
      "move",
      { x: 900, y: 900 },
      { width: 1_140, height: 692 },
      false,
    )).toEqual({ x: 740, y: 492, width: 400, height: 200 });

    const resized = editorCropAfterDrag(
      { x: 100, y: 50, width: 400, height: 200 },
      "se",
      { x: 120, y: 20 },
      { width: 1_140, height: 692 },
      true,
    );
    expect(resized.width / resized.height).toBeCloseTo(2, 1);
    expect(resized.x + resized.width).toBeLessThanOrEqual(1_140);
    expect(resized.y + resized.height).toBeLessThanOrEqual(692);
  });

  it("rejects empty, traversing, and reserved save names", () => {
    expect(recordingFilenameError("")).toMatch(/filename/i);
    expect(recordingFilenameError("../escape")).toMatch(/filename/i);
    expect(recordingFilenameError("CON")).toMatch(/filename/i);
    expect(recordingFilenameError("Demo recording")).toBe("");
  });
});
