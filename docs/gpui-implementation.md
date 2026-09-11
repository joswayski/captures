# GPUI implementation

`experiments/gpui` is a separate Rust application using published **GPUI 0.2.2**
for custom windows and GPU rendering. It reuses Captures' capture, recording,
media, and session crates, shared CSS design tokens, and editor icon geometry.
It does not embed React, a browser, or GTK widgets. Cairo rasterizes editable
image documents; FFmpeg handles media decoding and export.

**This is not yet a feature-equivalent replacement for the shipping app.** It
implements the capture/editor/recording workflow, rather than a static launcher,
but the gaps and unverified combinations below still matter. The Tauri Preview
build, installers, and update channel are unchanged. The shared UI now has native
adapters for Linux/X11, macOS, and Windows. Linux/X11 has rendered workflow checks;
macOS and Windows runtime verification remains pending. Wayland is still gated out.

## Implemented surfaces

| Surface | Behavior | Evidence and limits |
| --- | --- | --- |
| Desktop shell | Tray, eight configurable global shortcuts, private single-instance IPC, optional login startup, file opening, light/dark/system and accent settings | IPC forwarding, fragmented commands and lock release are tested. A hidden unmapped keeper window prevents GPUI 0.2.2 from exiting when its last visible window closes. GNOME/KDE tray integration still needs physical-desktop testing. |
| Capture selector | Frozen/live region, window and display targeting; region move/resize, aspect ratios, Shift square, display picker, draggable fixed-glass toolbar, screenshot/video/GIF options, countdown, auto-start and Escape | Integrated X11 check captures a real 730×450 region and independently compares every pixel. It also checks IPC reopening, lock cancellation, Escape, and zero-visible-window lifetime. Multi-monitor/HiDPI and window targeting need hardware validation. |
| Mini previews | Collapsed/expanded piles, hover controls, corner/gravity movement, rotated cached cards, blur, dismissal, confirmed deletion animation, copy/save/reveal, input-shape click-through | Live X11 Shape and native `text/uri-list` clipboard tests; bounded decoded/rotated caches; asynchronous generation-safe decoding. External file dragging to another app is **not implemented**. |
| Screenshot/canvas editor | Layered image/text/shapes/freehand, selection/transforms, crop, erase/restore/wand, undo/redo, visibility/lock/reorder/merge, custom colors, typography/shadows, transparency, zoom, private drafts, PNG/JPEG/WebP export and encoded comparison | Tests exercise real encoders, alpha, size constraints, layers, geometry and draft identity. Native text input and pointer coordinates were exercised. Full gesture, animation and accessibility equivalence is not established. |
| Recording | Countdown, xcap segments, pause/resume, microphone rollover, screenshot callback, restart/discard/quit, hide/restore controls, region guide, lock auto-pause, private manifests and recovery | Real X11 recording produced 66.298s of H.264 across two segments. The combined-app check also records 730×450, verifies all four guide bounds and preview exclusion, cancels an actual screenshot selector and resumes, auto-pauses on an isolated DBus lock event, restarts the process and recovers a probed MP4 through the UI. No session-check bypass is present. Hardware audio remains unverified. |
| Recording editor | Persistent FFmpeg frame pipe, seek/loop, draggable trim/crop with aspect lock, resizing, per-track controls, estimates/comparison, cancellable export/progress, source replacement staging | Real MP4/GIF encoding tests and pointer trim/crop checks. Old images are evicted during playback. No hardware AV-sync validation; native picker/source replacement/recovery prompts were not all exercised end-to-end. |
| Preferences/history | One scrolling settings document, anchored sidebar, Ctrl+F text search, shortcut editing, async audio-device enumeration; 30-day private history copies, filters/paging, restore, reveal and confirmed cleanup | Persisted light/dark/accent interactions and video filtering/deletion confirmation exercised. Tests cover independent recovery copies and preservation of externally saved files. Updater/feedback explicitly unavailable; interrupted-recording drafts have their own recovery UI, not history rows. |

## Data and safety

The GPUI profile uses `$XDG_DATA_HOME/captures-gpui` on Linux (normally
`~/.local/share/captures-gpui`), `~/Library/Application Support/captures-gpui`
on macOS, and `%LOCALAPPDATA%/captures-gpui` on Windows. `CAPTURES_GPUI_DATA`
overrides that location. Private storage uses owner-only Unix permissions or
Windows ACLs; settings replacement is atomic. It never
migrates or writes the Tauri or GTK experiment's settings/history. Quit other
Captures builds before testing overlapping shortcuts. The GPUI build does not
clear another application's OS shortcut assignments.

New screenshots are private lossless PNGs under `unsaved/`, with independent
history recovery copies. Capture does not automatically publish a file to the
selected output directory. Preview **Save** encodes the selected PNG/JPEG/WebP
format and publishes a new non-overwriting file. History Restore opens the recovery
copy as an unsaved preview without adding another history row. Preview dismissal,
Clear all, and unsaved-preview deletion preserve history; deleting a published
preview trashes the published file but preserves history. Confirmed history
cleanup removes private copies and private unsaved originals, never external saved
sources. Recording drafts are private and recovery validates path confinement.

