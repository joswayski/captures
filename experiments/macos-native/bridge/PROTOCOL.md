# Captures macOS native bridge protocol

The bridge is a standalone Rust `staticlib` with no Tauri dependency. Include
`include/captures_macos_bridge.h`, pass one UTF-8 JSON object to
`captures_native_request`, copy the returned UTF-8 JSON, and release it exactly
once with `captures_native_free`. Calls are synchronous. Recording lifecycle,
capture discovery, and screenshots execute on one bridge-owned background
thread. Stateless `image_encode`, `media_probe`, `media_export`, and
`recover_list` work, plus `microphone_permission`, executes on the calling thread
so long media jobs or permission prompts cannot block recording safety ticks.
Recovery assembly/discard executes on the caller
under a bridge-wide reservation after the recording worker confirms it is idle.
Swift must not call any potentially long operation on its main thread.

Every request has an `op`. Every response is exactly one of:

```json
{"ok":true,"value":{}}
{"ok":false,"error":"actionable description"}
```

Panics are caught at the operation and C ABI boundaries. The bridge does not log
request JSON, paths, pixels, audio, or media-tool output.

## Shared JSON types

`DisplayDescriptor`, `WindowDescriptor`, `AudioDevice`, and `RecordingOptions`
are serialized exactly as their public definitions in `captures-capture` and
`captures-recording`. The current `RecordingOptions` shape is:

```json
{
  "kind": "video | gif",
  "target": {"type":"display","display_id":"..."}
          | {"type":"region","display_id":"...","rect":{"x":0,"y":0,"width":800,"height":600}}
          | {"type":"window","window_id":"..."},
  "frames_per_second": 30,
  "max_resolution": "original | p1080 | p720",
  "countdown_seconds": 0,
  "show_cursor": true,
  "highlight_clicks": false,
  "show_keystrokes": false,
  "audio": {
    "capture_system_audio": false,
    "microphone_device_id": null,
    "mono_output": false,
    "system_volume_percent": 100,
    "microphone_volume_percent": 100,
    "microphone_muted": false
  },
  "gif": {"max_width":800,"max_colors":256,"optimize":true}
}
```

Screenshot region rectangles are logical coordinates local to the selected
display. Recording region rectangles use the existing integer `CaptureRect`.
Artifact values are `{"path":string,"width":number,"height":number,"kind":
"image"|"video"|"gif"}`.

## Discovery and screenshots

- `{"op":"describe","request_permission":false}` returns
  `{"displays":[DisplayDescriptor],"windows":[WindowDescriptor],"devices":[AudioDevice]}`.
  `request_permission` defaults to `false`; when true it may start the macOS
  screen-recording permission flow. It does not request microphone permission.
  Discovery returns an error without enumerating capture targets when the
  console is locked or inactive.
- `{"op":"microphone_permission","request":false}` returns
  `{"status":"authorized|not_determined|denied|unavailable","devices":[AudioDevice]}`.
  It needs no Screen Recording grant and captures no screen or microphone audio.
  `request` defaults to false. True requests microphone access only if macOS
  still allows a prompt; denied/restricted access requires System Settings.
  `unavailable` is the non-macOS test stub. Swift uses a separate permission
  queue; waiting for a TCC response must not block recording controls or safety ticks.
- `{"op":"session_status"}` returns `{"available":bool}` from the recording
  worker. It is the cheap pre-overlay check and does not enumerate targets or
  request permission.
- `{"op":"screenshot","target":TARGET,"cursor":false,"output_dir":"..."}`
  returns an image artifact. `TARGET` is `display`, `region`, `window`, or the
  `frozen_region` target below. A generated `Capture-*.png` is created without
  replacing an existing file.

### Private frozen region flow

This flow guarantees that selection and final pixels come from one capture:

1. `{"op":"freeze_create","display_id":"...","cursor":false}` captures once
   and returns `{"id":"UUID","path":"PRIVATE PNG","width":number,"height":number}`.
   `cursor` is baked into this frame when true. The path is only for the overlay
   preview; it is not an artifact and must never be put in history.
2. Publish with `{"op":"screenshot","target":{"type":"frozen_region",
   "freeze_id":"UUID","rect":{"x":10,"y":20,"width":300,"height":200}},
   "cursor":false,"output_dir":"..."}`. The bridge loads only its internally
   generated UUID directory, applies the persisted display scale to the logical
   rectangle, writes the public PNG, then consumes the private freeze. It never
   recaptures the desktop. `cursor:true` is rejected here because cursor choice
   belongs to `freeze_create`.
3. On cancel call `{"op":"freeze_discard","id":"UUID"}`. It returns `{}` and
   is idempotent. Invalid/non-UUID IDs are rejected, so discard cannot remove an
   arbitrary path.

If final image encoding/publication fails, the freeze remains for retry or an
explicit discard. A cleanup failure after successful publication does not turn
the successful artifact into an error; UUID-owned private freeze directories
older than 24 hours are pruned on the next `freeze_create`. Freeze directories
are mode `0700` and are never returned by `recover_list`.

## Recording

- `record_start`: `{"op":"record_start","options":RecordingOptions,
  "exclude_app":false,"output_dir":"..."}` returns status and starts capture
  immediately. The Swift flow owns any pre-start countdown UI. A selected
  microphone requires authorization before creating a recording draft; a
  persisted selection without permission returns an actionable error directing
  the user to Preferences. Start never waits for a permission prompt on the
  lifecycle worker or silently omits an unauthorized microphone.
