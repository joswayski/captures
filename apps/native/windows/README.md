# Captures Windows host

The Windows replacement for the egui/wgpu workbench: Win32 windows,
DirectComposition, Direct2D, DirectWrite (system Segoe UI Variable) and UI
Automation, with Captures' own token-drawn controls. See
[native-rewrite.md](../../../docs/native-rewrite.md#host-direction) for the
direction and the gates it must pass. It has Preferences (every shipping
card with the Custom accent's color editor, the narrow layout below 720 px,
and Find on a reusable text field, `src/textfield.rs`), Capture History
(`--scene history`) and a first screenshot editor slice, opened by History's
Edit or `--scene editor --artifact ID` (`src/editor.rs`).

Recordings open from History's Edit or `--scene recording --artifact ID`.
The first slice (`src/recording.rs`) contains the header, dropped-frame warning
and save footer: filename editing, Change save location, Save as new file with
the `-edited` suggestion, replacement, progress, Cancel and Show in Folder.
The shared recording session and media tools run on a worker; closing cancels
work and waits for replacement/history cleanup before destroying the window.
The preview displays retained decoded frames, plays/pauses video with source
audio and Loop preview, and autoplays GIFs without video-only controls.
Fit contains the media; 100% maps source pixels to DIPs and scrolls with the
wheel (Shift+wheel horizontally). The media worker retains only its newest
frame while the UI is busy; pause/restart generations discard stale frames.
The scrollable page has a source-relative thumbnail timeline with draggable
trim handles, keyboard steps and UIA RangeValue. Output cards use the shared
recording-editor menus: original/1080p/720p/custom dimensions, GIF frame rate,
maximum width and quality presets (64/96/128/256 palette colors), plus separate
0–200% source-audio volumes, mute and mono. Custom dimensions export as even
pixels. Valid edits debounce for 250 ms; invalid drafts stay local until blur.
Save waits for the accepted preview settings, and failed previews restore those
settings. Numeric fields expose UIA Value, menus ExpandCollapse/SelectionItem,
volumes RangeValue and audio checkboxes Toggle. Output controls passed physical
pass 25; pass 26 accepted source-sized preview and playback independent of export
dimensions, including stable custom-field steppers and retained audio edits.
Graphical crop has eight handles and interior movement; numeric source-pixel
bounds, aspect lock, keyboard movement and UIA ranges share the crop geometry.
Crop passed its separate hardware check with observations; comparison still awaits
physical acceptance. Wine tests and offscreen renders are not that gate.
MP4 Preserve/Compress presets and GIF compression use
the shared encoded comparison, with a 6–94% split, keyboard/UIA range, Hide/Show,
processing/error feedback and estimated size badges. New edits cancel old work;
only matching generations, accepted revisions and source positions can display.
Comparison hides during MP4 playback; crop controls remain above it.
GIF comparison is a static sample; Hide restores source autoplay.
Maximum file size uses the shared MP4/GIF hard-budget exporter, decimal KB/MB/GB,
a 10 MB default and a 100 KB minimum. Size drafts stay untouched on blur; Save
reports an invalid draft instead of exporting the last accepted cap. Unit changes
preserve exact whole bytes (unlike Tauri's eight-significant-digit formatter).
The comparison displays the cap rather than an estimate. Replacement resets to
source defaults and clears the cap. Physical pointer, UIA and DirectComposition
acceptance for these controls remains open. The filename's Format menu switches
between MP4 and GIF, preserving trim, crop, audio settings and size limits.
Changing formats forces a new file instead of overwriting the source; returning
to the source format allows replacement again. GIF-only frame rate/palette
settings do not leak into MP4 exports, and source playback stays independent of
the export format. Videos honor the saved output-format preference; GIF sources
start as GIF. WebM remains selectable to match Tauri, but saving it reports
unsupported and writes no file. The menu shares the existing keyboard and UIA
ExpandCollapse/SelectionItem behavior; physical interaction acceptance remains open.
The filename exposes UIA Value, the copy switch
and preview modes expose Toggle, and actions Invoke. Timeline/playback passed
physical pass 24; Narrator/Text-pattern and recording-editor parity remain open.

```powershell
node apps/native/prepare.mjs --output apps/native/windows/resources
cargo +1.95.0 run --manifest-path apps/native/windows/Cargo.toml -- `
  --profile $env:TEMP\captures-windows
```

Run it against a disposable profile, never your real settings or folders:
`--profile DIR` (or `CAPTURES_NATIVE_PROFILE=DIR`) keeps settings in
`DIR\settings.json`, history in `DIR\history` and a new profile's saves in
`DIR\Captures`, and never writes the launch-at-login Run key. It replaces
`--settings-file` and `--history-root`, which remain for CI and fixtures.

`--history-root DIR` enables Start Captures on login for that development
profile. `--appearance light|dark`, `--size WxH` and `--screenshot out.png`
render one Preferences, History, screenshot-editor or recording-shell frame offscreen
(Direct2D bitmap, WIC PNG) and exit, for parity
comparisons; `--find QUERY` renders it with Find open and the query typed, and
`--tool KEY` starts the editor with a rail tool or shape, `--select LAYER`
with that layer selected, and `--crop X,Y,W,H` with that crop staged. On other platforms the binary is a stub, so `cargo check` and
`clippy` still run; cross-check with `--target x86_64-pc-windows-gnu`.

Recordings need FFmpeg and FFprobe. `CAPTURES_FFMPEG` and `CAPTURES_FFPROBE`
override discovery; otherwise the host looks beside its executable, in Tauri's
`apps/desktop/src-tauri/binaries` (`*-<arch>-pc-windows-msvc.exe`), then on PATH
as `ffmpeg.exe` and `ffprobe.exe`. No tools are downloaded by the host.

On Linux (and in orbs), the tests also run under Wine 8 with a disposable prefix.
Run `node apps/native/prepare.mjs --output apps/native/windows/resources` first.
The prefix needs DejaVu copies renamed to Segoe UI/Consolas plus Wine's
Tahoma in `drive_c/windows/Fonts`, *registered* under
`HKLM\Software\Microsoft\Windows NT\CurrentVersion\Fonts` (Wine's DirectWrite
ignores unregistered files there, and every layout test then fails with
`0x80004005`), and a MinGW-built `bcryptprimitives.dll` exporting `ProcessPrng`
with `WINEDLLOVERRIDES=bcryptprimitives=n,b`. Build with
`cargo +1.95.0 test --target x86_64-pc-windows-gnu --no-run`, then run the test
executable under `wine` with `--test-threads=1`, an Xvfb `DISPLAY`, and
`CAPTURES_FFMPEG`/`CAPTURES_FFPROBE` pointing at Windows FFmpeg from a Preview
installer (`7z x`). In a 4 GB orb, run clippy with `-j1` and not alongside other
builds. These shims are test-only; never package them.

Each scene node (`src/scene.rs`) owns one composition surface. Hover and
toggle transitions animate visual opacity and offsets on the compositor, and
scrolling moves the card column's visual, so a frame redraws only nodes whose
content changed (in Preferences, on hover, at most one 32×19 switch track).
The recording shell retains each footer control separately; progress repaints
only the small progress/status surfaces and the caret blinks on the compositor.