Capture and recording use the shared fail-closed session gate. The test desktop
supplies an isolated `org.freedesktop.ScreenSaver` service because Xvfb has no
logind session; it does not disable the production gate. On Linux, controls can
appear in recordings: use Hide controls when needed. Window-exclusion options
cannot make X11 provide capture exclusion it does not support.

macOS recording uses the shared ScreenCaptureKit backend rather than xcap's
recording path. Screen/microphone permission requests recheck session safety
after the prompt. ScreenCaptureKit's whole-app exclusion applies when both
preview and recording-control inclusion are off; mixed inclusion cannot be
represented by this backend's current app-level filter. The AppKit sharing flag
is only a legacy best-effort hint, not a guarantee for modern capture APIs.
Windows applies native display affinity to excluded preview/control windows.
These native branches still need real desktop capture tests.

The UI remains shared: platform adapters handle tray/menu integration, shortcuts,
file clipboard/reveal, activation, login startup, and privacy. File copying uses
X11 URI targets, an AppKit file URL, or Windows `CF_HDROP`; clipboard support does
not implement cross-application dragging. Media tools resolve relative to bundles
and standard installed locations without changing PATH. Fresh profiles use the
shipping app's OS-specific shortcuts; existing custom bindings remain unchanged.

The parent integration repairs GPUI 0.2.2's forced X11 popup decorations only for
this process's notification windows. Region guides are positioned after mapping
and have empty native input regions, because initial bounds alone allow a window
manager to relocate these very thin windows. HUD button presses do not bubble
into the window-move handler. These details are checked in the real desktop,
not inferred from the GPUI element tree.

## Remaining parity and platform work

- Native cross-application XDND file dragging is missing. GPUI's in-app drag API
  is not a native drag source; clipboard support is not a substitute for it.
- WebM export returns a visible unsupported error from the shared media
  backend. MP4 and GIF export are implemented.
- Test-only native packages and PR build jobs are available through the
  [platform helpers](../experiments/gpui/platform/README.md). There is no GPUI
  distribution/update channel, feedback transport, crash diagnostics, or
  shipping onboarding/update/launch-notice workflow. macOS bundles do not yet
  advertise Finder document associations; CLI/file-URL routing is not proof of
  native Open With event handling.
- A finalized mixed audio stream cannot be split back into independent system
  and microphone tracks. Available separate tracks are handled separately;
  ffplay failure currently does not surface a warning in the editor.
- Not every React animation, reduced-motion setting, keyboard traversal,
  screen-reader semantic, or editing gesture has an equivalent verified GPUI
  implementation. This must not be advertised as exact visual/interaction parity.
- Crop gestures currently begin on the image canvas, not the surrounding gray
  viewport. Large-image responsiveness, long recordings, real audio, native save
  portals, multiple monitors, scaling, GNOME/KDE compositors and physical Linux
  desktop lock transitions need testing. Source image opening decodes asynchronously.

## Release resource comparison

Measured on September 11, 2026 in a two-vCPU Linux orb using Mesa 25.0.7
llvmpipe, Xvfb (1600×1000), Openbox and xcompmgr. Both applications were release
builds; Tauri embedded the production frontend with `tauri/custom-protocol`.
These measurements describe the original Linux baseline, before the subsequent
editor-parity and platform-adapter changes; they are not refreshed measurements.
Preferences used matching 980×720 client windows; image editors used 1280×760
windows with the same 960×540 fixture. Both used fresh isolated profiles, dark
appearance and software rendering. The screenshots were inspected for actual
loaded content, not merely mapped windows.

Each state had one visual warmup per frontend and five measured trials in
alternating frontend order. Memory covers the entire application process tree
after five seconds of settling; CPU is measured over the following two seconds.
Values below are medians. PSS apportions shared pages; summed RSS double-counts
pages shared between processes, so PSS is the more useful memory comparison.

| State | Frontend | PSS MiB | Private MiB | RSS MiB | Processes | First mapped window, ms | Idle CPU, % of one core |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Preferences | Tauri | 595.3 | 440.0 | 1197.4 | 6 | 1273 | 21.0 |
| Preferences | GPUI | 161.1 | 159.0 | 167.2 | 1 | 647 | 2.5 |
| Image editor | Tauri | 698.0 | 525.2 | 1326.1 | 6 | 1335 | 44.0 |
| Image editor | GPUI | 171.4 | 169.4 | 177.3 | 1 | 646 | 1.5 |

GPUI's median PSS was 73% lower for Preferences and 75% lower for the image
editor in this fixture. **First mapped window is not first useful paint or
readiness.** These are different implementations with remaining parity gaps,
not a controlled replacement of only the rendering engine. Software-rendering
CPU and memory results must not be extrapolated to physical GPUs, battery life,
macOS, Windows, Wayland, or a full recording/editing workload. The standalone
GPUI executable is also slightly larger (37.0 MiB versus 34.4 MiB); these sizes
exclude shared libraries and media tools and are not installer sizes.

