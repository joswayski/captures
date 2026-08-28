import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
  saved_path: "/Users/josevalerio/Captures/Captures_1140x692.mp4",
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

const historyOnlyArtifact: RecordingArtifact = {
  ...artifact,
  id: "recording-history-only",
  path: "/Users/josevalerio/Library/Application Support/app.captures.desktop/history/recording-history-only/media.mp4",
  saved_path: null,
  created_at: "2026-07-26T16:45:01.250Z",
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
  feedback_shortcut: "Ctrl+Shift+F",
  auto_copy_to_clipboard: true,
  auto_start_on_selection: false,
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

describe("RecordingEditor", () => {
  const eventHandlers = new Map<string, (event: { payload: unknown }) => void>();

  beforeEach(() => {
    window.history.replaceState({}, "", "/?view=recording-editor&artifact_id=recording-1");
    eventHandlers.clear();
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      eventHandlers.set(event, handler as (event: { payload: unknown }) => void);
      return () => undefined;
    });
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_recording_artifact") return artifact;
      if (command === "get_settings") return settings;
      if (command === "prepare_recording_timeline_preview") return timeline;
      if (command === "start_recording_export") return "export-1";
      if (command === "estimate_recording_export") {
        const request = args as {
          edit?: { output_height?: number | null };
          export?: { quality?: string };
        } | undefined;
        if (request?.export?.quality && request.export.quality !== "preserve") {
          return { sizeBytes: 1_680_000, exact: false };
        }
        if (
          typeof request?.edit?.output_height === "number"
          && request.edit.output_height < artifact.height
        ) {
          return { sizeBytes: 1_050_000, exact: false };
        }
        return { sizeBytes: artifact.size_bytes, exact: true };
      }
      if (command === "preview_recording_export") return { beforePng: [1, 2], afterPng: [3, 4] };
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
    expect(screen.getByRole("combobox", { name: "Output resolution" })).toHaveTextContent(
      "Original — 1140 × 692",
    );
    expect(screen.queryByText("Final video size")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("checkbox", { name: "Crop recording" }));
    expect(screen.getAllByRole("button", { name: /Resize crop/ })).toHaveLength(8);
    fireEvent.click(screen.getByRole("button", { name: "100%" }));
    expect(preview).toHaveStyle({ width: "1140px", height: "692px" });
    expect(container.querySelector(".preview-size-segmented .capture-segmented-indicator"))
      .not.toBeNull();
    expect(screen.getByRole("button", { name: "Loop preview" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );

    const video = container.querySelector<HTMLVideoElement>("video");
    expect(video).not.toBeNull();
    expect(preview?.querySelector(".recording-preview-overlay-play")).toBeInTheDocument();
    expect(container.querySelector(".recording-preview-toolbar .recording-preview-overlay-play")).not.toBeInTheDocument();
    const play = vi.spyOn(video!, "play").mockResolvedValue();
    fireEvent.click(screen.getByRole("button", { name: "Play preview" }));
    expect(play).toHaveBeenCalledOnce();
  });

  it("keeps playback inside the trim range, seeks on handle clicks without nudging edges, and optionally loops", async () => {
    const { container } = render(<RecordingEditor />);

    expect(await screen.findByRole("heading", { name: "Edit recording" })).toBeInTheDocument();
    const video = container.querySelector<HTMLVideoElement>("video");
    expect(video).not.toBeNull();
    let paused = true;
    Object.defineProperty(video!, "paused", {
      configurable: true,
      get: () => paused,
    });
    const play = vi.spyOn(video!, "play").mockImplementation(async () => {
      paused = false;
    });
    const pause = vi.spyOn(video!, "pause").mockImplementation(() => {
      paused = true;
    });
    const trimStart = screen.getByRole("slider", { name: "Trim start" });
    const trimEnd = screen.getByRole("slider", { name: "Trim end" });
    trimStart.setPointerCapture = vi.fn();
    trimEnd.setPointerCapture = vi.fn();
    const track = container.querySelector<HTMLElement>(".timeline-track");
    expect(track).not.toBeNull();
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

    fireEvent.keyDown(trimStart, { key: "PageUp" });
    fireEvent.keyDown(trimStart, { key: "PageUp" });
    fireEvent.keyDown(trimEnd, { key: "PageDown" });
    fireEvent.keyDown(trimEnd, { key: "PageDown" });
    expect(trimStart).toHaveAttribute("aria-valuetext", "0:02.000");
    expect(trimEnd).toHaveAttribute("aria-valuetext", "0:06.750");

    // Click (and sub-threshold wiggle) seeks playhead without moving the trim edge.
    const startClientX = (2_000 / artifact.duration_ms) * 1_000;
    video!.currentTime = 4;
    fireEvent.seeked(video!);
    fireEvent.pointerDown(trimStart, { pointerId: 1, clientX: startClientX });
    fireEvent.pointerMove(trimStart, { pointerId: 1, clientX: startClientX + 2 });
    expect(video!.currentTime).toBe(2);
    expect(trimStart).toHaveAttribute("aria-valuetext", "0:02.000");
    expect(container.querySelector(".timeline-playhead")).toHaveStyle({
      left: `${2_000 / artifact.duration_ms * 100}%`,
    });
    fireEvent.pointerUp(trimStart, { pointerId: 1 });
    expect(trimStart).toHaveAttribute("aria-valuetext", "0:02.000");

    const endClientX = (6_750 / artifact.duration_ms) * 1_000;
    fireEvent.pointerDown(trimEnd, { pointerId: 2, clientX: endClientX });
    fireEvent.pointerMove(trimEnd, { pointerId: 2, clientX: endClientX - 1 });
    expect(video!.currentTime).toBe(6.75);
    expect(trimEnd).toHaveAttribute("aria-valuetext", "0:06.750");
    expect(container.querySelector(".timeline-playhead")).toHaveStyle({
      left: `${6_750 / artifact.duration_ms * 100}%`,
    });
    fireEvent.pointerUp(trimEnd, { pointerId: 2 });
    expect(trimEnd).toHaveAttribute("aria-valuetext", "0:06.750");

    // Dragging past the threshold moves the trim edge, then restore for playback checks.
    fireEvent.pointerDown(trimStart, { pointerId: 3, clientX: startClientX });
    fireEvent.pointerMove(trimStart, { pointerId: 3, clientX: startClientX + 50 });
    expect(trimStart).toHaveAttribute("aria-valuetext", "0:02.438");
    fireEvent.pointerUp(trimStart, { pointerId: 3 });
    fireEvent.pointerDown(trimStart, { pointerId: 4, clientX: startClientX + 50 });
    fireEvent.pointerMove(trimStart, { pointerId: 4, clientX: startClientX });
    fireEvent.pointerUp(trimStart, { pointerId: 4 });
    expect(trimStart).toHaveAttribute("aria-valuetext", "0:02.000");

    fireEvent.click(screen.getByRole("button", { name: "Play preview" }));
    await waitFor(() => expect(play).toHaveBeenCalledOnce());
    expect(video!.currentTime).toBe(2);
    fireEvent.play(video!);

    video!.currentTime = 7;
    fireEvent.timeUpdate(video!);
    expect(pause).toHaveBeenCalledOnce();
    expect(video!.currentTime).toBe(6.75);
    expect(container.querySelector(".timeline-playhead")).toHaveStyle({
      left: `${6_750 / artifact.duration_ms * 100}%`,
    });

    const loop = screen.getByRole("button", { name: "Loop preview" });
    fireEvent.click(loop);
    expect(loop).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(screen.getByRole("button", { name: "Play preview" }));
    await waitFor(() => expect(play).toHaveBeenCalledTimes(2));
    video!.currentTime = 7;
    fireEvent.timeUpdate(video!);
    expect(pause).toHaveBeenCalledOnce();
    expect(video!.currentTime).toBe(2);
    expect(container.querySelector(".timeline-playhead")).toHaveStyle({
      left: `${2_000 / artifact.duration_ms * 100}%`,
    });
  });

  it("updates the playhead every animation frame while the preview is playing", async () => {
    const rafQueue: FrameRequestCallback[] = [];
    const raf = vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
      rafQueue.push(callback);
      return rafQueue.length;
    });
    vi.spyOn(window, "cancelAnimationFrame").mockImplementation(() => undefined);

    const { container } = render(<RecordingEditor />);
    expect(await screen.findByRole("heading", { name: "Edit recording" })).toBeInTheDocument();
    const video = container.querySelector<HTMLVideoElement>("video");
    expect(video).not.toBeNull();
    let paused = true;
    Object.defineProperty(video!, "paused", {
      configurable: true,
      get: () => paused,
    });
    vi.spyOn(video!, "play").mockImplementation(async () => {
      paused = false;
    });

    fireEvent.click(screen.getByRole("button", { name: "Play preview" }));
    fireEvent.play(video!);
    await waitFor(() => expect(raf).toHaveBeenCalled());

    video!.currentTime = 1.25;
    const pending = rafQueue.splice(0);
    await act(async () => {
      for (const callback of pending) callback(performance.now());
    });
    expect(container.querySelector(".timeline-playhead")).toHaveStyle({
      left: `${1_250 / artifact.duration_ms * 100}%`,
    });

    video!.currentTime = 3.5;
    const next = rafQueue.splice(0);
    await act(async () => {
      for (const callback of next) callback(performance.now());
    });
    expect(container.querySelector(".timeline-playhead")).toHaveStyle({
      left: `${3_500 / artifact.duration_ms * 100}%`,
    });
  });

  it("updates the source by default and can explicitly save a copy", async () => {
    render(<RecordingEditor />);

    const filename = await screen.findByRole("textbox", { name: "Saved filename" });
    expect(filename).toHaveValue("Captures_1140x692");
    expect(filename).toBeEnabled();
    fireEvent.focus(filename);
    expect((filename as HTMLInputElement).selectionStart).toBe(0);
    expect((filename as HTMLInputElement).selectionEnd).toBe("Captures_1140x692".length);
    expect(screen.getByRole("checkbox", { name: "Save as new file" })).not.toBeChecked();
    expect(screen.getByRole("combobox", { name: "Format" })).toHaveTextContent(".mp4");
    expect(filename.closest(".recording-filename-input"))
      .toContainElement(screen.getByRole("combobox", { name: "Format" }));
    expect(screen.queryByRole("radio")).not.toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Save quality" })).toHaveTextContent(
      "Preserve quality",
    );
    expect(screen.getByLabelText("Save location")).toHaveTextContent(
      "/Users/josevalerio/Captures",
    );
    expect(screen.getByText("Saving to")).toBeInTheDocument();
    expect(screen.queryByText("Ready to save.")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Change save location" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Show in Folder" })).not.toBeInTheDocument();
    const save = screen.getByRole("button", { name: "Save" });
    expect(save).toBeEnabled();
    expect(screen.queryByText(/already saved/i)).not.toBeInTheDocument();
    expect(screen.queryByText("Unsaved changes.")).not.toBeInTheDocument();

    fireEvent.change(filename, { target: { value: "Renamed recording" } });
    expect(save).toBeEnabled();
    fireEvent.click(save);
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_recording_export", {
        request: expect.objectContaining({
          artifact_id: artifact.id,
          file_stem: "Renamed recording",
          destination_directory: "/Users/josevalerio/Captures",
          overwrite_source: true,
        }),
      });
    });

    await act(async () => {
      eventHandlers.get("recording-export-complete")?.({
        payload: {
          export_id: "export-1",
          artifact: { ...artifact, size_bytes: 40_700 },
          reveal_error: null,
        },
      });
    });
    expect(screen.getByText("Video saved — 40.7 KB.").closest(".recording-save-toast"))
      .toHaveClass("success");
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Saved" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show in Folder" }));
    expect(invoke).toHaveBeenCalledWith("reveal_recording_artifact", {
      artifactId: artifact.id,
    });

    // Reset to the original name so Save as new file can apply the -edited suffix.
    fireEvent.change(filename, { target: { value: "Captures_1140x692" } });
    fireEvent.click(screen.getByRole("checkbox", { name: "Save as new file" }));
    expect(filename).toHaveValue("Captures_1140x692-edited");
    expect(filename).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Show in Folder" })).not.toBeInTheDocument();
    fireEvent.change(filename, { target: { value: "Demo recording" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_recording_export", {
        request: expect.objectContaining({
          artifact_id: artifact.id,
          file_stem: "Demo recording",
          destination_directory: "/Users/josevalerio/Captures",
          overwrite_source: false,
          edit: expect.objectContaining({ trim_start_ms: 0, trim_end_ms: null }),
          export: expect.objectContaining({ format: "mp4", quality: "preserve" }),
        }),
      });
    });
  });

  it("defaults history-only recordings to the Captures folder, not media.mp4 recovery paths", async () => {
    window.history.replaceState(
      {},
      "",
      "/?view=recording-editor&artifact_id=recording-history-only",
    );
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_recording_artifact") return historyOnlyArtifact;
      if (command === "get_settings") return settings;
      if (command === "prepare_recording_timeline_preview") return timeline;
      if (command === "start_recording_export") return "export-1";
      throw new Error(`unexpected command: ${command}`);
    });

    render(<RecordingEditor />);

    const filename = await screen.findByRole("textbox", { name: "Saved filename" });
    const stem = (filename as HTMLInputElement).value;
    expect(stem).toMatch(/^Captures_\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2}_\d{3}$/);
    expect(stem).not.toBe("media");
    expect(screen.getByLabelText("Save location")).toHaveTextContent(
      "/Users/josevalerio/Captures",
    );
    expect(screen.getByLabelText("Save location")).not.toHaveTextContent("history");
    expect(screen.getByRole("checkbox", { name: "Save as new file" })).not.toBeChecked();

    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_recording_export", {
        request: expect.objectContaining({
          artifact_id: historyOnlyArtifact.id,
          destination_directory: "/Users/josevalerio/Captures",
          file_stem: stem,
          overwrite_source: true,
        }),
      });
    });
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
    expect(screen.queryByText("Ready to save.")).not.toBeInTheDocument();

    const qualityMode = screen.getByRole("combobox", { name: "Save quality" });
    fireEvent.click(qualityMode);
    fireEvent.click(screen.getByRole("option", { name: /Maximum file size/ }));
    const maximum = screen.getByRole("spinbutton", { name: "Maximum file size" });
    fireEvent.change(maximum, { target: { value: "10" } });
    const unit = screen.getByRole("combobox", { name: "File size unit" });
    fireEvent.click(unit);
    fireEvent.click(screen.getByRole("option", { name: "GB" }));
    expect(maximum).toHaveValue(0.01);

    fireEvent.change(screen.getByRole("textbox", { name: "Saved filename" }), {
      target: { value: "Moved recording" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_recording_export", {
        request: expect.objectContaining({
          destination_directory: "/Users/josevalerio/Desktop/Exports",
          file_stem: "Moved recording",
          overwrite_source: true,
          export: expect.objectContaining({ max_size_bytes: 10_000_000 }),
        }),
      });
    });
  });

  it("uses compress quality presets instead of a notched slider", async () => {
    render(<RecordingEditor />);
    await screen.findByRole("heading", { name: "Edit recording" });

    fireEvent.click(screen.getByRole("combobox", { name: "Save quality" }));
    fireEvent.click(screen.getByRole("option", { name: /Compress/ }));
    const quality = screen.getByRole("combobox", { name: "Compression quality" });
    expect(quality).toHaveTextContent("High");
    expect(screen.queryByRole("slider", { name: "Compression quality" }))
      .not.toBeInTheDocument();

    fireEvent.click(quality);
    expect(screen.getByRole("option", { name: /Tiny/ })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Smaller/ })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Balanced/ })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /High/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: /Balanced/ }));
    expect(quality).toHaveTextContent("Balanced");
    fireEvent.change(screen.getByRole("textbox", { name: "Saved filename" }), {
      target: { value: "Balanced recording" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_recording_export", {
        request: expect.objectContaining({
          export: expect.objectContaining({
            quality: "standard",
          }),
        }),
      });
    });
  });

  it("maps GIF compress quality to palette size instead of a separate color control", async () => {
    render(<RecordingEditor />);
    await screen.findByRole("heading", { name: "Edit recording" });

    fireEvent.click(await screen.findByRole("combobox", { name: "Format" }));
    fireEvent.click(screen.getByRole("option", { name: "GIF" }));
    expect(screen.getByRole("combobox", { name: "Format" })).toHaveTextContent(".gif");
    expect(screen.queryByRole("combobox", { name: "GIF palette" })).not.toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Compression comparison" }))
      .toBeInTheDocument();
    const quality = screen.getByRole("combobox", { name: "Compression quality" });
    fireEvent.click(quality);
    fireEvent.click(screen.getByRole("option", { name: /Tiny/ }));
    fireEvent.change(screen.getByRole("textbox", { name: "Saved filename" }), {
      target: { value: "Tiny gif" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_recording_export", {
        request: expect.objectContaining({
          export: expect.objectContaining({
            format: "gif",
            quality: "tiny",
            gif_max_colors: 64,
          }),
        }),
      });
    });
  });

  it("estimates the saved size in the background and shows the reduction for Compress", async () => {
    render(<RecordingEditor />);
    await screen.findByRole("heading", { name: "Edit recording" });

    fireEvent.click(screen.getByRole("combobox", { name: "Save quality" }));
    fireEvent.click(screen.getByRole("option", { name: /Compress/ }));

    // 1.68 MB estimate against the 4.2 MB source: ≈ value plus a −60% badge.
    await waitFor(() => {
      expect(screen.getByText("≈ 1.7 MB")).toBeInTheDocument();
    }, { timeout: 3_000 });
    expect(screen.getByText("−60%")).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("estimate_recording_export", expect.objectContaining({
      artifactId: artifact.id,
      export: expect.objectContaining({ format: "mp4", quality: "high" }),
    }));

    // Maximum mode shows the cap instead of a sampled estimate.
    fireEvent.click(screen.getByRole("combobox", { name: "Save quality" }));
    fireEvent.click(screen.getByRole("option", { name: /Maximum file size/ }));
    expect(screen.getByText("≤ 10.0 MB")).toBeInTheDocument();
  });

  it("shows a size-change percent when output resolution shrinks the pixel dimensions", async () => {
    render(<RecordingEditor />);
    await screen.findByRole("heading", { name: "Edit recording" });

    await waitFor(() => {
      expect(screen.getByTitle("Estimated saved file size for the current edits and settings"))
        .toHaveTextContent("4.2 MB");
    }, { timeout: 3_000 });
    expect(document.querySelector(".recording-output-estimate-delta")).toBeNull();

    fireEvent.click(screen.getByRole("combobox", { name: "Output resolution" }));
    expect(screen.getByRole("option", { name: /Choose exact pixel dimensions/ })).toHaveTextContent(
      "Choose exact pixel dimensions.",
    );
    fireEvent.click(screen.getByRole("option", { name: /Choose exact pixel dimensions/ }));
    const customSize = document.querySelector(".editor-number-grid.dimensions");
    expect(customSize).not.toBeNull();
    fireEvent.change(within(customSize as HTMLElement).getByRole("spinbutton", { name: "Width" }), {
      target: { value: "570" },
    });
    fireEvent.change(within(customSize as HTMLElement).getByRole("spinbutton", { name: "Height" }), {
      target: { value: "346" },
    });

    await waitFor(() => {
      expect(screen.getByText("≈ 1.1 MB")).toBeInTheDocument();
      expect(screen.getByText("−75%")).toBeInTheDocument();
    }, { timeout: 3_000 });
    expect(screen.getByText("−75%")).toHaveClass("recording-output-estimate-delta", "is-smaller");
    expect(screen.getByRole("combobox", { name: "Save quality" }))
      .toHaveTextContent("Preserve quality");
  });

  it("shows the before/after comparison in the preview when Compress is selected", async () => {
    const createObjectURL = vi.fn(() => "blob:recording-preview");
    const revokeObjectURL = vi.fn();
    Object.assign(URL, { createObjectURL, revokeObjectURL });

    const { container } = render(<RecordingEditor />);
    await screen.findByRole("heading", { name: "Edit recording" });
    const video = container.querySelector<HTMLVideoElement>("video");
    expect(video).not.toBeNull();

    expect(screen.queryByRole("button", { name: "Compare before / after" }))
      .not.toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Compression comparison" }))
      .not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("combobox", { name: "Save quality" }));
    fireEvent.click(screen.getByRole("option", { name: /Compress/ }));

    expect(screen.getByRole("group", { name: "Compression comparison" }))
      .toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "Compression preview" }))
      .not.toBeInTheDocument();
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("preview_recording_export", expect.objectContaining({
        artifactId: artifact.id,
        atMs: 0,
        export: expect.objectContaining({ format: "mp4", quality: "high" }),
      }));
    }, { timeout: 3_000 });
    await waitFor(() => {
      expect(createObjectURL).toHaveBeenCalledTimes(2);
    });
    expect(screen.getByAltText("Before compression")).toHaveAttribute("src", "blob:recording-preview");
    expect(screen.getByAltText("After compression")).toHaveAttribute("src", "blob:recording-preview");

    fireEvent.play(video!);
    expect(screen.queryByRole("group", { name: "Compression comparison" }))
      .not.toBeInTheDocument();
    fireEvent.pause(video!);
    expect(screen.getByRole("group", { name: "Compression comparison" }))
      .toBeInTheDocument();

    fireEvent.click(screen.getByRole("combobox", { name: "Save quality" }));
    fireEvent.click(screen.getByRole("option", { name: /Maximum file size/ }));
    expect(screen.getByRole("group", { name: "Compression comparison" }))
      .toBeInTheDocument();

    fireEvent.click(screen.getByRole("combobox", { name: "Save quality" }));
    fireEvent.click(screen.getByRole("option", { name: /Preserve quality/ }));
    expect(screen.queryByRole("group", { name: "Compression comparison" }))
      .not.toBeInTheDocument();
  });

  it("uses shared accessible controls for output format and recorded-audio volume", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_recording_artifact") {
        return {
          ...artifact,
          has_system_audio: true,
          has_microphone_audio: true,
        };
      }
      if (command === "get_settings") return settings;
      if (command === "prepare_recording_timeline_preview") return timeline;
      if (command === "start_recording_export") return "export-1";
      throw new Error(`unexpected command: ${command}`);
    });
    render(<RecordingEditor />);
    await screen.findByRole("heading", { name: "Edit recording" });

    const format = screen.getByRole("combobox", { name: "Format" });
    expect(format).toHaveTextContent(".mp4");
    fireEvent.click(format);
    expect(screen.getByRole("option", { name: "MP4" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("option", { name: "GIF" })).toHaveAttribute("aria-selected", "false");
    fireEvent.keyDown(format, { key: "Escape" });

    const systemVolume = screen.getByRole("slider", { name: "System audio volume" });
    const microphoneVolume = screen.getByRole("slider", { name: "Microphone volume" });
    expect(systemVolume).toHaveAttribute("aria-valuetext", "100%");
    expect(microphoneVolume).toHaveAttribute("aria-valuetext", "100%");

    fireEvent.change(systemVolume, { target: { value: "140" } });
    expect(systemVolume).toHaveAttribute("aria-valuetext", "140%");
    fireEvent.click(screen.getByRole("checkbox", { name: "System audio" }));
    expect(systemVolume).toBeDisabled();

    fireEvent.change(screen.getByRole("textbox", { name: "Saved filename" }), {
      target: { value: "Audio controls" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_recording_export", {
        request: expect.objectContaining({
          edit: expect.objectContaining({
            audio: expect.objectContaining({
              system_volume: 1.4,
              microphone_volume: 1,
              mute_system_audio: true,
              mute_microphone: false,
            }),
          }),
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
      if (command === "preview_recording_export") return { beforePng: [1, 2], afterPng: [3, 4] };
      if (command === "estimate_recording_export") return { sizeBytes: 1_680_000, exact: false };
      throw new Error(`unexpected command: ${command}`);
    });
    render(<RecordingEditor />);

    fireEvent.click(await screen.findByRole("combobox", { name: "Format" }));
    fireEvent.click(screen.getByRole("option", { name: "GIF" }));
    expect(screen.getByRole("heading", { name: "GIF settings" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Audio" })).toBeInTheDocument();
    expect(screen.getByText("GIFs do not include recorded audio.")).toBeInTheDocument();
  });

  it("treats an explicit save cancellation as status rather than an error", async () => {
    render(<RecordingEditor />);

    await screen.findByRole("textbox", { name: "Saved filename" });
    const save = screen.getByRole("button", { name: "Save" });
    const showInFolder = screen.queryByRole("button", { name: "Show in Folder" });
    expect(showInFolder).not.toBeInTheDocument();
    fireEvent.click(save);
    await waitFor(() => expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument());
    expect(save).toHaveTextContent("Save");
    expect(screen.queryByRole("button", { name: "Show in Folder" })).not.toBeInTheDocument();
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
    expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Show in Folder" })).not.toBeInTheDocument();
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
