import { isTauri } from "@tauri-apps/api/core";

import { installTauriBridge } from "../lib/tauri";
import type {
  ActiveSession,
  AppSettings,
  ArtifactSummary,
  AudioDevice,
  CaptureArtifact,
  OnboardingState,
  RecordingArtifact,
  RecordingDraftManifest,
  RecordingSelectionSession,
  RecordingSessionSnapshot,
  RecordingTimelinePreview,
  UpdateStatus,
} from "../types";

function query(name: string): string | null {
  return new URLSearchParams(window.location.search).get(name);
}

function makeDesktopSnapshot(width = 1_440, height = 900): string {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (!ctx) return "";

  const sky = ctx.createLinearGradient(0, 0, 0, height);
  sky.addColorStop(0, "#1c1c22");
  sky.addColorStop(0.45, "#141418");
  sky.addColorStop(1, "#0b0b0d");
  ctx.fillStyle = sky;
  ctx.fillRect(0, 0, width, height);

  ctx.fillStyle = "#18181b";
  roundRect(ctx, 72, 64, 620, 420, 16);
  ctx.fill();
  ctx.fillStyle = "#f4f4f5";
  ctx.font = "600 22px ui-sans-serif, system-ui, sans-serif";
  ctx.fillText("Inbox", 96, 108);
  ctx.fillStyle = "#a1a1aa";
  ctx.font = "500 14px ui-sans-serif, system-ui, sans-serif";
  ctx.fillText("Design review · Captures redesign", 96, 138);
  for (let index = 0; index < 5; index += 1) {
    ctx.fillStyle = index === 1 ? "rgba(255,202,40,0.16)" : "#121214";
    roundRect(ctx, 96, 168 + index * 52, 572, 44, 10);
    ctx.fill();
  }

  ctx.fillStyle = "#121214";
  roundRect(ctx, 720, 64, 648, 520, 16);
  ctx.fill();
  ctx.fillStyle = "#27272b";
  roundRect(ctx, 744, 88, 600, 360, 12);
  ctx.fill();
  ctx.fillStyle = "#f4f4f5";
  ctx.font = "560 28px ui-sans-serif, system-ui, sans-serif";
  ctx.fillText("Captures", 768, 140);
  ctx.fillStyle = "#a1a1aa";
  ctx.font = "500 16px ui-sans-serif, system-ui, sans-serif";
  ctx.fillText("A quieter capture, still crystal clear.", 768, 172);

  ctx.fillStyle = "rgba(18,18,21,0.92)";
  ctx.fillRect(0, height - 48, width, 48);
  ctx.fillStyle = "#f4f4f5";
  ctx.font = "500 13px ui-sans-serif, system-ui, sans-serif";
  ctx.fillText("12:41", width - 86, height - 18);

  return canvas.toDataURL("image/png");
}

function makeTimelineSprite(): string {
  const frameCount = 12;
  const frameWidth = 160;
  const frameHeight = 90;
  const canvas = document.createElement("canvas");
  canvas.width = frameWidth * frameCount;
  canvas.height = frameHeight;
  const ctx = canvas.getContext("2d");
  if (!ctx) return "";
  for (let index = 0; index < frameCount; index += 1) {
    const x = index * frameWidth;
    ctx.fillStyle = index % 2 === 0 ? "#18181b" : "#1f1f23";
    ctx.fillRect(x, 0, frameWidth, frameHeight);
    ctx.fillStyle = "#ffca28";
    ctx.fillRect(x + 16 + index * 8, 28, 48, 34);
  }
  return canvas.toDataURL("image/png");
}

function roundRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  width: number,
  height: number,
  radius: number,
) {
  ctx.beginPath();
  ctx.roundRect(x, y, width, height, radius);
}

let snapshotUrl = "";
let timelineUrl = "";

function images() {
  if (!snapshotUrl) snapshotUrl = makeDesktopSnapshot();
  if (!timelineUrl) timelineUrl = makeTimelineSprite();
  return { snapshotUrl, timelineUrl };
}