[Raw samples, ranges, environment and binary/input hashes](../experiments/gpui/results/linux-release.json)
are checked in with the measurement scripts. The source-base field identifies
the unchanged Tauri base; the GPUI implementation is the accompanying PR, not
that base commit.

### Selection motion to observed pixels

The same release binaries were sampled separately with `latency.py`, with no
competing builds or UI checks. Each frontend had two discarded warmups and ten
measured trials, each using a new process and selector. After activating the
selector and settling for three seconds, the harness presses at (320, 180),
waits 80 ms, timestamps and injects motion to (1050, 630), then polls a 13×9 root
window patch for the yellow top border. A baseline-yellow patch invalidates the
trial; five-second timeouts are failures, not fast frames. Both resulting
730×450 selections were visually inspected to verify the detector's target.

| Frontend | Successful trials | Median, ms | Min–max, ms | Nearest-rank p95, ms |
| --- | ---: | ---: | ---: | ---: |
| Tauri | 10/10 | 297.0 | 245.0–391.5 | 391.5 |
| GPUI | 10/10 | 123.0 | 77.6–142.3 | 142.3 |

Raw results: [Tauri](../experiments/gpui/results/latency-tauri.json) and
[GPUI](../experiments/gpui/results/latency-gpui.json). Median idle polling overhead
was 0.140 ms and 0.136 ms respectively. This measures the first drag motion's
**event-to-observed-software-compositor-pixel latency**, including scheduling and
polling effects, not steady-state drag FPS or physical display latency. The
80 ms pointer-down delay does not prove either input queue has drained. Tauri
trials ran first, then GPUI; unlike the resource trials, these were not interleaved.
Ten trials do not establish a reliable tail-latency distribution.

## Rendered comparison and interaction evidence

The updated editor uses the shipping layout's left SVG tool rail, centered
fit canvas, right Layers/contextual-properties panel, and grouped export controls.
The following release renders were inspected after the platform/parity changes.
Pointer checks exercised the six-shape flyout, numeric Apply/Escape, export and
format menus, appearance switching, and resizing to 980×650 with properties
scrolling. The smaller dark window shows retained custom export dimensions after
a canvas resize; window, canvas, and export dimensions are independent.

![Updated light editor with shape flyout](images/gpui/editor-parity-light.png)
![Updated dark editor at 980×650 with scrolled properties](images/gpui/editor-parity-dark-small.png)

These are actual application screenshots, not generated design concepts.
The historical comparison pairs below place release Tauri on the left and the
original pre-parity GPUI build on the right at identical client dimensions.
They correspond to the benchmark baseline, not the updated editor above. Light
mode is an additional visual check, not another performance dataset. None of
these images establishes exact visual or interaction parity.

![Dark Preferences: Tauri left, GPUI right](images/gpui/preferences-dark.png)
![Dark image editor: Tauri left, GPUI right](images/gpui/image-dark.png)
![Light Preferences: Tauri left, GPUI right](images/gpui/preferences-light.png)
![Light image editor: Tauri left, GPUI right](images/gpui/image-light.png)

The integrated recording check exercised the frameless HUD and lock auto-pause.
Focused native interaction checks also exercised the custom color picker and
rotated preview pile. Their screenshots illustrate those states; the tests
and interaction checks above establish only the behaviors they actually ran.

![Integrated recording paused by an isolated session lock](images/gpui/recording-lock.png)
![Custom color picker after actual saturation, hue and alpha clicks](images/gpui/editor-color.png)
![Collapsed preview pile with rendered rear-card rotation](images/gpui/previews.png)

## Verification performed

- `npm run check`: passed, including 830 desktop tests and production web builds.
  The orb run disabled inherited Git signing only for the command's disposable
  test repositories (`GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=commit.gpgsign
  GIT_CONFIG_VALUE_0=false`); the initial run could not sign their fixture commits.
- Root `cargo fmt --all -- --check`, `cargo test --workspace` and strict workspace
  Clippy: passed; 366 Rust tests passed, one ignored.
- GPUI locked tests: 86 passed, including the live X11 Shape/clipboard suite;
  formatting, strict all-target Clippy and release build passed.
- Python measurement-helper tests: six passed.
- Platform helper policy, shell syntax and workflow formatting checks passed.
  Full native macOS/Windows builds and runtime checks were unavailable in the
  Linux orb; isolated exact-dependency adapter type checks do not substitute
  for the new native CI jobs or physical-desktop testing.
- `capture_check.py`: exact 730×450 pixel comparison; private history and no
  automatic publication; IPC, Escape, lock cancellation and tray lifetime;
  failed asynchronous source opening cannot expose a placeholder Save action.
- `recording_check.py`: actual region recording and guide bounds, preview
  exclusion, screenshot-cancel resume, lock-preserved segments, and process
  restart followed by recovery into a probed 730×450 MP4.

These checks ran in the isolated Linux/X11 fixture. They do not cover the
physical-desktop and product gaps listed above.

Build and repeatable test commands are in [DEVELOPMENT.md](../DEVELOPMENT.md#gpui-implementation).
