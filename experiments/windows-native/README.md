# Captures Windows native experiment

This standalone crate presents Captures with Win32 windows and custom Direct2D/DirectWrite chrome.
It initializes a D3D11 device and DirectComposition visual tree for every app instance, preferring
the hardware driver and falling back to WARP when hardware creation is unavailable. The selected
driver is written to `render-driver.txt` in the profile data directory. No webview, GTK, stock menu,
or stock confirmation dialog is used. It reuses the repository capture, session, recording, and
media crates. Data is isolated under
`%LOCALAPPDATA%\Captures Windows Native Experiment` (override with
`CAPTURES_WINDOWS_NATIVE_DATA`).

## Windows commands

From the repository root in Developer PowerShell for VS 2022:

```powershell
./experiments/windows-native/scripts/check.ps1
./experiments/windows-native/scripts/build.ps1
./experiments/windows-native/scripts/render-fixtures.ps1 -ImagePath <shared-sample.png> -VideoPath <shared-sample.mp4>
./experiments/windows-native/scripts/benchmark.ps1
```

`render-fixtures.ps1` launches real custom-rendered app routes and captures them using the Windows
desktop. The helper temporarily changes the primary display mode when needed and therefore must run
only in a disposable test desktop, not an interactive user session. It restores the previous display
mode and thread DPI context during cleanup. Its PNGs are runtime review artifacts and intentionally
ignored by Git. The Linux cross-check used during development is:

```sh
cargo check --manifest-path experiments/windows-native/Cargo.toml --target x86_64-pc-windows-gnu
```

## Experiment status

The capture overlay, screenshot persistence and clipboard path, mini preview, history storage,
recording start/pause/resume/restart/finalization, session safeguards, native file drag,
profile-scoped single-instance IPC, autostart, and custom-rendered routes are wired to the shared
Rust backends. The screenshot editor supports source-coordinate shapes, selection, crop, undo/redo,
delete, Segoe UI text entry, editable hex color, rectangle/ellipse/triangle/diamond/star shapes,
raster export, and copy. Its custom D2D layout follows the shipping editor hierarchy with a vector
tool rail and shape flyout, fitted canvas, layers/properties sidebar, and filename/format/export
footer. Filename edits, format changes, Save as new file, layer selection/visibility, copy, and Save
are functional. Replacing a source is allowed only while its sanitized filename and supported format
remain unchanged; staged bytes are flushed and synced before replacement. Canvas
dimension/background/zoom controls are currently read-only and Add images is explicitly unavailable.
Selected annotations can be moved, resized, and rotated, and their color, stroke, and supported fill
state can be edited with undo/redo.
The recording editor probes real media, decodes playback frames, seeks, trims, chooses quality,
exports through `captures-media`, and can either preserve the source or safely replace it. Probe,
paused-frame extraction, compression comparison, and export run outside the Win32 message thread.
Playback uses one long-lived FFmpeg child per play/seek interval to CPU-decode raw RGBA frames for
D2D presentation; it is **not** hardware-accelerated video presentation. Leaving the editor and
superseding frame/comparison/export work cancel active workers, while request generations prevent
stale results from changing the current editor. The shared probe API is asynchronous here but does
not yet expose child-process cancellation. Playback audio and split/video-crop controls remain
unavailable and are labeled as such. Opening the editor and changing its custom quality dropdown
automatically encode a one-second sample and show a real split-frame before/after comparison plus an
extrapolated size estimate. Any performance measurement must include FFmpeg child-process CPU and
memory.

Preview input uses a combined rounded Win32 window region, with `HTTRANSPARENT` only supplemental;
cross-process click-through still requires runtime verification on Windows hardware.

## Hardware verification checklist

- Test 100%, 150%, and mixed-DPI displays, including a display left of the primary.
- Lock Windows during selection and recording; selection must cancel and recording must not resume.
- Verify overlay/HUD/preview exclusion with the controls-in-captures settings both ways.
- Paste a transparent screenshot from the clipboard into Paint and an Office app.
- Exercise region, window, and display screenshots plus pause/resume recording with system and mic audio.
- Compare all runtime screenshots against the corresponding Tauri harness captures.