const previewSettings: AppSettings = {
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

function display() {
  return {
    id: "display-1",
    name: "Built-in Display",
    x: 0,
    y: 0,
    width: 1_440,
    height: 900,
    scale_factor: 1,
    is_primary: true,
  };
}

function windows() {
  return [
    {
      id: "front-window",
      title: "Captures Preferences",
      app_name: "Captures",
      z_order: 30,
      x: 120,
      y: 90,
      width: 880,
      height: 640,
      display_id: "display-1",
      corner_radius: 12,
    },
    {
      id: "browser-window",
      title: "captur.es",
      app_name: "Browser",
      z_order: 20,
      x: 280,
      y: 140,
      width: 980,
      height: 680,
      display_id: "display-1",
      corner_radius: 16,
    },
  ];
}

function captureArtifact(): CaptureArtifact {
  const { snapshotUrl } = images();
  return {
    id: "capture-1",
    path: "/Users/josevalerio/Captures/Captures_1440x900.png",
    preview_url: snapshotUrl,
    full_url: snapshotUrl,
    width: 1_440,
    height: 900,
    size_bytes: 250_000,
    created_at: "2026-08-27T12:41:00Z",
    mode: "region",
    history_saved: true,
    clipboard_copy_status: "copied",
  };
}

function recordingArtifact(): RecordingArtifact {
  const { snapshotUrl } = images();
  return {
    id: "recording-1",
    kind: "video",
    path: "/Users/josevalerio/Captures/Captures_1440x900.mp4",
    saved_path: "/Users/josevalerio/Captures/Captures_1440x900.mp4",
    media_url: snapshotUrl,
    poster_url: snapshotUrl,
    mime_type: "video/mp4",
    duration_ms: 8_750,
    width: 1_140,
    height: 692,
    size_bytes: 4_200_000,
    dropped_frames: 0,
    has_system_audio: true,
    has_microphone_audio: true,
    created_at: "2026-08-27T12:38:00Z",
    target: { type: "display", display_id: "display-1" },
    missing: false,
  };
}

function recordingOptions(): RecordingSessionSnapshot["options"] {
  return {
    kind: "video",
    target: {
      type: "region",
      display_id: "display-1",
      rect: { x: 180, y: 120, width: 960, height: 540 },
    },
    frames_per_second: 60,
    max_resolution: "original",
    countdown_seconds: 3,
    show_cursor: true,
    highlight_clicks: false,
    show_keystrokes: false,
    audio: {
      capture_system_audio: false,
      microphone_device_id: null,
      mono_output: false,
      system_volume_percent: 100,
      microphone_volume_percent: 100,
      microphone_muted: false,
    },
    gif: { max_width: 800, max_colors: 256, optimize: true },
  };
}

function recordingHud(): RecordingSessionSnapshot {
  const mode = query("preview_mode");
  if (mode === "countdown") {
    return {
      id: "recording-1",
      state: "countdown",
      options: recordingOptions(),
      elapsed_ms: 0,
      countdown_remaining_seconds: 2,
      warning: null,
      error: null,
    };
  }
  if (mode === "paused") {
    return {
      id: "recording-1",
      state: "paused",
      options: recordingOptions(),
      elapsed_ms: 12_400,
      countdown_remaining_seconds: null,
      warning: null,
      error: null,
    };
  }
  return {
    id: "recording-1",
    state: "recording",
    options: recordingOptions(),
    elapsed_ms: 12_400,
    countdown_remaining_seconds: null,
    warning: null,
    error: null,
  };
}

function selectionSession(): RecordingSelectionSession {
  const { snapshotUrl } = images();
  const mode = query("preview_mode") === "recording" ? "recording" : "screenshot";
  const target = (query("target") ?? "region") as RecordingSelectionSession["initial_target"];
  return {
    id: "selection-1",
    kind: "video",
    initial_mode: mode,
    initial_target: target,
    recording_available: true,
    recording_capabilities: {
      system_audio: true,
      microphone: true,
      cursor_control: true,
      click_highlights: true,
      controls_excluded: true,
    },
    display: display(),
    displays: [display(), { ...display(), id: "display-2", name: "Studio Display", x: 1440, is_primary: false }],
    window_coordinate_scale: 1,
    window_corner_radius: 16,
    snapshot_url: snapshotUrl,
    windows: windows(),
  };
}

function overlaySession(): ActiveSession {
  const { snapshotUrl } = images();
  const mode = (query("mode") ?? "region") as ActiveSession["mode"];
  return {
    id: "capture-1",
    mode,
    display: display(),
    window_coordinate_scale: 1,
    window_corner_radius: 16,
    snapshot_url: snapshotUrl,
    windows: windows(),
  };
}

function historyEntries(): ArtifactSummary[] {
  const { snapshotUrl } = images();
  return [
    {
      id: "capture-1",
      kind: "screenshot",
      preview_url: snapshotUrl,
      full_url: snapshotUrl,
      width: 1_440,
      height: 900,
      size_bytes: 250_000,
      created_at: "2026-08-27T12:41:00Z",
      mode: "region",
    },
    {
      id: "recording-1",
      kind: "video",
      poster_url: snapshotUrl,
      media_url: snapshotUrl,
      saved_path: null,
      mime_type: "video/mp4",
      duration_ms: 62_500,
      width: 1_920,
      height: 1_080,
      size_bytes: 5_000_000,
      dropped_frames: 0,
      has_system_audio: true,
      has_microphone_audio: true,
      created_at: "2026-08-27T12:38:00Z",
      target: { type: "display", display_id: "1" },
      missing: false,
    },
  ];
}

function interruptedDraft(): RecordingDraftManifest {
  return {
    session_id: "draft-1",
    created_at_ms: Date.parse("2026-08-27T12:30:00Z"),
    updated_at_ms: Date.parse("2026-08-27T12:30:08Z"),
    state: "failed",
    options: recordingOptions(),
    segments: [{
      index: 0,
      duration_ms: 4_200,
      size_bytes: 250_000,
      dropped_frames: 0,
      complete: true,
    }],
    final_path: null,
    last_error: "The recording did not contain a complete video frame.",
  };
}

function onboardingState(): OnboardingState {
  if (query("preview_mode") === "macos") {
    return {
      platform: "macos",
      screen_recording_required: true,
      screen_recording_granted: false,
      screen_recording_can_request: true,
      screen_recording_requested_this_launch: false,
      capture_system_audio: false,
      microphone_enabled: false,
      microphone_granted: false,
      microphone_can_request: true,
    };
  }
  return {
    platform: "linux",
    screen_recording_required: false,
    screen_recording_granted: true,
    screen_recording_can_request: false,
    screen_recording_requested_this_launch: false,
    capture_system_audio: true,
    microphone_enabled: true,
    microphone_granted: true,
    microphone_can_request: true,
  };
}

function updateStatus(): UpdateStatus {
  if (query("preview_mode") === "downloading") {
    return {
      state: "downloading",
      current_version: "2026.8.2701",
      current_display_version: "2026.08.27.1",
      version: "2026.8.2702",
      display_version: "2026.08.27.2",
      downloaded: 42,
      total: 100,
    };
  }
  return {
    state: "available",
    current_version: "2026.8.2701",
    current_display_version: "2026.08.27.1",
    version: "2026.8.2702",
    display_version: "2026.08.27.2",
    notes: "## What's Changed\n* Quieter chrome and clearer actions across every window.\n",
    installable: true,
    manual_download_url: null,
  };
}

const audioDevices: AudioDevice[] = [
  { id: "default", name: "MacBook Pro Microphone", kind: "default", is_default: true },
  { id: "studio", name: "Studio Mic", kind: "microphone", is_default: false },
];

async function previewInvoke(command: string, args?: Record<string, unknown>): Promise<unknown> {
  switch (command) {
    case "get_settings":
    case "update_settings":
      return args && "settings" in args ? args.settings : previewSettings;
    case "get_update_status":
      return updateStatus();
    case "list_recording_audio_devices":
      return audioDevices;
    case "get_feedback_context":
      return {
        app_version: "2026.08.27.1",
        os: "linux",
        os_version: "6.12",
        arch: "x86_64",
      };
    case "get_onboarding_state":
      return onboardingState();
    case "get_active_session":
    case "get_pending_session":
      return overlaySession();
    case "get_recording_selection":
      return selectionSession();
    case "recording_controls_are_excluded":
      return true;
    case "show_capture_overlay":
    case "reveal_capture_overlay":
    case "show_recording_selector":
    case "reveal_recording_selector":
    case "set_shortcut_capture_suppressed":
      return undefined;
    case "get_recording_snapshot":
      return recordingHud();
    case "get_screenshot_countdown":
      return { remaining_seconds: 3 };
    case "get_artifact":
    case "get_artifacts":
      return command === "get_artifacts" ? [captureArtifact()] : captureArtifact();
    case "get_clipboard_state":
      return { revision: 1, artifact_id: "capture-1" };
    case "get_thumbnail_pointer_position":
      return query("preview_mode") === "hover"
        ? { x: 48, y: 120, inside: true }
        : { x: 0, y: 0, inside: false };
    case "get_capture_history":
      return historyEntries();
    case "get_recording_drafts":
      return [interruptedDraft()];
    case "get_recording_artifact":
      return recordingArtifact();
    case "prepare_recording_timeline_preview": {
      const { timelineUrl } = images();
      const preview: RecordingTimelinePreview = {
        url: timelineUrl,
        frame_count: 12,
        frame_width: 160,
        frame_height: 90,
        sprite_width: 1_920,
        sprite_height: 90,
      };
      return preview;
    }
    case "default_screenshot_edit_path":
      return "/Users/josevalerio/Captures/Captures_1440x900.png";
    case "load_screenshot_editor_draft":
      return null;
    case "estimate_screenshot_export":
    case "estimate_recording_export":
      return command === "estimate_recording_export"
        ? { sizeBytes: 2_400_000, exact: false }
        : 180_000;
    default:
      return undefined;
  }
}

export function installPreviewIfNeeded() {
  const forced = query("preview") === "1";
  if (!forced && isTauri()) return;
  document.documentElement.classList.add("capture-preview");
  const view = query("view");
  if (view) document.documentElement.dataset.previewView = view;
  const previewMode = query("preview_mode");
  if (previewMode) document.documentElement.dataset.previewMode = previewMode;
  images();
  installTauriBridge({
    invoke: previewInvoke as never,
    listen: (async () => () => undefined) as never,
    emit: (async () => undefined) as never,
    isTauri: () => false,
  });
}
