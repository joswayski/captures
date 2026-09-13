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
ignored by Git. In addition to prepared visual states, the editor input-smoke fixture changes the
canvas width, selects a shape, and draws it through real HWND keyboard and pointer messages. A second
input-smoke fixture selects Eraser, switches to Erase, and paints through the same native pointer
route. A Trim input smoke hides the locked original layer and invokes Trim edges through real HWND
hit targets, failing unless the expected asymmetric output dimensions are committed. The resulting
images still require inspection and are not substitutes for broader
accessibility, IME, physical-pointer, or hardware-input testing. The Linux cross-check used during
development is:

```sh
cargo check --manifest-path experiments/windows-native/Cargo.toml --target x86_64-pc-windows-gnu
```

## Experiment status

The capture overlay, screenshot persistence and clipboard path, mini preview, history storage,
recording start/pause/resume/restart/finalization, session safeguards, native file drag,
profile-scoped single-instance IPC, autostart, and custom-rendered routes are wired to the shared
Rust backends. The screenshot editor supports source-coordinate shapes, selection, crop,
geometry-based Trim edges, undo/redo, delete, Segoe UI text entry, editable hex color,
rectangle/ellipse/triangle/diamond/star shapes,
raster export, and copy. Its custom D2D layout follows the shipping editor hierarchy with a vector
tool rail and shape flyout, fitted canvas, layers/properties sidebar, and filename/format/export
footer. Filename edits, format changes, Save as new file, layer selection/visibility, copy, and Save
are functional. Replacing a source is allowed only while its sanitized filename and supported format
remain unchanged; staged bytes are flushed and synced before replacement. Canvas
dimension/background and fit/manual zoom/pan controls are functional. Export settings choose original,
percentage, or aspect-locked custom dimensions and preserve, compressed, or maximum-size encoding;
preview rendering and exact encoded-size estimates run outside the message thread. Add images opens the native multi-file
picker, decodes selected images off the message thread, and adds independently selectable raster
layers through the shared `captures-image` renderer. Imported layers preserve aspect ratio and can
be moved, resized, rotated, hidden, exported, and undone/redone. The picker filters to the image
formats supported by the experiment, and a multi-select import is committed as one undo step.
The current native chrome uses the shipping editor's icon-led tool rail, separate Arrow tool,
three-column shape flyout, empty default inspector, locked-background layer row, grouped canvas/zoom
header, and filename/export footer. Switches share the shipping 30-by-18 geometry and have distinct
on, off, and disabled states. Light and dark runtime fixtures cover default, imported-image, shapes,
export, selected-properties, selected-line, merge/combine, source-consumed, flattened-source,
erased-source, and trim-ready editor states.
Selected annotations can be moved, resized, and rotated, and their color, stroke, and supported fill
state can be edited with undo/redo. Selected layers can also change opacity and blend mode, move to
the front or back, duplicate, delete, lock, hide, and—when they are images—be renamed.
The layer Combine menu implements Merge down for an unlocked adjacent pair, Merge visible while
retaining hidden-layer order, and Flatten image with the canvas background baked into one locked
source. Each operation rasterizes through `captures-image` and is one undoable transaction.
The Eraser tool targets the topmost visible image (including locked image layers and the locked
original screenshot). Its contiguous or global color wand, continuous soft erase brush, and restore
brush edit real alpha pixels, clear an active solid canvas background, and commit each action as one
undoable transaction. Restore uses pixels frozen before that image's first alpha edit. Brush points
are collected continuously, but the edited preview is currently presented only after pointer-up;
live during-drag brush presentation remains outstanding.
The locked original screenshot can be hidden without making it editable. Trim edges fits the canvas
to every visible layer's axis-aligned bounds (locked layers included, hidden layers ignored), retains
content that overhangs the old canvas, and is one undoable action. Nonfinite bounds, dimensions over
16,384 pixels, and frames over 100 million pixels are rejected before document mutation.
The recording editor probes real media, decodes playback frames, seeks, trims, chooses quality,
exports through `captures-media`, and can either preserve the source or safely replace it. Probe,
paused-frame extraction, compression comparison, and export run outside the Win32 message thread.
Playback uses one long-lived FFmpeg child per play/seek interval to CPU-decode raw RGBA frames for
D2D presentation; it is **not** hardware-accelerated video presentation. Leaving the editor and
superseding frame/comparison/export work cancel active workers, while request generations prevent
stale results from changing the current editor. The shared probe API is asynchronous here but does
not yet expose child-process cancellation. Playback audio and video-crop controls remain
unavailable and are labeled as such. The native recording editor uses one thumbnail track, two trim
handles, and one playhead; it does not draw a second decorative trim control. Opening the editor and
changing its custom quality dropdown automatically encode a one-second sample and show a real
split-frame before/after comparison plus an extrapolated size estimate. Any performance measurement
must include FFmpeg child-process CPU and memory.

General, Capture, Recording, Shortcuts, and Appearance preference pages are native routes. Changed
values persist through the experiment profile. Screenshot format/freeze, recording format/rate/cursor/
system audio/editor handoff, and capture-exclusion appearance values are consumed by native paths;
screenshot countdown/cursor are not yet applied during capture. Shortcuts are currently read-only and
registration conflicts fail closed. Microphone selection, GIF-specific limits, custom shortcut
editing, and custom accent entry remain unavailable.

Preferences links to an explicit native feedback form. It sends only the typed message, optional
contact, selected category, and bounded app/platform context after the user presses Send. A single
off-thread `captures-feedback` client enforces transport validation, timeout, no redirects, and local
cooldown; no feedback or telemetry is sent at startup. Fixture rendering never submits the form.
Crash collection and crash-reporting consent are not implemented, so raw panic text is never sent.

Editable screenshot drafts are not yet persisted or restored: closing or restarting the native app
loses the in-memory document even though raster exports remain on disk. Future draft storage must
remain inside the isolated experiment profile and retain whether the source was an imported file or
a new capture. Recording-editor drafts remain intentionally absent, matching the shipping app.

Image flip/rotate layer actions, recording crop, and preview audio are not implemented. Their controls are
omitted or explicitly marked unavailable rather than presented as working. Windows DirectComposition
fixtures are required to assess final pixel-level parity; they cannot be rendered in the Linux orb.

Preview input uses a combined rounded Win32 window region, with `HTTRANSPARENT` only supplemental;
cross-process click-through still requires runtime verification on Windows hardware.

## Hardware verification checklist

- Test 100%, 150%, and mixed-DPI displays, including a display left of the primary.
- Lock Windows during selection and recording; selection must cancel and recording must not resume.
- Verify overlay/HUD/preview exclusion with the controls-in-captures settings both ways.
- Paste a transparent screenshot from the clipboard into Paint and an Office app.
- Exercise region, window, and display screenshots plus pause/resume recording with system and mic audio.
- Compare all runtime screenshots against the corresponding Tauri harness captures.
