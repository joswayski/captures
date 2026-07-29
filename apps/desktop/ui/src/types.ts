import type { ColorTheme } from "../../../../shared/themes";

export type { ColorTheme };

export type CaptureMode = "region" | "window" | "display";
export type RecordingKind = "video" | "gif";
export type RecordingState =
  | "selecting"
  | "countdown"
  | "recording"
  | "paused"
  | "finalizing"
  | "ready"
  | "editor"
  | "failed"
  | "discarded";
export type MaxResolution = "original" | "p1080" | "p720";
export type ClipboardCopyStatus = "skipped" | "pending" | "copied" | "failed";

export interface DisplayDescriptor {
  id: string;
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  scale_factor: number;
  is_primary: boolean;
}

export interface WindowDescriptor {
  id: string;
  title: string;
  app_name: string | null;
  z_order: number;
  x: number;
  y: number;
  width: number;
  height: number;
  display_id: string;
}

export interface ActiveSession {
  id: string;
  mode: CaptureMode;
  display: DisplayDescriptor;
  window_coordinate_scale: number;
  window_corner_radius: number;
  snapshot_url: string;
  windows: WindowDescriptor[];
}

export interface AppSettings {
  settings_schema_version?: number;
  theme: ColorTheme;
  output_directory: string;
  new_capture_shortcut: string;
  region_shortcut: string;
  window_shortcut: string;
  display_shortcut: string;
  auto_copy_to_clipboard: boolean;
  launch_at_login: boolean;
  last_screen_permission_request_id: string | null;
  pending_capture_after_restart: CaptureMode | null;
  recording: RecordingSettings;
}

export interface RecordingSettings {
  video_shortcut: string;
  gif_shortcut: string;
  video_fps: number;
  video_max_resolution: MaxResolution;
  gif_fps: number;
  gif_max_width: number;
  gif_max_colors: number;
  countdown_seconds: number;
  show_cursor: boolean;
  capture_system_audio: boolean;
  microphone_device_id: string | null;
  mono_audio: boolean;
  highlight_clicks: boolean;
  show_keystrokes: boolean;
  open_editor_after_recording: boolean;
}

export type RecordingTarget =
  | { type: "display"; display_id: string }
  | { type: "region"; display_id: string; rect: { x: number; y: number; width: number; height: number } }
  | { type: "window"; window_id: string };

export interface RecordingOptions {
  kind: RecordingKind;
  target: RecordingTarget;
  frames_per_second: number;
  max_resolution: MaxResolution;
  countdown_seconds: number;
  show_cursor: boolean;
  highlight_clicks: boolean;
  show_keystrokes: boolean;
  audio: {
    capture_system_audio: boolean;
    microphone_device_id: string | null;
    mono_output: boolean;
    system_volume_percent: number;
    microphone_volume_percent: number;
    microphone_muted: boolean;
  };
  gif: {
    max_width: number;
    max_colors: number;
    optimize: boolean;
  };
}

export interface RecordingSelectionSession {
  id: string;
  kind: RecordingKind;
  initial_mode: "screenshot" | "recording";
  recording_available: boolean;
  recording_capabilities: {
    system_audio: boolean;
    microphone: boolean;
    cursor_control: boolean;
    click_highlights: boolean;
    controls_excluded: boolean;
  };
  display: DisplayDescriptor;
  window_coordinate_scale: number;
  window_corner_radius: number;
  snapshot_url: string;
  windows: WindowDescriptor[];
}

export interface RecordingSessionSnapshot {
  id: string;
  state: RecordingState;
  options: RecordingOptions;
  elapsed_ms: number;
  countdown_remaining_seconds: number | null;
  warning: string | null;
  error: string | null;
}

export interface RecordingDraftManifest {
  session_id: string;
  created_at_ms: number;
  updated_at_ms: number;
  state: RecordingState;
  options: RecordingOptions;
  segments: Array<{
    index: number;
    duration_ms: number;
    size_bytes: number;
    dropped_frames: number;
    complete: boolean;
  }>;
  final_path: string | null;
  last_error: string | null;
}