- `record_pause`, `record_resume`, `record_restart`, `record_status`,
  `record_stop`, and `record_discard` take only `op`.
  `record_restart` discards the current take and prepares a fresh draft in
  `selecting`; the frontend runs its visible cancellable countdown and calls
  `record_resume` at zero. Cancelling that countdown calls `record_discard`.
- `record_mute`: `{"op":"record_mute","muted":true}`.
- Status is `{"state":"idle|selecting|recording|paused|finalizing|failed",
  "elapsed_ms":u64,"microphone_level":number,"microphone_muted":bool,
  "warning"?:string}`. With no session, `record_status` succeeds with idle,
  zero elapsed/level, and `microphone_muted:false`. Elapsed time excludes pauses.
- `record_stop` returns a video or GIF artifact. `record_discard` returns `{}`.

Pause and mute finalize the active segment before changing state. Resume starts
a new segment. Restart discards only the bridge-owned draft and starts over.
Every completed segment and manifest update is synced into
`CAPTURES_NATIVE_DATA/recording-drafts` (default
`~/Library/Application Support/Captures Native Experiment/recording-drafts`).
The dedicated recording engine checks console availability once per second and
before every queued state request. It auto-finalizes the active segment into the
draft before entering paused state when the console is locked or inactive.
Stateless image/media work does not occupy this worker. Recording publication
uses private same-filesystem staging and a no-replace hard link.

Current upstream limitations: `show_keystrokes` is accepted by
`RecordingOptions` but ScreenCaptureKit does not render keystroke overlays;
`countdown_seconds` is validated but the Swift app owns countdown timing. These
are not silently presented as engine features.

## Image and media editing

- `{"op":"image_encode","path":"...","output":"...","format":"png|jpeg|webp",
  "quality"?:1..100,"max_bytes"?:positive_integer}` returns an image artifact.
  Input must be a regular non-symlink image file. Output may be any caller-owned
  path, including a private temporary comparison path, but must not exist.
  Publication is a no-replace hard link from same-directory private staging.
  JPEG defaults to quality 92 and composites transparency onto white. WebP is
  lossless when quality is omitted and lossy when supplied. For JPEG/WebP,
  `max_bytes` tests qualities in descending order and returns the first fitting
  encoding no greater than `quality` (or 100), so the result is the highest
  tested fitting quality without assuming encoded sizes are monotonic. JPEG
  uses 4:4:4 sampling and the shipping ImageMagick quantization tables. PNG is
  always lossless; `quality` is validated but does not alter PNG pixels, and an
  unattainable `max_bytes` returns an error rather than resizing or
  misrepresenting success.
- `{"op":"media_probe","path":"..."}` returns
  `{"duration_ms":u64,"width":u32,"height":u32}`.
- `{"op":"media_export","path":"...","output":"...","format":"mp4|gif|webm",
  "start_ms":u64,"end_ms":u64,"crop"?:{"x":u32,"y":u32,"width":u32,"height":u32},
  "width"?:u32,"fps"?:1..30,"quality"?:"preserve|highest|high|standard|small|tiny",
  "max_bytes"?:u64,"system_volume"?:number,"microphone_volume"?:number,"mono"?:bool}` returns a
  video/GIF artifact. Times are milliseconds with `start_ms < end_ms` inside the
  source. Crop is in source pixels. Width is rounded down to an even value and
  height is derived from the cropped aspect ratio and made even. Volumes are
  finite multipliers from 0 through 2 and default to 1. `fps` is accepted only
  for GIF and defaults to the media toolchain choice. `max_bytes`, when present,
  must be greater than zero and applies the media toolchain's hard size budget.

`media_export.output` may be a private temporary path for before/after
comparison. The bridge treats it exactly like any other caller-owned output,
returns the normal artifact value, does not add it to history, and never removes
it; the caller owns its cleanup. All image/media exports reject an existing
destination both before work and atomically at publication. Sources are never
modified or deleted. WebM is in the request enum for forward compatibility but
the current `captures-media` backend does not implement it, so requests return a
descriptive error and create no output.

Media tools come from `CAPTURES_NATIVE_FFMPEG` and
`CAPTURES_NATIVE_FFPROBE`, independently, or default to `ffmpeg` and `ffprobe`
on `PATH`.

## Recovery

- `{"op":"recover_list"}` returns `{"drafts":[{"id":"UUID","name":"..."}]}`.
- `{"op":"recover","id":"UUID","output_dir":"..."}` assembles only
  contiguous, finalized segments, publishes a new artifact without replacement,
  then removes that owned draft. Failure retains the draft and records the error.
- `{"op":"recover_discard","id":"UUID"}` removes only the validated,
  bridge-owned draft directory and returns `{}`. It is idempotent.

`recover` and `recover_discard` reserve recovery, then synchronously ask the
recording worker for status. Any non-idle session rejects the operation; while
recovery remains reserved, `record_start` is rejected. This closes both races
without blocking the worker's console-safety loop during media assembly.

Recovery rejects absolute paths, parent traversal, symlinks, non-regular media,
mismatched UUID manifests, unsupported schemas, and complete segments appearing
after an incomplete one. An interrupted trailing segment is ignored; already
finalized segments remain recoverable.
