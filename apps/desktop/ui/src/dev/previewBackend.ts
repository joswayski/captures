/**
 * Dev-only design harness.
 *
 * Every Captures window is a `?view=` route that normally gets its data from
 * the Rust backend. `installPreviewBackend()` answers those IPC calls with
 * representative sample data so the real components can be reviewed, styled,
 * and screenshotted in a plain browser.
 *
 * Loaded only when `import.meta.env.DEV` and `?mock` is present, so it is
 * dropped from production bundles.
 */
import { mockIPC } from "@tauri-apps/api/mocks";

import type {
  ActiveSession,
  AppSettings,
  ArtifactSummary,
  AudioDevice,
  CaptureArtifact,
  ClipboardState,
  OnboardingState,
  RecordingArtifact,
  RecordingDraftManifest,
  RecordingSelectionSession,
  RecordingSessionSnapshot,
  UpdateStatus,
} from "../types";

type Query = URLSearchParams;

function query(): Query {
  return new URLSearchParams(window.location.search);
}

function flag(name: string): boolean {
  const value = query().get(name);
  return value !== null && value !== "0" && value !== "false";
}

/** A believable desktop screenshot, drawn as SVG so no binaries are checked in. */
function sampleCapture(width: number, height: number): string {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 1600 1000">
  <defs>
    <linearGradient id="sky" x1="0" y1="0" x2="0.4" y2="1">
      <stop offset="0" stop-color="#1b2440"/>
      <stop offset="0.55" stop-color="#2d3f63"/>
      <stop offset="1" stop-color="#5c6f92"/>
    </linearGradient>
    <linearGradient id="glass" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#ffffff" stop-opacity="0.16"/>
      <stop offset="1" stop-color="#ffffff" stop-opacity="0.04"/>
    </linearGradient>
  </defs>
  <rect width="1600" height="1000" fill="url(#sky)"/>
  <circle cx="1310" cy="180" r="86" fill="#f7e6b8" opacity="0.85"/>
  <path d="M0 700 L320 520 L560 660 L840 470 L1130 640 L1370 520 L1600 620 L1600 1000 L0 1000 Z" fill="#141d33" opacity="0.85"/>
  <path d="M0 800 L280 690 L620 790 L940 660 L1240 780 L1600 700 L1600 1000 L0 1000 Z" fill="#0d1424"/>
  <g transform="translate(180 160)">
    <rect width="760" height="500" rx="18" fill="#0f1220" opacity="0.92"/>
    <rect width="760" height="500" rx="18" fill="url(#glass)"/>
    <rect x="0" y="0" width="760" height="42" rx="18" fill="#171b2c"/>
    <circle cx="26" cy="21" r="6" fill="#ef5f57"/>
    <circle cx="48" cy="21" r="6" fill="#f0be4f"/>
    <circle cx="70" cy="21" r="6" fill="#5fc253"/>
    <g fill="#8ea0c8" font-family="monospace" font-size="17">
      <text x="34" y="96">function capture(region) {</text>
      <text x="60" y="130">const frame = await screen.read(region)</text>
      <text x="60" y="164">return encode(frame, { format: "png" })</text>
      <text x="34" y="198">}</text>
    </g>
    <g fill="#4d5c80" font-family="monospace" font-size="15">
      <text x="34" y="268">✓ 42 tests passed</text>
      <text x="34" y="300">✓ build finished in 1.8s</text>
    </g>
    <rect x="34" y="340" width="520" height="10" rx="5" fill="#243050"/>
    <rect x="34" y="340" width="330" height="10" rx="5" fill="#6f8bd0"/>
    <rect x="34" y="374" width="420" height="10" rx="5" fill="#243050"/>
    <rect x="34" y="408" width="300" height="10" rx="5" fill="#243050"/>
  </g>
  <g transform="translate(1000 300)">
    <rect width="420" height="420" rx="22" fill="#101528" opacity="0.9"/>
    <rect width="420" height="420" rx="22" fill="url(#glass)"/>
    <rect x="32" y="40" width="180" height="14" rx="7" fill="#3b4870"/>
    <rect x="32" y="78" width="260" height="10" rx="5" fill="#26314f"/>
    <rect x="32" y="126" width="356" height="150" rx="12" fill="#18203a"/>
    <polyline points="52,250 112,206 172,232 232,168 292,196 352,146" fill="none" stroke="#7fb8ff" stroke-width="5" stroke-linecap="round" stroke-linejoin="round"/>
    <rect x="32" y="300" width="160" height="12" rx="6" fill="#26314f"/>
    <rect x="32" y="330" width="220" height="12" rx="6" fill="#26314f"/>
    <rect x="32" y="360" width="120" height="12" rx="6" fill="#26314f"/>
  </g>
</svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

const CAPTURE_URL = sampleCapture(1600, 1000);
const CAPTURE_TALL_URL = sampleCapture(1200, 900);

const SETTINGS: AppSettings = {
  settings_schema_version: 2,
  appearance: "dark",
  theme: "mustard",
  custom_theme: { accent: "#32d3ff", signal: "#ff4fc3" },
  output_directory: "/Users/alex/Pictures/Captures",
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
  launch_at_login: true,
  last_screen_permission_request_id: null,
  pending_capture_after_restart: null,
  onboarding_completed: true,
  screenshot_countdown_seconds: 3,
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
    capture_system_audio: true,
    microphone_device_id: "mic-1",
    mono_audio: false,
    highlight_clicks: true,
    show_keystrokes: false,
    open_editor_after_recording: true,
  },
};

