# Shared wgpu renderer candidate

An **experimental Windows/Linux native workbench**, not the chosen production UI
or a replacement download. macOS keeps its Swift/AppKit frontend. This candidate uses
Rust, winit native windows, egui custom-drawn controls, and wgpu. It has no WebView,
JS runtime, Tauri dependency, network
service, installer, or updater. Node is build-time only.

Preferences now uses shared Rust settings persistence and custom-theme math,
including automatic save, retry, and a flush when the window closes. It uses a
separate Captures Native development identity; pass `--settings-file PATH` to
use an explicit test file. Screenshots and scripted exercises without that flag
use disposable settings. `--live` opts into the shared Rust New Capture controls
plus direct full-display, region and window PNG, history, copy, export and delete
flows; see the [live slice and limits](../README.md#live-display-capture-slice).
Live mode also provides a native tray menu for New Capture, those three direct
capture modes, History, Preferences, the output folder and Quit. Its persisted
display, region, window, recording and New Capture shortcuts work globally except while
capture is unavailable or a focused Preferences window is editing them. All seven
shortcut rows can be edited from Preferences with
physical-key recording, modifier previews, Escape/blur cancellation, and inline invalid
chord errors. Recording keys open Record on the requested target without starting a take.
Inside New Capture, Screenshot/Record and region/window/display controls share the
existing prepared selector. Region/window/display keys switch the existing selector's
mode and target. Screenshot keys return to Screenshot mode. Keyboard Full screen does not auto-start; preparation/countdown/capture
reject target keys, and New Capture cannot re-enter the active selector.
Fixture scenes can edit disposable shortcut settings but never register global shortcuts.
Windows tray left-click opens Preferences.
Live capture applies automatic copy, screenshot countdown, cursor inclusion, and
PNG/JPEG/WebP output format/folder preferences. Region selection also applies
freeze-screen and auto-start-on-selection preferences, retains one shared
`RegionSession` through confirmation, and uses the shipping shared drag/aspect
geometry. Window selection retains one shared `WindowSession`, uses its shipping
frontmost-window/shell hit testing and source-safety policy, and treats shell or
empty-desktop clicks as display capture. It applies freeze-screen and
auto-start-on-selection preferences; otherwise a clicked target stays selected
until Capture or Enter confirms it. Frozen and live window selection both refresh
after a nonzero countdown. A successful screenshot can join a fixed-glass native
preview stack in any preference-selected corner, with per-card full-resolution Copy,
Save, history selection and nondestructive Dismiss actions. Expanded overflow scrolls
without dropping cards; the stack can collapse or be cleared without deleting captures.
It is excluded from captures by default and retained when the include-in-captures
preference is enabled.
Record creates H.264 MP4 recordings with the stored FPS, maximum resolution,
countdown, cursor, click-highlight, desktop-audio and microphone defaults where
the current platform reports support. Pause/resume, confirmed Restart using the
stored countdown, Stop and Discard run from a
fixed-glass native HUD. Hide removes that HUD while preserving the current take and shows
a temporary noninteractive fixed-glass notice. The tray, app reactivation, or configured
New Capture shortcut restores it; Hide is disabled if no tray restore path exists, and Linux
tray-host loss restores the HUD and workspace. Microphone mute/unmute rotates the active segment without
changing the selected device or global preference, while paused changes remain
paused; mic-less sessions explain why the control is unavailable. Successful output is listed in native History with its
poster and metadata. **Edit recording** opens a decoded-frame editor with staged
graphical/numeric trim, numeric crop/output size, audio settings and MP4/GIF
save-new-copy. Trim grips share the shipping pointer geometry and support focused
arrow/Page Up/Page Down keys. Dragging changes staged values, not decoded frames;
Apply publishes the preview before seeking, estimating or saving. The trim track
retains full-source thumbnails across edits and seek. Generation has independent
cancel/retry and does not change accepted preview or History state. **Play/Pause**
provides silent playback of the accepted trim using one persistent shared decoder,
at up to 30 fps and 1280 × 720. A single latest-frame slot prevents queued stale
frames. Pause retains the last displayed frame; EOF makes the next Play restart
the trim. Focus loss/minimize pauses, and close waits for teardown before checking
unsaved edits. Seek/edit/save/estimate wait for playback to stop; playback never
changes accepted edits or History. **Loop preview** defaults off and can be changed
while playing; turning it off finishes the current lap, while Pause stops it.
Each lap reopens the accepted trim after the previous decoder finishes teardown.
Audio playback remains open. See the
[recording-editor status](../../../docs/native-rewrite.md#recording-editor-first-wgpu-host-not-playback-parity)
for accepted-frame, export and platform limitations.
After finalization, a fixed-glass Recording ready notice offers Save file using
the current output folder, followed by Show in Folder for the saved copy. It does
not activate the root; hidden-root actions and expiry work through one-shot
wakeups. The 15.2-second expiry pauses during a save and resets after its result;
errors allow retry. Dismiss/expiry never delete media, and a new capture clears
the notice. This is a finalization trigger until the native editor is connected.
A passive region guide remains visible through countdown, pause, restart and
hidden controls. Its veil and accent border are painted strictly outside the
recorded rectangle, with outward pixel rounding at fractional scale. It accepts
no input and closes with the recording; display/window recordings have no guide.
The private-X11 recording smoke checks input passthrough, clean inner-edge pixels,
decoded output, Hide/restore preservation and end/cancellation cleanup. Windows
and physical macOS/compositor/mixed-DPI acceptance remain open; Wayland stays gated.
Windows/X11 use the shipping
synthetic cursor arrow, not the actual system cursor image. Other capture defaults remain unconnected;
other scenes remain fixtures. The selector fixture handles window-focused Escape
only, while live capture uses the shared process-wide Escape cancellation handler.
Preferences → About → Send feedback uses the shared Rust client on a separate
worker. Only explicit Send in `--live` can contact captur.es; fixture mode keeps
submission disabled. The form previews the included app/system context, retains
drafts on errors and navigation, and prevents duplicate sends while pending.
Captures, files, and diagnostics are never attached. Login and updating remain
visibly unavailable.

Feedback input/retry verification uses a rejecting loopback proxy, never the
production service: `python apps/native/x11_feedback_smoke.py --binary
apps/native/wgpu/target/release/captures-wgpu-workbench --output feedback-smoke`.
It runs on private X11/software GL and checks empty/pending submission gates,
retained text through navigation and failure, offline retry, and clean exit in
both appearances. Shared client tests cover HTTP success, cooldown and payload
privacy against disposable loopback servers. Physical input/AT acceptance remains open.

History → Edit screenshot opens a worker-owned crop/canvas/draft editor. Undo/redo,
Save draft, confirmed Discard edits, and unsaved-close choices preserve the original
capture and exports. Closing without saving preserves any older saved draft.
Geometry → Draw crop selects on the preview, including reverse and outside-image
drags. Free, 1:1, 4:3, 3:2 and 16:9 presets use shared Rust geometry; hold Shift
to lock the current ratio. Apply crop commits; Cancel, Escape or switching panels
abandons the selection without editing or saving. Numeric crop fields remain usable.
The Layers panel selects front-to-back layers and connects visibility, locking,
image rename, opacity, X/Y movement, duplicate, delete and adjacent ordering.
Shared Rust preserves locked boundaries and makes every accepted edit undoable.
Draw adds filled rectangles/ellipses, straight lines, tapered arrows and freehand Pen strokes with the
shipping default annotation color and rounded rectangle corners. Drag previews are
transient; release creates one selected layer and undo step. Escape, focus loss,
close or switching panels cancels
the unfinished drag. Reverse and off-canvas drags use document coordinates; partial
overhang stays clipped and fully outside shapes expand the canvas. Zero-width or
zero-height closed shapes add nothing; lines retain horizontal, vertical and
zero-length gestures. Arrows require at least 1.5 document pixels and 3 screen pixels.
Arrow previews triangulate the shared tapered polygon rather than approximating
its arrowhead. Pen retains movements at least 1.5 screen pixels apart, including
coalesced events, and previews the shared midpoint-smoothed centerline. Click-only
strokes remain dots; release does not add an extra sample. Switching tools also
cancels the unfinished stroke. The chosen tool remains active. Resize/curve grips and
other drawing tools remain unconnected.
Layers → Annotation style now edits closed-shape fill/stroke toggles, stroke/fill
colors, stroke width and drop-shadow color, opacity, blur and offsets. Color pickers
and hex fields edit local values; Apply style sends only changed fields as one
undoable patch. Reset fields or changing the selected layer drops unapplied values.
Shared Rust supplies shadow defaults/clamps; toggling shadow off preserves its
stored custom settings. Hidden and locked annotations remain editable. Open shapes
and paths omit fill/stroke toggles, while images and text have no annotation controls.
Applying styles invalidates encoded previews without writing files or drafts.
Text placement opens an on-canvas multiline composing field; clicking existing
visible, unlocked text with the Text tool or double-clicking it with Select edits
that layer. Single clicks still select, and drags still move/resize. Typing previews shared
Rust pixels without saving a draft or adding undo steps. Done, Escape, clicking
outside, or closing finishes the latest text as one edit; Cancel restores the
previous document and output. Blank new text creates nothing; blank existing text
deletes that layer. Normal quit finishes the latest buffer before saving its draft.
Failed renders retain input for retry or cancellation, and output actions are
blocked until composition ends. This first composing field uses the UI font and
is unrotated; the document's pinned-font styled pixels remain authoritative.
It is not Tauri's WYSIWYG input layout. Physical input, IME, accessibility,
Windows and Wayland presentation still require acceptance.
Import image opens a single-file PNG/JPEG/WebP/TIFF picker without blocking draft
saves or close. The worker bounds and decodes the file, honors EXIF orientation,
then imports below the selected visible image using shared placement/expansion.
Undo/redo and saved drafts own the imported pixels; the external file is never
modified and is not needed after draft saving. Cancel and failed imports leave
the document unchanged. Batch/drag-and-drop and other formats remain open.
RGB/grayscale ICC imports convert to sRGB with straight alpha preserved; untagged
images assume sRGB. Unsupported/malformed ICC profiles (including CMYK), PNG
gamma/chromaticity-only metadata and CICP return recoverable errors asking for an
sRGB conversion first. Import normalizes to 8-bit RGBA, not HDR/wide-gamut editing.
The Output panel offers PNG/JPEG/WebP, Preserve/Compress/Maximum quality, custom
PNG palette sizes and an optional hard byte limit. Preview output runs the real
shared encoder and decoder on the worker, reports the exact byte count, and
switches between edited and encoded pixels. Changing options or editing clears
stale output; encoding errors retain the draft and allow retry. Preview does not
write files or change undo/redo. Save new copy and lossless edited-image Copy pixels
are connected. Linux enables arboard's
native Wayland data-control backend before its X11 fallback. Compositors without
`ext-data-control-v1` or `wlr-data-control` return a recoverable clipboard error.
This does not enable Wayland capture or close editor/input parity.
Normal quit drains edits and saves dirty sessions; a save failure cancels quit and
keeps the editor recoverable. Drafts live in `editor-drafts` beside the selected
History root, never the installed Tauri data. Other annotation tools remain unconnected.
The same Windows/X11/Wayland host code is present;
only private-X11/software-GL presentation has been exercised.

Run `python apps/native/x11_editor_smoke.py --binary
apps/native/wgpu/target/release/captures-wgpu-workbench --output editor-smoke
--appearance light` (also run dark). It checks asymmetric crop/resize preview
pixels, undo/redo, saved draft geometry and reopening, unsaved close, discard,
filesystem save failure and cancelled quit, retry and clean exit. Layer checks
exercise actual moved/half-opacity/hidden pixels, flags, order, deletion, empty
document undo and persisted fields after reopening. Screenshots
include both appearances and minimum-size scroll/error states. This is not
physical-desktop, accessibility, IME, or full editor parity acceptance.

The candidate tests whether shared custom components are viable. It is not a
retained widget renderer: egui rebuilds the visible UI on an event-driven repaint,
while image textures remain resident until a scene closes. Static scenes request
no recurring repaint. Compare this approach with DirectComposition/Direct2D on
Windows and GTK4 custom snapshots on Linux before selecting a renderer.

## Build

Requires Node 24, Rust **1.95.0**, and native build tools (MSVC/Windows SDK on
Windows; a C compiler, pkg-config, Wayland/X11/xkbcommon development libraries on
Linux). Linux tray builds additionally require the D-Bus development package
(`libdbus-1-dev` on Ubuntu). A working Vulkan or other wgpu-supported graphics
driver is required.
This development host does not bundle the Tauri app's media sidecars. Recording is
enabled only after `ffmpeg` and `ffprobe` are both verified from `PATH` on its worker;
install compatible command-line builds before launching. Missing tools are reported
in New Capture before recording can start. This is a development dependency, not
distribution or packaging parity.
The isolated Cargo workspace/lockfile leaves the shipping Rust 1.94 workspace
unchanged. The egui/eframe stack is pinned to upstream
[`60d7caae`](https://github.com/emilk/egui/commit/60d7caaea38a795618e842925061ad2210028a2a),
after the 0.36.2 release. This adds `RequestPaintWhileHidden`, needed to create
selectors and the first mini preview while the root stays hidden. Child-window
transitions request one paint, not a recurring hidden repaint loop. Released
0.36.2 lacks that API; 0.34.3 also lacks the native idle-loop fix. Do not downgrade
solely to match the shipping toolchain.

From the repository root, on either Windows or Linux:

```sh
rustup toolchain install 1.95.0 --profile minimal --component rustfmt --component clippy
node apps/native/prepare.mjs --output apps/native/wgpu/resources
cargo +1.95.0 build --manifest-path apps/native/wgpu/Cargo.toml --locked --release
```

Run `apps/native/wgpu/target/release/captures-wgpu-workbench` (add `.exe` on
Windows). Resources are embedded; no separate resource directory is needed at
runtime. CI also uploads unsigned experimental binaries and viewport screenshots;
these are not Preview installers. Its Linux binary targets Ubuntu 24.04, not every
supported production distro.

```sh
apps/native/wgpu/target/release/captures-wgpu-workbench --scene preferences
apps/native/wgpu/target/release/captures-wgpu-workbench --live
apps/native/wgpu/target/release/captures-wgpu-workbench --scene history --history-count 1000
apps/native/wgpu/target/release/captures-wgpu-workbench --scene hud --floating --appearance light
apps/native/wgpu/target/release/captures-wgpu-workbench --scene preview --floating
apps/native/wgpu/target/release/captures-wgpu-workbench --scene editor
apps/native/wgpu/target/release/captures-wgpu-workbench --scene region --exercise
apps/native/wgpu/target/release/captures-wgpu-workbench --scene window --exercise
apps/native/wgpu/target/release/captures-wgpu-workbench --scene capture-controls --capture-controls-recording
```

Appearance: `--appearance system|light|dark`; palettes: `--theme cobalt` (or any
existing preset). In live mode, closing the root hides it only while a working tray
reopen route exists; explicit Quit drains accepted work. Without a tray backend the
root remains visible and closable with an explanatory error. Floating fixtures have
Close and Move window controls. Launch one instance at a time for measurements.

On Linux, live tray residency uses StatusNotifierItem (SNI), not an XEmbed fallback.
It requires a session D-Bus and a registered SNI host, such as Xfce Panel's built-in
systray. `trayer` or `tint2` alone is insufficient without an SNI bridge. If no host
is available, or the watcher/last host disappears, the root is restored and close
quits so the process cannot be stranded. Opening the output folder also requires
`xdg-open` (provided by `xdg-utils`).

## Implemented probes and deliberate gaps

| Scene | Exercise | Not implemented / not accepted |
| --- | --- | --- |
| Preferences | Persisted appearance/presets/custom colors, capture/media defaults, folder picker, Find, save errors/retry | OS integrations, full font/visual/input parity |
| History | Empty/100/1,000 rows, filters, virtualized scrolling, selection, image-backed rows | Real files, open/delete, thumbnail cache pressure: rows intentionally share one synthetic texture |
| HUD | Running/paused/muted/busy/no-microphone fixture matching the bounded live controls; fixed glass palette even in light mode; live Hide/temporary notice/restore | Screenshot during recording |
| Preview | Cold/reused texture, fade/settle, reset mid-animation, explicit Reduce motion, optional transparent native window | **Not the shipping dust effect**: no isolated-chip blur, dust trajectories, source treatment or pile/drag/hit-region parity |
| Editor | 2048×1152 synthetic image, clipped canvas, pan/zoom/rotate, separate outline/text layers, editable text field | Real document, layer editing/undo/export; outlines/text do not rotate with the image |
| Capture Controls | Unified Screenshot/Record and Region/Window/Full screen controls over one prepared session; recording options, frozen/live previews, aspect/display pickers, keyboard confirm/cancel and draggable toolbar | Fixture uses synthetic pixels; real recording editing/export is a separate History action |
| Region | Deterministic blank/draw/move/corner-resize/aspect/Shift/cancel selector fixture using the live component | Fixture uses synthetic pixels and does not request screen permission |
| Window | Deterministic blank/frontmost-overlap/window/shell/display/cancel fixture using the live component and shared hit testing | Fixture uses synthetic pixels and does not request screen permission |
| Idle | Hidden native window; no scheduled application work except optional quit deadline | Process/GPU teardown after last window; production tray lifecycle |

The current screens are token-styled fixtures, not pixel-parity reproductions.
Default bundled fonts differ from the shipping system-font stack. AccessKit is
enabled, but screen-reader navigation and IME need real platform testing; painted
images/canvas layers lack full semantic nodes. Reduce motion is an explicit probe
switch, not yet connected to each OS setting. Transparency does not imply desktop
blur, click-through, topmost behavior, or correct Wayland overlay placement.
Live previews support stacking, collapse and overflow; drag placement, hover fan
motion and dust remain open. winit exposes full monitor
bounds but not the OS work area, so X11 intersects EWMH `_NET_WORKAREA` with the
target monitor and Windows uses the shared audited `rcWork` query. If usable bounds
cannot be resolved, capture still succeeds but no preview is shown. Multi-monitor
panel behavior still needs desktop verification. Preview positioning is unsupported
on Wayland. Windows preview runtime, nonactivation, and accessibility also remain
unverified beyond compilation and focused host tests.

**Hidden idle is unsupported on this candidate's Wayland backend.** winit cannot
hide/query the root there; live capture is disabled, and `--scene idle` exits with an explicit unsupported event
and status 3 rather than measuring a visible window. On X11/Windows the workbench
re-hides the root after eframe's automatic first paint and verifies visibility at
the quit deadline. A transient startup map remains possible. Resolving this is a
renderer gate.

**Transparent Vulkan windows failed under the orb's Xvfb/Mesa llvmpipe setup.**
The countdown's GPU readback was correct, but the compositor displayed no content.
`WGPU_BACKEND=gl` rendered the live overlay correctly with picom; this is a test
workaround, not a production backend decision. Verify transparent windows on real
Linux and Windows GPUs. Countdown entrance/exit motion and complete visual parity
remain open; cancellation and timing are shared Rust behavior.

## Validate and collect evidence

```sh
cargo +1.95.0 fmt --manifest-path apps/native/wgpu/Cargo.toml --all -- --check
cargo +1.95.0 test --manifest-path apps/native/wgpu/Cargo.toml --locked
cargo +1.95.0 clippy --manifest-path apps/native/wgpu/Cargo.toml --locked --all-targets -- -D warnings
python -m unittest discover -s apps/native -p 'test_*.py'
python apps/native/wgpu/smoke.py --binary apps/native/wgpu/target/release/captures-wgpu-workbench --output native-smoke
python apps/native/profile.py --renderer wgpu --binary apps/native/wgpu/target/release/captures-wgpu-workbench --output native-resources
/usr/bin/python3 apps/native/x11_recording_smoke.py --hide-controls-only \
  --binary apps/native/wgpu/target/release/captures-wgpu-workbench \
  --output /tmp/native-x11-hide-controls
sudo apt-get install xfce4-panel xdg-utils
/usr/bin/python3 apps/native/x11_preview_smoke.py --lifecycle \
  --binary apps/native/wgpu/target/release/captures-wgpu-workbench \
  --output /tmp/native-x11-lifecycle
```

Add `.exe` to both binary paths on Windows. Output directories must not exist.
Smoke tests need an interactive desktop or a test compositor. They check two idle
cases, twenty-nine framebuffer captures (including countdown, region/window selector
states, and empty/populated native file history), and forty-two scheduled actions. Inspect the
PNGs: their presence alone is not visual acceptance. The Wayland run explicitly
reports hidden idle as unsupported, not passed; the full resource runner fails
closed on that unsupported workload. Capture another state with
`--screenshot capture.png --screenshot-after 3 --exercise`; this reads only the
workbench's framebuffer, never your desktop. Screenshots stop the app after saving
and must be collected separately from resource trials.

The Linux CI job also runs a **real capture/persistence integration test** on a
private Xvfb desktop with Openbox, picom and software GL. It injects X11 pointer
and keyboard events, draws a region, and checks every saved pixel against an
asymmetric background pattern. Zero-delay capture must retain frozen pixels;
a nonzero countdown must capture the changed desktop. Repeated captures, Escape
while another application owns focus, simulated lock/unlock cancellation, region
metadata and clean shutdown are checked in the same process.

The Linux job also runs `x11_history_smoke.py` against disposable on-disk history.
It checks Clear history confirmation, Escape/Cancel, preserved exports, empty
history, and a real permission-denied partial failure followed by retry. Light
and dark captures are emitted for inspection. Run it as an unprivileged user:

```sh
python apps/native/x11_history_smoke.py --binary apps/native/wgpu/target/release/captures-wgpu-workbench --output native-x11-history
```

```sh
sudo apt-get install xvfb dbus python3-dbus python3-gi openbox picom hsetroot xdotool x11-utils x11-apps imagemagick libgl1-mesa-dri
/usr/bin/python3 apps/native/x11_capture_smoke.py \
  --binary apps/native/wgpu/target/release/captures-wgpu-workbench \
  --output /tmp/native-x11-capture
/usr/bin/python3 apps/native/x11_capture_smoke.py --controls \
  --binary apps/native/wgpu/target/release/captures-wgpu-workbench \
  --output /tmp/native-x11-controls
```

The `--controls` run uses New Capture instead of the direct selectors. It checks
the same exact saved pixels, target switching/retention, blank-toolbar drag and
empty-region confirmation guard. Both runs are part of Linux CI.

Use system Python for the distro's D-Bus/GLib bindings. The test owns its display
and D-Bus daemon; it never uses the caller's desktop/session or installed Captures
data. A private `org.freedesktop.ScreenSaver` fixture reports unlocked/locked
state through the normal session adapter. **Session state is simulated; X11
input delivery, the capture engine and PNG/history persistence are real.** There
is no application bypass flag. This does not verify an actual login manager,
hardware keyboard/GPU, Wayland, accessibility or real-desktop compositor behavior.
CI retains the disposable captures, metadata, screenshots and process logs.

The profiler takes about 44 minutes by default (eleven workloads, one excluded
warmup and three 60-second trials). It records process CPU-time deltas plus Linux
RSS or Windows working set, raw samples, initialization and action events. Neither
memory counter is physical footprint, GPU memory, or process-tree accounting.
Ready means renderer initialization, not first presented pixels. UI construction
timings are wall time, not input latency or GPU presentation timing. Use native
profilers for those acceptance gates. The wgpu preview probe and AppKit dust are
different workloads: **do not compare their effect timings as equivalent work**.

The `ready` event identifies the adapter/backend and whether wgpu reports a CPU
device. Xvfb/headless Weston plus Mesa llvmpipe can verify Linux rendering and
window lifecycle but cannot establish real-GPU performance, desktop-compositor
behavior, energy use, multi-monitor DPI, or screen-reader/IME acceptance. Test X11
and Wayland separately. CI Windows is also not a substitute for maintainer desktop
testing. No platform parity row is closed by this workbench.

Run `python apps/native/wayland_clipboard_smoke.py --binary
apps/native/wgpu/target/release/wayland_clipboard_probe` to test clipboard
transport independently of host UI. It starts disposable headless Sway with X11
disabled, publishes a 3×2 asymmetric RGBA image, independently decodes two
`image/png` pastes, and verifies the selection owner remains alive. This proves
the supported wlroots data-control path, not physical compositor, editor input,
accessibility, capture, or clipboard-manager acceptance.
