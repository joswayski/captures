export type CaptureMode = "region" | "window" | "display";

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
  launch_at_login: boolean;
}

export interface CaptureArtifact {
  id: string;
  path: string;
  preview_url: string;
  width: number;
  height: number;
  created_at: string;
  mode: CaptureMode;
  clipboard_copied: boolean;
}
