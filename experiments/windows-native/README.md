# Captures Windows native experiment

This standalone crate presents Captures with Win32 windows and custom Direct2D/DirectWrite chrome.
It initializes a hardware D3D11 device and DirectComposition visual tree for every app instance; no
webview, GTK, stock menu, or stock confirmation dialog is used. It reuses the repository capture,
session, recording, and media crates. Data is isolated under
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
desktop. Its PNGs are runtime review artifacts and intentionally ignored by Git. The Linux
cross-check used during development is:

```sh
cargo check --manifest-path experiments/windows-native/Cargo.toml --target x86_64-pc-windows-gnu
```

## Experiment status

The capture overlay, screenshot persistence and clipboard path, mini preview, history storage,
recording start/pause/resume/restart/finalization, session safeguards, native file drag,
profile-scoped single-instance IPC, autostart, and custom-rendered routes are wired to the shared
Rust backends. The screenshot editor supports source-coordinate shapes, selection, crop, undo/redo,
delete, Segoe UI text entry, editable hex color, rectangle/ellipse/triangle/diamond/star shapes,
raster export, and copy; moving/resizing/rotating selected shapes and richer fill controls remain.
The recording editor probes real media, decodes playback frames, seeks, trims, chooses quality,
exports through `captures-media`, and can either preserve the source or safely replace it. Its first
correctness path launches FFmpeg for CPU-decoded PNG frames and presents those frames through D2D;
it is **not** hardware-accelerated video presentation. Playback audio, non-blocking export,
and split/crop controls remain unavailable and are labeled as such. Opening the editor and changing
its custom quality dropdown automatically encode a one-second sample and show a real split-frame
before/after comparison plus an extrapolated size estimate. Any performance measurement must include
FFmpeg child-process CPU and memory.

Preview input uses a combined rounded Win32 window region, with `HTTRANSPARENT` only supplemental;
cross-process click-through still requires runtime verification on Windows hardware.

## Hardware verification checklist

- Test 100%, 150%, and mixed-DPI displays, including a display left of the primary.
- Lock Windows during selection and recording; selection must cancel and recording must not resume.
- Verify overlay/HUD/preview exclusion with the controls-in-captures settings both ways.
- Paste a transparent screenshot from the clipboard into Paint and an Office app.
- Exercise region, window, and display screenshots plus pause/resume recording with system and mic audio.
- Compare all runtime screenshots against the corresponding Tauri harness captures.