const DISPLAY = {
  id: "display-1",
  name: "Built-in Display",
  x: 0,
  y: 0,
  width: 1600,
  height: 1000,
  scale_factor: 2,
  is_primary: true,
};

const WINDOWS = [
  {
    id: "window-1",
    title: "capture.ts — captures",
    app_name: "Editor",
    z_order: 0,
    x: 180,
    y: 160,
    width: 760,
    height: 500,
    display_id: "display-1",
    corner_radius: 12,
  },
  {
    id: "window-2",
    title: "Metrics",
    app_name: "Dashboard",
    z_order: 1,
    x: 1000,
    y: 300,
    width: 420,
    height: 420,
    display_id: "display-1",
    corner_radius: 14,
  },
];

const ARTIFACT: CaptureArtifact = {
  id: "artifact-1",
  path: "/Users/alex/Pictures/Captures/Capture 2026-08-27 at 09.41.12.png",
  preview_url: CAPTURE_URL,
  full_url: CAPTURE_URL,
  width: 1600,
  height: 1000,
  size_bytes: 842_113,
  created_at: "2026-08-27T09:41:12Z",
  mode: "region",
  history_saved: true,
  clipboard_copy_status: "copied",
};

const SECOND_ARTIFACT: CaptureArtifact = {
  ...ARTIFACT,
  id: "artifact-2",
  path: null,
  preview_url: CAPTURE_TALL_URL,
  full_url: CAPTURE_TALL_URL,
  width: 1200,
  height: 900,
  size_bytes: 431_902,
  created_at: "2026-08-27T09:44:03Z",
  mode: "window",
  clipboard_copy_status: "skipped",
};

const RECORDING: RecordingArtifact = {
  id: "recording-1",
  kind: "video",
  path: "/Users/alex/Pictures/Captures/Recording 2026-08-27 at 09.32.00.mp4",
  saved_path: null,
  media_url: "",
  poster_url: CAPTURE_URL,
  mime_type: "video/mp4",
  duration_ms: 42_500,
  width: 1600,
  height: 1000,
  size_bytes: 18_442_000,
  dropped_frames: 0,
  has_system_audio: true,
  has_microphone_audio: true,
  created_at: "2026-08-27T09:32:00Z",
  target: { type: "display", display_id: "display-1" },
  missing: false,
};

const HISTORY: ArtifactSummary[] = [
  {
    id: "artifact-1",
    kind: "screenshot",
    preview_url: CAPTURE_URL,
    full_url: CAPTURE_URL,
    mode: "region",
    width: 1600,
    height: 1000,
    size_bytes: 842_113,
    created_at: "2026-08-27T09:41:12Z",
  },
  {
    id: "recording-1",
    kind: "video",
    poster_url: CAPTURE_URL,
    media_url: "",
    saved_path: null,
    mime_type: "video/mp4",
    duration_ms: 42_500,
    width: 1600,
    height: 1000,
    size_bytes: 18_442_000,
    dropped_frames: 0,
    has_system_audio: true,
    has_microphone_audio: true,
    target: { type: "display", display_id: "display-1" },
    missing: false,
    created_at: "2026-08-27T09:32:00Z",
  },
  {
    id: "artifact-2",
    kind: "screenshot",
    preview_url: CAPTURE_TALL_URL,
    full_url: CAPTURE_TALL_URL,
    mode: "window",
    width: 1200,
    height: 900,
    size_bytes: 431_902,
    created_at: "2026-08-26T18:05:44Z",
  },
  {
    id: "gif-1",
    kind: "gif",
    poster_url: CAPTURE_TALL_URL,
    media_url: "",
    saved_path: "/Users/alex/Pictures/Captures/Loop.gif",
    mime_type: "image/gif",
    duration_ms: 6_200,
    width: 800,
    height: 500,
    size_bytes: 3_120_400,
    dropped_frames: 0,
    has_system_audio: false,
    has_microphone_audio: false,
    target: { type: "region", display_id: "display-1", rect: { x: 0, y: 0, width: 800, height: 500 } },
    missing: false,
    created_at: "2026-08-26T11:22:09Z",
  },
];

const AUDIO_DEVICES: AudioDevice[] = [
  { id: "mic-1", name: "MacBook Pro Microphone", kind: "default", is_default: true },
  { id: "mic-2", name: "Shure MV7", kind: "microphone", is_default: false },
];