export interface AudioDevice {
  id: string;
  name: string;
  kind: "default" | "microphone";
  is_default: boolean;
}

export interface RecordingArtifact {
  id: string;
  kind: RecordingKind;
  path: string;
  media_url: string;
  poster_url: string;
  mime_type: string;
  duration_ms: number;
  width: number;
  height: number;
  size_bytes: number;
  dropped_frames: number;
  has_system_audio: boolean;
  has_microphone_audio: boolean;
  created_at: string;
  target: RecordingTarget;
  missing: boolean;
}

export interface EditSpec {
  trim_start_ms: number;
  trim_end_ms: number | null;
  crop: { x: number; y: number; width: number; height: number } | null;
  output_width: number | null;
  output_height: number | null;
  audio: {
    system_volume: number;
    microphone_volume: number;
    mute_system_audio: boolean;
    mute_microphone: boolean;
    mono_output: boolean;
    source_has_system_audio: boolean;
    source_has_microphone_audio: boolean;
  };
}

export interface ExportSpec {
  format: "mp4" | "gif" | "web_m";
  quality: "preserve" | "high" | "standard" | "small";
  max_size_bytes: number | null;
  frames_per_second: number | null;
  gif_max_colors: number | null;
}

export interface RecordingTimelinePreview {
  url: string;
  frame_count: number;
  frame_width: number;
  frame_height: number;
  sprite_width: number;
  sprite_height: number;
}

export interface ExportProgress {
  stage: "preparing" | "encoding" | "verifying" | "complete" | "cancelled" | "failed";
  completed_per_mille: number;
  attempt: number;
  message: string | null;
}

export interface CaptureArtifact {
  id: string;
  path: string | null;
  preview_url: string;
  full_url: string;
  width: number;
  height: number;
  size_bytes: number;
  created_at: string;
  mode: CaptureMode;
  history_saved: boolean;
  clipboard_copy_status: ClipboardCopyStatus;
}

export interface ArtifactDragPayload {
  path: string;
  icon_path: string;
}

interface ArtifactSummaryBase {
  id: string;
  width: number;
  height: number;
  size_bytes: number;
  created_at: string;
}

export interface ScreenshotArtifactSummary extends ArtifactSummaryBase {
  kind: "screenshot";
  preview_url: string;
  full_url: string;
  mode: CaptureMode;
}

export interface RecordingArtifactSummary extends ArtifactSummaryBase {
  kind: "video" | "gif";
  poster_url: string;
  media_url: string;
  saved_path: string;
  mime_type: string;
  duration_ms: number;
  dropped_frames: number;
  has_system_audio: boolean;
  has_microphone_audio: boolean;
  target: RecordingTarget;
  missing: boolean;
}

export type ArtifactSummary = ScreenshotArtifactSummary | RecordingArtifactSummary;

export type HistoryEntry = ArtifactSummary;

export interface ClipboardState {
  revision: number;
  artifact_id: string | null;
}

export interface ViewerActivationState {
  artifact_id: string;
  active: boolean;
}

export interface ThumbnailPointerPosition {
  x: number;
  y: number;
  inside: boolean;
}

interface UpdateVersionInfo {
  current_version: string;
  current_display_version: string;
}

export type UpdateStatus =
  | (UpdateVersionInfo & { state: "idle" | "checking" | "up_to_date" })
  | (UpdateVersionInfo & {
      state: "available";
      version: string;
      display_version: string;
      notes: string | null;
      installable: boolean;
      manual_download_url: string | null;
    })
  | (UpdateVersionInfo & {
      state: "downloading";
      version: string;
      display_version: string;
      downloaded: number;
      total: number | null;
    })
  | (UpdateVersionInfo & { state: "error"; message: string });
