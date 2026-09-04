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

import { readStoredAppearance } from "../../../../../shared/appearance";

import type {
  ActiveSession,
  AppSettings,
  ArtifactSummary,
  AudioDevice,
  CaptureArtifact,
  ClipboardState,
  DisplayDescriptor,
  OnboardingState,
  RecordingArtifact,
  RecordingDraftManifest,
  RecordingSelectionSession,
  RecordingSessionSnapshot,
  UpdateStatus,
  WindowDescriptor,
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

/** Horizontal sprite of evenly spaced frames, like the backend timeline preview. */
function sampleFilmstrip(frames: number): string {
  const frameWidth = 120;
  const frameHeight = 76;
  const cells = Array.from({ length: frames }, (_, index) => {
    const x = index * frameWidth;
    const shift = (index / Math.max(1, frames - 1)) * 60;
    return `<g transform="translate(${x} 0)">
      <rect width="${frameWidth}" height="${frameHeight}" fill="#1b2440"/>
      <path d="M0 ${frameHeight} L30 ${52 - shift / 6} L62 ${60 - shift / 8} L92 ${46 - shift / 5} L${frameWidth} ${58 - shift / 7} L${frameWidth} ${frameHeight} Z" fill="#101827"/>
      <rect x="${8 + shift / 4}" y="14" width="26" height="16" rx="3" fill="#3a4b74"/>
      <rect x="8" y="${frameHeight - 18}" width="${44 + shift / 3}" height="4" rx="2" fill="#4a5f92"/>
    </g>`;
  }).join("");
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${frames * frameWidth}" height="${frameHeight}" viewBox="0 0 ${frames * frameWidth} ${frameHeight}">${cells}</svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

const TIMELINE_FRAMES = 14;
const CAPTURE_URL = sampleCapture(1600, 1000);
const FILMSTRIP_URL = sampleFilmstrip(TIMELINE_FRAMES);
const CAPTURE_TALL_URL = sampleCapture(1200, 900);

function previewPlatform(): "macos" | "windows" | "linux" {
  const value = query().get("platform");
  if (value === "windows" || value === "linux" || value === "macos") return value;
  return "macos";
}

function previewFreezeScreen(): boolean {
  return !flag("live") && query().get("frozen") !== "0";
}

function previewScreenshotFormat(): AppSettings["screenshot_format"] {
  const value = query().get("screenshot_format");
  return value === "jpeg" || value === "webp" ? value : "png";
}

function previewVideoFormat(): AppSettings["recording"]["video_format"] {
  const value = query().get("video_format");
  return value === "gif" || value === "webm" ? value : "mp4";
}

function previewShortcutSettings(
  platform: "macos" | "windows" | "linux" = previewPlatform(),
): Pick<
  AppSettings,
  | "new_capture_shortcut"
  | "region_shortcut"
  | "window_shortcut"
  | "display_shortcut"
> & { recording: AppSettings["recording"] } {
  const recording = {
    video_fps: 60 as const,
    video_max_resolution: "original" as const,
    video_format: previewVideoFormat(),
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
    gif_shortcut: "CommandOrControl+Shift+6",
    video_shortcut: "CommandOrControl+Shift+5",
  };
  if (platform === "windows") {
    return {
      new_capture_shortcut: "CommandOrControl+Shift+Space",
      region_shortcut: "Super+Shift+S",
      window_shortcut: "Alt+PrintScreen",
      display_shortcut: "PrintScreen",
      recording: { ...recording, video_shortcut: "Super+Alt+R" },
    };
  }
  if (platform === "linux") {
    return {
      new_capture_shortcut: "PrintScreen",
      region_shortcut: "Super+Shift+S",
      window_shortcut: "Alt+PrintScreen",
      display_shortcut: "Shift+PrintScreen",
      recording: { ...recording, video_shortcut: "Control+Shift+Alt+R" },
    };
  }
  return {
    new_capture_shortcut: "CommandOrControl+Shift+Space",
    region_shortcut: "CommandOrControl+Shift+4",
    window_shortcut: "CommandOrControl+Shift+W",
    display_shortcut: "CommandOrControl+Shift+3",
    recording,
  };
}

const SETTINGS: AppSettings = {
  settings_schema_version: 4,
  appearance: readStoredAppearance(),
  theme: "mustard",
  custom_theme: { accent: "#32d3ff", signal: "#ff4fc3" },
  output_directory: "/Users/alex/Pictures/Captures",
  ...previewShortcutSettings(),
  auto_copy_to_clipboard: true,
  auto_start_on_selection: flag("auto"),
  show_mini_previews: true,
  include_mini_previews_in_captures: false,
  include_recording_controls_in_captures: false,
  launch_at_login: true,
  last_screen_permission_request_id: null,
  pending_capture_after_restart: null,
  onboarding_completed: true,
  screenshot_countdown_seconds: 3,
  freeze_screen: previewFreezeScreen(),
  show_cursor_in_screenshots: true,
  screenshot_format: previewScreenshotFormat(),
};

const DISPLAY: DisplayDescriptor = {
  id: "display-1",
  name: "Built-in Display",
  x: 0,
  y: 0,
  width: 1600,
  height: 1000,
  scale_factor: 2,
  is_primary: true,
};

const DISPLAYS: DisplayDescriptor[] = [
  DISPLAY,
  {
    id: "display-2",
    name: "Studio Display",
    x: DISPLAY.width,
    y: 0,
    width: 1920,
    height: 1080,
    scale_factor: 2,
    is_primary: false,
  },
];

const WINDOWS: WindowDescriptor[] = [
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
  {
    id: "window-3",
    title: "Safari",
    app_name: "Safari",
    z_order: 0,
    x: DISPLAY.width + 80,
    y: 80,
    width: 1100,
    height: 800,
    display_id: "display-2",
    corner_radius: 12,
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

function mockArtifacts(): CaptureArtifact[] {
  const parsed = Number.parseInt(query().get("count") ?? "", 10);
  const count = Number.isFinite(parsed) ? Math.min(24, Math.max(1, parsed)) : 2;
  const samples = [ARTIFACT, SECOND_ARTIFACT];
  return Array.from({ length: count }, (_, index) => ({
    ...samples[index % samples.length],
    id: `artifact-${index + 1}`,
    created_at: new Date(Date.parse("2026-08-27T09:41:12Z") + index * 60_000).toISOString(),
  }));
}

/**
 * Optional local clip for reviewing the recording editor. Generate one with
 * `ffmpeg -f lavfi -i color=c=black:s=800x500:d=6 apps/desktop/ui/public/dev-sample.mp4`.
 * The editor still lays out correctly when it is missing.
 */
const SAMPLE_VIDEO_URL = "/dev-sample.mp4";

const RECORDING: RecordingArtifact = {
  id: "recording-1",
  kind: "video",
  path: "/Users/alex/Pictures/Captures/Recording 2026-08-27 at 09.32.00.mp4",
  saved_path: null,
  media_url: SAMPLE_VIDEO_URL,
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
    media_url: SAMPLE_VIDEO_URL,
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
  platform: previewPlatform(),
  screen_recording_required: true,
  screen_recording_granted: flag("granted"),
  screen_recording_can_request: true,
  screen_recording_requested_this_launch: false,
  capture_system_audio: false,
  microphone_enabled: false,
  microphone_granted: false,
  microphone_can_request: true,
};

function previewNotes(summary: string): string {
  return [
    "> [!WARNING]",
    "> This Preview is functional, but experimental.",
    "",
    "## What's Changed",
    `* ${summary} by @joswayski in https://github.com/joswayski/captures/pull/1`,
    "* @devin-ai-integration[bot] made their first contribution in https://github.com/joswayski/captures/pull/1",
    "",
    "## New Contributors",
    "* @someone made their first contribution in https://github.com/joswayski/captures/pull/1",
    "",
    "**Full Changelog**: https://github.com/joswayski/captures/compare/old...new",
  ].join("\n");
}

function updateStatus(): UpdateStatus {
  const state = query().get("update") ?? "available";
  const base = {
    current_version: "2026.8.2702",
    current_display_version: "2026.08.27.2",
  };
  if (state === "downloading") {
    return {
      ...base,
      state: "downloading",
      version: "2026.8.2705",
      display_version: "2026.08.27.5",
      downloaded: 7_340_032,
      total: 12_582_912,
    };
  }
  if (state === "up_to_date") return { ...base, state: "up_to_date" };
  if (state === "error") {
    return {
      ...base,
      state: "error",
      message:
        "Could not install the update: Download request failed with status: 403 Forbidden",
      retry_install: true,
    };
  }
  const latestNotes = previewNotes("Fix post-update launch notice position on macOS");
  return {
    ...base,
    state: "available",
    version: "2026.8.2705",
    display_version: "2026.08.27.5",
    notes: latestNotes,
    changelog: [
      {
        version: "2026.8.2705",
        display_version: "2026.08.27.5",
        notes: latestNotes,
      },
      {
        version: "2026.8.2704",
        display_version: "2026.08.27.4",
        notes: previewNotes("Fix capture menu display switching and the Record CTA"),
      },
      {
        version: "2026.8.2703",
        display_version: "2026.08.27.3",
        notes: previewNotes("Redesign the desktop UI around one design system"),
      },
    ],
    installable: true,
    manual_download_url: null,
    download_size: 12_582_912,
    will_close_open_captures: flag("captures"),
  };
}

function recordingSnapshot(): RecordingSessionSnapshot {
  const target = query().get("target") === "region"
    ? {
        type: "region" as const,
        display_id: "display-1",
        rect: { x: 260, y: 180, width: 1_000, height: 640 },
      }
    : { type: "display" as const, display_id: "display-1" };
  return {
    id: "session-1",
    state: (query().get("state") as RecordingSessionSnapshot["state"]) ?? "recording",
    options: {
      kind: "video",
      target,
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

function snapshotUrlFor(display: DisplayDescriptor): string {
  return `${sampleCapture(display.width, display.height)}#${display.id}`;
}

function payloadString(payload: unknown, ...keys: string[]): string | undefined {
  if (!payload || typeof payload !== "object") return undefined;
  const record = payload as Record<string, unknown>;
  for (const key of keys) {
    const value = record[key];
    if (typeof value === "string" && value.length > 0) return value;
  }
  return undefined;
}

function createSelection(): RecordingSelectionSession {
  return {
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
    displays: DISPLAYS.map((item) => ({ ...item })),
    window_coordinate_scale: 1,
    window_corner_radius: 12,
    display_corner_radius: 0,
    frozen: previewFreezeScreen(),
    snapshot_url: previewFreezeScreen() ? snapshotUrlFor(DISPLAY) : "",
    windows: WINDOWS.filter((window) => window.display_id === DISPLAY.id),
  };
}

let selection: RecordingSelectionSession = createSelection();

function selectCaptureDisplay(payload: unknown): RecordingSelectionSession {
  const selectionId = payloadString(payload, "selectionId", "selection_id");
  const displayId = payloadString(payload, "displayId", "display_id");
  if (!displayId) {
    throw new Error("display is unavailable");
  }
  if (selectionId && selectionId !== selection.id) {
    throw new Error("session is unavailable");
  }
  const display = selection.displays.find((candidate) => candidate.id === displayId);
  if (!display) {
    throw new Error("display is unavailable");
  }
  selection = {
    ...selection,
    display: { ...display },
    frozen: previewFreezeScreen(),
    snapshot_url: previewFreezeScreen() ? snapshotUrlFor(display) : "",
    windows: WINDOWS.filter((window) => window.display_id === display.id),
  };
  return selection;
}

const CAPTURE_SESSION: ActiveSession = {
  id: "session-1",
  mode: (query().get("mode") as ActiveSession["mode"]) ?? "region",
  display: DISPLAY,
  window_coordinate_scale: 1,
  window_corner_radius: 12,
  display_corner_radius: 0,
  frozen: previewFreezeScreen(),
  snapshot_url: previewFreezeScreen() ? CAPTURE_URL : "",
  windows: WINDOWS.filter((window) => window.display_id === DISPLAY.id),
};

const CLIPBOARD: ClipboardState = { revision: 4, artifact_id: "artifact-1" };

const DRAFTS: RecordingDraftManifest[] = flag("drafts")
  ? [
    {
      session_id: "draft-1",
      created_at_ms: Date.parse("2026-08-27T08:12:00Z"),
      updated_at_ms: Date.parse("2026-08-27T08:13:20Z"),
      state: "failed",
      options: recordingSnapshot().options,
      segments: [
        { index: 0, duration_ms: 42_000, size_bytes: 8_200_000, dropped_frames: 0, complete: true },
        { index: 1, duration_ms: 6_000, size_bytes: 1_100_000, dropped_frames: 3, complete: false },
      ],
      final_path: null,
      last_error: "Recording failed: the display went to sleep.",
    },
  ]
  : [];

const RESPONSES: Record<string, unknown> = {
  get_settings: SETTINGS,
  update_settings: SETTINGS,
  get_update_status: updateStatus(),
  get_capture_history: HISTORY,
  get_recording_drafts: DRAFTS,
  get_artifacts: mockArtifacts(),
  get_artifact: ARTIFACT,
  get_recording_artifact: RECORDING,
  get_clipboard_state: CLIPBOARD,
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
  prepare_recording_timeline_preview: {
    url: FILMSTRIP_URL,
    frame_count: TIMELINE_FRAMES,
    frame_width: 120,
    frame_height: 76,
    sprite_width: TIMELINE_FRAMES * 120,
    sprite_height: 76,
  },
  prepared_drag_artifact_id: null,
  preview_file_drop_landing: "app_window",
};

function mockScreenshotExportBytes(payload: unknown): number {
  const request = payload as {
    imagePng?: number[];
    pngMaxColors?: number;
    jpegQuality?: number;
    maxSizeBytes?: number | null;
    qualityMode?: string;
  } | undefined;
  const preserveBytes = Array.isArray(request?.imagePng) && request.imagePng.length > 0
    ? request.imagePng.length
    : 1_200_000;
  const colors = Number(request?.pngMaxColors);
  const qualityEncoded = Number.isFinite(colors) && colors > 0
    ? Math.round(140_000 + colors * 630)
    : Math.round(120_000 + Math.max(20, Number(request?.jpegQuality ?? 98)) * 2_000);
  const maximum = Number(request?.maxSizeBytes);
  if (request?.qualityMode === "maximum" && Number.isFinite(maximum) && maximum > 0) {
    if (preserveBytes <= maximum) return preserveBytes;
    return Math.max(10_000, Math.floor(maximum * 0.94));
  }
  return qualityEncoded;
}

async function samplePreviewPng(quality = 0.92): Promise<number[]> {
  const image = new Image();
  image.src = CAPTURE_URL;
  try {
    await image.decode();
  } catch {
    return [];
  }
  const canvas = document.createElement("canvas");
  canvas.width = image.naturalWidth || 800;
  canvas.height = image.naturalHeight || 500;
  const context = canvas.getContext("2d");
  if (!context) return [];
  // Match the real encoder: keep hues and show spatial loss (blockiness /
  // pixelation) instead of a global desaturation wash.
  if (quality < 0.85) {
    const block = Math.max(2, Math.round((1 - quality) * 10));
    const smallWidth = Math.max(1, Math.round(canvas.width / block));
    const smallHeight = Math.max(1, Math.round(canvas.height / block));
    const scratch = document.createElement("canvas");
    scratch.width = smallWidth;
    scratch.height = smallHeight;
    const scratchContext = scratch.getContext("2d");
    if (!scratchContext) return [];
    scratchContext.imageSmoothingEnabled = false;
    scratchContext.drawImage(image, 0, 0, smallWidth, smallHeight);
    context.imageSmoothingEnabled = false;
    context.drawImage(scratch, 0, 0, canvas.width, canvas.height);
  } else {
    context.drawImage(image, 0, 0, canvas.width, canvas.height);
  }
  const blob = await new Promise<Blob | null>((resolve) => {
    canvas.toBlob(resolve, "image/jpeg", quality);
  });
  if (!blob) return [];
  return Array.from(new Uint8Array(await blob.arrayBuffer()));
}

/**
 * Overlay windows are transparent and normally float over the desktop. `?stage`
 * paints the sample desktop behind them so they can be reviewed in context.
 */
function applyPreviewStage(): void {
  if (!flag("stage")) return;
  const style = document.documentElement.style;
  style.setProperty("background-image", `url("${CAPTURE_URL}")`);
  style.setProperty("background-size", "cover");
  style.setProperty("background-position", "center");
}

let thumbnailPointer = { x: 0, y: 0, inside: false };
let thumbnailPointerTracking = false;

function trackThumbnailPointerForHarness(): void {
  if (thumbnailPointerTracking) return;
  thumbnailPointerTracking = true;
  const update = (event: PointerEvent) => {
    thumbnailPointer = { x: event.clientX, y: event.clientY, inside: true };
  };
  window.addEventListener("pointermove", update, true);
  window.addEventListener("pointerdown", update, true);
  document.documentElement.addEventListener("pointerleave", (event) => {
    if (event.relatedTarget) return;
    thumbnailPointer = { ...thumbnailPointer, inside: false };
  });
}

export function installPreviewBackend(): void {
  applyPreviewStage();
  trackThumbnailPointerForHarness();
  selection = createSelection();
  mockIPC(async (command, payload) => {
    if (command === "get_recording_selection") return selection;
    if (command === "select_capture_display") return selectCaptureDisplay(payload);
    if (command === "get_thumbnail_pointer_position") return thumbnailPointer;
    if (command === "estimate_screenshot_export") {
      return mockScreenshotExportBytes(payload);
    }
    if (command === "preview_screenshot_export") {
      await new Promise((resolve) => window.setTimeout(resolve, 650));
      const request = payload as {
        imagePng?: number[];
        jpegQuality?: number;
        format?: string;
      } | undefined;
      // Use the flattened editor canvas so text blobs keep the same glyphs as
      // the live before view. Falling back to the sample still would overlay a
      // different image and look like the typeface changed at the split.
      const imagePng = request?.imagePng;
      const sizeBytes = mockScreenshotExportBytes(payload);
      if (imagePng && imagePng.length > 0) {
        return {
          bytes: imagePng,
          sizeBytes,
          format: request?.format ?? "png",
        };
      }
      const quality = Number(request?.jpegQuality ?? 70);
      const bytes = await samplePreviewPng(Math.max(0.2, quality / 100));
      return {
        bytes,
        sizeBytes: mockScreenshotExportBytes(payload),
        format: request?.format ?? "png",
      };
    }
    if (command === "preview_recording_export") {
      const [beforePng, afterPng] = await Promise.all([
        samplePreviewPng(0.95),
        samplePreviewPng(0.45),
      ]);
      return { beforePng, afterPng };
    }
    if (command === "estimate_recording_export") {
      const request = payload as {
        edit?: { output_width?: number | null; output_height?: number | null };
        export?: { quality?: string };
      } | undefined;
      const original = RECORDING.size_bytes;
      if (request?.export?.quality && request.export.quality !== "preserve") {
        const factors: Record<string, number> = {
          tiny: 0.18,
          small: 0.28,
          standard: 0.38,
          high: 0.49,
          highest: 0.65,
        };
        const factor = factors[request.export.quality] ?? 0.49;
        return { sizeBytes: Math.round(original * factor), exact: false };
      }
      const outHeight = request?.edit?.output_height;
      const outWidth = request?.edit?.output_width;
      if (typeof outHeight === "number" && outHeight < RECORDING.height) {
        const width = typeof outWidth === "number" ? outWidth : RECORDING.width;
        const scale = (width * outHeight) / (RECORDING.width * RECORDING.height);
        return { sizeBytes: Math.round(original * Math.max(0.18, scale)), exact: false };
      }
      return { sizeBytes: original, exact: true };
    }
    if (command in RESPONSES) return RESPONSES[command];
    // Everything else is a side effect (show window, copy, save…) with no payload.
    return undefined;
  }, { shouldMockEvents: true });
}
