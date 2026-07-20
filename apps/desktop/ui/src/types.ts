export type CaptureMode = "region" | "window" | "display";
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
  snapshot_url: string;
  windows: WindowDescriptor[];
}

export interface AppSettings {
  output_directory: string;
  region_shortcut: string;
  window_shortcut: string;
  display_shortcut: string;
  auto_copy_to_clipboard: boolean;
  launch_at_login: boolean;
  last_screen_permission_request_id: string | null;
  pending_capture_after_restart: CaptureMode | null;
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

export interface HistoryEntry {
  id: string;
  preview_url: string;
  full_url: string;
  width: number;
  height: number;
  size_bytes: number;
  created_at: string;
  mode: CaptureMode;
}

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
