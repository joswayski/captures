import type { AppearanceMode } from "../../../../shared/appearance";
import type { ColorTheme, CustomThemeColors } from "../../../../shared/themes";

export type { AppearanceMode, ColorTheme, CustomThemeColors };

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
  /** Measured visible corner radius in logical points, when known. */
  corner_radius?: number | null;
}

export type ScreenshotFormat = "png" | "jpeg" | "webp";
export type VideoFormat = "mp4" | "gif" | "webm";
export type MiniPreviewPlacement =
  | "bottom_left"
  | "bottom_right"
  | "top_left"
  | "top_right";

export interface ActiveSession {
  id: string;
  mode: CaptureMode;
  display: DisplayDescriptor;
  window_coordinate_scale: number;
  window_corner_radius: number;
  /** Visible display corner radius in logical points (MacBooks, etc.). */
  display_corner_radius?: number;
  /** False when the overlay shows the live desktop instead of a freeze-frame. */
  frozen?: boolean;
  snapshot_url: string;
  windows: WindowDescriptor[];
  /** Menu bar / taskbar / dock strips used only for hit-testing. */
  shell_chrome?: WindowDescriptor[];
  /** False while window enumeration is still running. Omitted means ready. */
  windows_ready?: boolean;
}

export interface AppSettings {
  settings_schema_version?: number;
  appearance: AppearanceMode;
  theme: ColorTheme;
  custom_theme: CustomThemeColors;
  output_directory: string;
  new_capture_shortcut: string;
  region_shortcut: string;
  window_shortcut: string;
  display_shortcut: string;
  auto_copy_to_clipboard: boolean;
  /** Start screenshot/recording as soon as a region is drawn or a window is picked. */
  auto_start_on_selection: boolean;
  show_mini_previews: boolean;
  mini_preview_placement: MiniPreviewPlacement;
  include_mini_previews_in_captures: boolean;
  include_recording_controls_in_captures: boolean;
  launch_at_login: boolean;
  last_screen_permission_request_id: string | null;
  pending_capture_after_restart: CaptureMode | null;
  onboarding_completed: boolean;
  screenshot_countdown_seconds: number;
  freeze_screen: boolean;
  show_cursor_in_screenshots: boolean;
  screenshot_format: ScreenshotFormat;
  /** When false, the update notice hides release notes and stays compact. */
  show_update_changelog: boolean;
  recording: RecordingSettings;
}

export interface OnboardingState {
  platform: "macos" | "windows" | "linux" | string;
  screen_recording_required: boolean;
  screen_recording_granted: boolean;
  screen_recording_can_request: boolean;
  screen_recording_requested_this_launch: boolean;
  capture_system_audio: boolean;
  microphone_enabled: boolean;
  microphone_granted: boolean;
  microphone_can_request: boolean;
}

export interface RecordingSettings {
  /** Legacy field name retained for the Record Region shortcut. */
  video_shortcut: string;
  window_shortcut: string;
  display_shortcut: string;
  gif_shortcut: string;
  video_format: VideoFormat;
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
  initial_target: CaptureMode;
  recording_available: boolean;
  recording_capabilities: {
    system_audio: boolean;
    microphone: boolean;
    cursor_control: boolean;
    click_highlights: boolean;
    controls_excluded: boolean;
    can_exclude_controls: boolean;
  };
  display: DisplayDescriptor;
  displays: DisplayDescriptor[];
  window_coordinate_scale: number;
  window_corner_radius: number;
  /** Visible display corner radius in logical points (MacBooks, etc.). */
  display_corner_radius?: number;
  /** False when the selector shows the live desktop instead of a freeze-frame. */
  frozen?: boolean;
  snapshot_url: string;
  windows: WindowDescriptor[];
  /** Menu bar / taskbar / dock strips used only for hit-testing. */
  shell_chrome?: WindowDescriptor[];
  /** False while window enumeration is still running. Omitted means ready. */
  windows_ready?: boolean;
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
  /** Permanent Captures-folder copy when the user has explicitly saved. */
  saved_path?: string | null;
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
  format: "mp4" | "gif" | "webm";
  quality: "preserve" | "highest" | "high" | "standard" | "small" | "tiny";
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

/** Where a mini-preview file drag ended, from `preview_file_drop_landing`. */
export type PreviewFileDropLanding = "preview_stack" | "app_window" | "external";

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
  /** Permanent Captures-folder path when saved; null while history-only. */
  saved_path: string | null;
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

/** Which capture artifacts still appear as layers in a given editor window. */
export interface EditorLayerPresence {
  editor_id: string;
  artifact_ids: string[];
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

export interface UpdateChangelogEntry {
  version: string;
  display_version: string;
  notes: string | null;
}

export type UpdateStatus =
  | (UpdateVersionInfo & { state: "idle" | "checking" | "up_to_date" })
  | (UpdateVersionInfo & {
      state: "available";
      version: string;
      display_version: string;
      notes: string | null;
      changelog: UpdateChangelogEntry[];
      installable: boolean;
      manual_download_url: string | null;
      download_size: number | null;
      will_close_open_captures: boolean;
    })
  | (UpdateVersionInfo & {
      state: "downloading";
      version: string;
      display_version: string;
      downloaded: number;
      total: number | null;
    })
  | (UpdateVersionInfo & {
      state: "restarting";
      version: string;
      display_version: string;
      seconds_remaining: number;
    })
  | (UpdateVersionInfo & { state: "error"; message: string; retry_install: boolean });