const ONBOARDING: OnboardingState = {
  platform: query().get("platform") ?? "macos",
  screen_recording_required: true,
  screen_recording_granted: flag("granted"),
  screen_recording_can_request: true,
  screen_recording_requested_this_launch: false,
  capture_system_audio: false,
  microphone_enabled: false,
  microphone_granted: false,
  microphone_can_request: true,
};

function updateStatus(): UpdateStatus {
  const state = query().get("update") ?? "available";
  const base = { current_version: "0.4.1", current_display_version: "0.4.1" };
  if (state === "downloading") {
    return { ...base, state: "downloading", version: "0.5.0", display_version: "0.5.0", downloaded: 7_340_032, total: 12_582_912 };
  }
  if (state === "up_to_date") return { ...base, state: "up_to_date" };
  return {
    ...base,
    state: "available",
    version: "0.5.0",
    display_version: "0.5.0",
    notes: [
      "Redesigned every Captures window around one design system.",
      "Added a light, dark, and system appearance setting.",
      "Capture History now filters by screenshots, video, and GIF.",
    ].map((note) => `- ${note}`).join("\n"),
    installable: true,
    manual_download_url: null,
  };
}

function recordingSnapshot(): RecordingSessionSnapshot {
  return {
    id: "session-1",
    state: (query().get("state") as RecordingSessionSnapshot["state"]) ?? "recording",
    options: {
      kind: "video",
      target: { type: "display", display_id: "display-1" },
      frames_per_second: 60,
      max_resolution: "original",
      countdown_seconds: 3,
      show_cursor: true,
      highlight_clicks: true,
      show_keystrokes: false,
      audio: {
        capture_system_audio: true,
        microphone_device_id: "mic-1",
        mono_output: false,
        system_volume_percent: 100,
        microphone_volume_percent: 100,
        microphone_muted: false,
      },
      gif: { max_width: 800, max_colors: 256, optimize: true },
    },
    elapsed_ms: 94_000,
    countdown_remaining_seconds: 3,
    warning: null,
    error: null,
  };
}

const SELECTION: RecordingSelectionSession = {
  id: "selection-1",
  kind: "video",
  initial_mode: (query().get("mode") as "screenshot" | "recording") ?? "screenshot",
  initial_target: (query().get("target") as ActiveSession["mode"]) ?? "region",
  recording_available: true,
  recording_capabilities: {
    system_audio: true,
    microphone: true,
    cursor_control: true,
    click_highlights: true,
    controls_excluded: true,
  },
  display: DISPLAY,
  displays: [DISPLAY, { ...DISPLAY, id: "display-2", name: "Studio Display", is_primary: false }],
  window_coordinate_scale: 1,
  window_corner_radius: 12,
  display_corner_radius: 0,
  snapshot_url: CAPTURE_URL,
  windows: WINDOWS,
};

const CAPTURE_SESSION: ActiveSession = {
  id: "session-1",
  mode: (query().get("mode") as ActiveSession["mode"]) ?? "region",
  display: DISPLAY,
  window_coordinate_scale: 1,
  window_corner_radius: 12,
  display_corner_radius: 0,
  snapshot_url: CAPTURE_URL,
  windows: WINDOWS,
};

const CLIPBOARD: ClipboardState = { revision: 4, artifact_id: "artifact-1" };
const DRAFTS: RecordingDraftManifest[] = [];

const RESPONSES: Record<string, unknown> = {
  get_settings: SETTINGS,
  update_settings: SETTINGS,
  get_update_status: updateStatus(),
  get_capture_history: HISTORY,
  get_recording_drafts: DRAFTS,
  get_artifacts: [ARTIFACT, SECOND_ARTIFACT],
  get_artifact: ARTIFACT,
  get_recording_artifact: RECORDING,
  get_clipboard_state: CLIPBOARD,
  get_recording_selection: SELECTION,
  get_pending_session: CAPTURE_SESSION,
  get_active_session: CAPTURE_SESSION,
  get_recording_snapshot: recordingSnapshot(),
  get_onboarding_state: ONBOARDING,
  list_recording_audio_devices: AUDIO_DEVICES,
  recording_controls_are_excluded: true,
  get_feedback_context: {
    app_version: "0.4.1",
    os: "macOS",
    os_version: "26.1",
    arch: "aarch64",
  },
  get_screenshot_countdown: { remaining_seconds: 3 },
  default_screenshot_edit_path: "/Users/alex/Pictures/Captures/Capture edited.png",
  load_screenshot_editor_draft: null,
  prepare_recording_timeline_preview: null,
  estimate_recording_export: { sizeBytes: 9_120_000, exact: false },
  estimate_screenshot_export: 512_000,
  prepared_drag_artifact_id: null,
};

export function installPreviewBackend(): void {
  mockIPC(async (command) => {
    if (command in RESPONSES) return RESPONSES[command];
    // Everything else is a side effect (show window, copy, save…) with no payload.
    return undefined;
  }, { shouldMockEvents: true });
}
