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
Capture History (1020 × 720) and Preferences (880 × 660) are separate, resizable
windows like the shipping app's, and first-run setup uses the History window's
root retitled as the 620 × 560 **Captures** setup window until setup completes;
`--live --open-preferences` also opens Preferences at launch.
Live mode also provides a native tray menu with the shipping labels and order:
New Capture…, Screenshot Region/Window/Display, Record Region/Window/Display,
Capture History…, Open Save Location, Preferences, Send Feedback…, a disabled
Check for Updates… and Quit Captures. Its persisted
display, region, window, recording and New Capture shortcuts work globally except while
capture is unavailable or a focused Preferences shortcut recorder is active. All seven
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
Close streaks a card out and Delete dissolves it into dust while survivors settle
into its slot; collapse and expand fly the cards, following shipping motion.
It is excluded from captures by default and retained when the include-in-captures
preference is enabled.
Cards follow the shipping hover chrome: an "In editor" pill and accent ring while
their screenshot editor is open, a pre-blurred darkened hover image, hover held
off after an expand or a new card until the pointer moves, and instant glass
icon tooltips.

Mini-preview **Share capture** and History's **Share selected capture…** open
native email-code sign-in and upload/link settings. The shared Rust worker owns
the isolated OS-vault session and exact original bytes, independently of popup
and preview dismissal. Upload is explicit; progress/cancel/retry, password/expiry,
Copy/Open link, Stop sharing and confirmed cloud Trash/Restore are connected.
Configuration retries never repeat a completed upload. These development controls
depend on the still-disabled #613 API; they do not activate or deploy it. AppKit
controls are connected through the same worker. Its seven sharing regressions
passed in macOS CI and light/dark minimum-size renders were inspected; physical
vault/object-store/compositor acceptance remains open.

`--scene sharing` renders without vault/network operations. Set
`CAPTURES_NATIVE_SHARE_FIXTURE` to `otp`, `vault`, `shared`, `uploading`, `trash`
or `error` for non-default states (unset means signed out). Fixture actions are
disabled. This variable never changes the production API origin or vault.

Record creates H.264 MP4 recordings with the stored FPS, maximum resolution,
countdown, cursor, click-highlight, desktop-audio and microphone defaults where
the current platform reports support. Pause/resume, confirmed Restart using the
stored countdown, Stop and Discard run from a
fixed-glass native HUD. Hide removes that HUD while preserving the current take and shows
a temporary noninteractive fixed-glass notice. The tray, app reactivation, or configured
New Capture shortcut restores it; Hide is disabled if no tray restore path exists, and Linux
tray-host loss restores the HUD and workspace. Microphone mute/unmute rotates the active segment without
changing the selected device or global preference, while paused changes remain
paused; mic-less sessions explain why the control is unavailable. A live microphone
meter uses the existing capture stream, with bounded worker sampling while visible
and unmuted; pause, mute and lifecycle changes clear the meter. Successful output is listed in native History with its
poster and metadata. **Edit recording** opens a decoded-frame editor with staged
graphical/numeric trim and crop, output size, audio settings and MP4/GIF
save-new-copy. Trim grips share the shipping pointer geometry and support focused
arrow/Page Up/Page Down keys. Paused handle press, thresholded dragging and keyboard
nudges preview the edge live with one active decode and only the latest queued
edit/position. Failure retains the accepted still and staged values, ends the
gesture and requires fresh input to retry. Other edits and Save wait for decoding;
background estimate/comparison waits for release. Playing handles stop the old
decoder, accept the latest trim, then resume without ending a held gesture or
losing keyboard focus. The end grip pauses with Loop off and wraps to the selected
start with Loop on. Sound/Loop selections survive; Pause remains usable during a
pending resume. Focus loss, Close/Quit and failures retire resume intent.
**Adjust crop**
loads an independent full-source still at the accepted position; eight handles and
interior move stage source-pixel coordinates with the current aspect lock. Arrow
keys move one pixel (Shift: ten). **Done cropping** restores the previous display
without publishing. Source loading is cancellable/retryable and cached until the
accepted position changes. Playback waits until adjustment ends. **Fit / 100%**
controls display scale without decoding or changing edits. 100% uses one decoded
image pixel per logical screen point, with bounded two-axis scrolling; it does not
upgrade the resolution of motion frames. New items start in Fit. Crop handles map
through the scrolled source image, and scrolling or switching scale ends a gesture.
The trim track
retains full-source thumbnails across edits and seek. Generation has independent
cancel/retry and does not change accepted preview or History state. **Play/Pause**
provides silent playback of the accepted trim using one persistent shared decoder,
at up to 30 fps and 1280 × 720. A single latest-frame slot prevents queued stale
frames. Pause retains the last displayed frame; EOF makes the next Play restart
the trim. Focus loss/minimize pauses, and close waits for teardown before checking
unsaved edits. Numeric seek/other edits/save/estimate wait for playback to stop;
motion frames never change accepted edits or History. **Loop preview** defaults off and can be changed
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
Preferences → About → Send feedback and tray Send Feedback… open the shipping
feedback window (its own 640×700 viewport, not a Preferences pane) with copy,
category cards, placeholders and limits from `captures_app::feedback`. It uses the
shared Rust client on a separate worker. Only explicit Send in `--live` can
contact captur.es; fixture mode keeps submission disabled. The form previews the
included app/system context below its header before editable fields, retains drafts on errors and when the window is
closed and reopened, and prevents duplicate sends while pending.
Captures, files, and diagnostics are never attached. General offers opt-in
development login startup on Windows/Linux, including Wayland hidden residency.
Updates retains native build identity and a disabled check action; signed native
updates remain unavailable.

Feedback input/retry verification uses a rejecting loopback proxy, never the
production service: `python apps/native/x11_feedback_smoke.py --binary
apps/native/wgpu/target/release/captures-wgpu-workbench --output feedback-smoke`.
It runs on private X11/software GL, follows the form's
`CAPTURES_NATIVE_LAYOUT_PROBE` control rectangles in the feedback window, and
checks empty/pending submission gates, retained text through window close/reopen
and failure, offline retry, and clean exit in both appearances. Shared client tests cover HTTP success, cooldown and payload
privacy against disposable loopback servers. Physical input/AT acceptance remains open.

A screenshot History card's Edit opens a worker-owned crop/canvas/draft editor. Like
shipping, the draft autosaves in the background 700 ms after each change and closing
flushes it without a prompt; the original capture and exports stay unchanged, and a
restored draft's notice offers Discard. The shipping header also holds the canvas W × H fields, Trim edges,
the canvas background, the zoom group and Add images; the rail chooses the inspector.
The Crop tool selects on the preview, including reverse and outside-image
drags. Free, 1:1, 4:3, 3:2 and 16:9 presets use shared Rust geometry; hold Shift
to lock the current ratio. Apply crop commits; Cancel, Escape or switching tools
abandons the selection without editing or saving. Numeric crop fields remain usable.
The sidebar always shows Layers above the tool's Properties. Rows list layers front
to back with a grip, a live thumbnail, the name and kind, and eye, lock and ⋯
actions; drag a row to reorder it and double-click an image row to rename it. The ⋯
popover holds blend mode, opacity, image transforms, Bring to front/Send to back,
Merge down/visible, Flatten image, Duplicate and Delete. Selected images show live
Width/Height/X/Y fields and selected text edits live; a burst of changes in one
field is one undo step. Shared Rust preserves locked boundaries and makes every
accepted edit undoable.
Draw adds filled rectangles/ellipses, straight lines, tapered arrows and freehand Pen strokes with the
shipping default annotation color and rounded rectangle corners. Drag previews are
transient; release creates one selected layer and undo step. Escape, focus loss,
close or switching tools cancels
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
colors, stroke width and drop-shadow color, opacity, blur and offsets. Stroke, fill
and shadow colors use the shipping swatch row with a custom color. Changes apply live
with only the changed fields; a burst in one field is one undo step and each toggle
is its own. An Opacity slider sits below the stroke width, as in shipping.
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
Add images opens a multi-file PNG/JPEG/WebP/TIFF/GIF/BMP/SVG picker without blocking draft
saves or close. The worker bounds and decodes each file, honors EXIF orientation,
then imports below the selected visible image using shared placement/expansion.
Undo/redo and saved drafts own the imported pixels; the external file is never
modified and is not needed after draft saving. Cancel and failed imports leave
the document unchanged. Canvas drops use the same queue. GIF imports freeze frame
zero on its logical canvas, not the animation; Open still uses the recording editor.
BMP supports row orientation, declared alpha and embedded RGB ICC profiles.
Calibrated or linked BMP color descriptions require an sRGB conversion first;
linked profiles never trigger filesystem/network reads. Legacy 32-bit BI_RGB is opaque.
Supported ICC and PNG gamma/chromaticity/CICP metadata convert to sRGB with straight
alpha preserved; untagged images assume sRGB. Unsupported/malformed profiles return
recoverable errors. Import normalizes to 8-bit RGBA, not HDR/wide-gamut editing.
SVG imports rasterize self-contained vectors/text using only bundled editor fonts;
unavailable faces may substitute. Both hosts use the shared resvg decoder, not a
browser or OS SVG renderer. Use absolute width and height, or one absolute dimension
with a valid `viewBox` aspect ratio to infer the other. Percentage dimensions,
`viewBox`-only sizes and missing dimensions with `preserveAspectRatio="none"` still
require conversion: their browser defaults differ from resvg's.
Input is limited to 4 MiB, 32,768 XML nodes, 32 element
levels and 4,194,304 output pixels, with the editor's 16,384-pixel side limit.
Images, HTML/foreignObject, reusable use references, DTD entities and SVGZ are not
accepted. Resource-bearing documents fail before data-URI decoding or filesystem
access; external image resolvers are disabled and system fonts are never loaded.
Convert unsupported SVGs to PNG first. AVIF, HEIC and broader SVG layer parity remain open.
The bottom export bar's **Export settings** disclosure offers output size,
Preserve/Compress/Maximum quality, custom PNG palette sizes and an optional hard
byte limit; the format is the filename's suffix menu. **Est. size** re-encodes on a
background thread after a 220 ms debounce (never on the session worker) and shows
the % change from the original. **Encoded** runs the real shared encoder and decoder
on the worker and shows the encoded pixels; changing options or editing clears
stale output, and encoding errors retain the draft and allow retry. **Save** follows
the shipping model through `captures_app::editor_export`: overwrite a saved
original by default (History entry and path re-checked), or save a new file when
**Save as new file** is on, the name/folder changed or the format differs. The
saved file becomes the next overwrite target, and **Show in Folder** opens its
folder. Lossless edited-image **Copy image** pixels are connected. Linux enables arboard's
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
pixels, undo/redo, autosaved draft geometry and reopening, close flush, discard,
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
Recording verifies FFmpeg/FFprobe on its worker before starting. Recording, recovery
and the editor share AppKit's lookup order: executable `CAPTURES_FFMPEG` /
`CAPTURES_FFPROBE` overrides, target-suffixed tools beside the executable or in its
`binaries/`, prepared checkout sidecars, then commands on `PATH`. Invalid overrides
fall back. [Development package staging](../../../DEVELOPMENT.md#native-development-open-with)
can copy the pinned pair with corresponding source/licenses using `--media-target`.
Missing tools remain visible in New Capture. This does not establish signed
distribution, physical-platform playback or packaging acceptance.
The isolated Cargo workspace/lockfile leaves the shipping Rust 1.94 workspace
unchanged. The egui/eframe stack is pinned to upstream
[`60d7caae`](https://github.com/emilk/egui/commit/60d7caaea38a795618e842925061ad2210028a2a),
after the 0.36.2 release. This adds `RequestPaintWhileHidden`, needed to create
selectors and the first mini preview while the root stays hidden. Child-window
transitions request one paint, not a recurring hidden repaint loop. Released
0.36.2 lacks that API; 0.34.3 also lacks the native idle-loop fix. Do not downgrade
solely to match the shipping toolchain.

At this pin with winit 0.30.13, Windows can keep repainting an animated editor
without delivering an already-requested visible root redraw. The host mirrors
accepted ROOT repaint deadlines and dispatches one due pass at an event boundary;
it does not poll or advance future deadlines. Hidden/minimized roots retain
eframe's normal throttled path, and Linux is unchanged. A later native redraw may
produce one redundant frame. Re-evaluate this adapter when changing the renderer
pin. `CAPTURES_NATIVE_TRACE=1` enables opt-in event/pass/scheduler diagnostics
without recording media paths or input contents. The forwarding smoke supports
`--trace`; CI also exercises staged Windows forwarding with diagnostics off.

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
apps/native/wgpu/target/release/captures-wgpu-workbench --live --open-preferences
apps/native/wgpu/target/release/captures-wgpu-workbench --scene history --history-count 1000
apps/native/wgpu/target/release/captures-wgpu-workbench --scene hud --floating --appearance light
apps/native/wgpu/target/release/captures-wgpu-workbench --scene preview --floating
apps/native/wgpu/target/release/captures-wgpu-workbench --scene editor
apps/native/wgpu/target/release/captures-wgpu-workbench --scene region --exercise
apps/native/wgpu/target/release/captures-wgpu-workbench --scene window --exercise
apps/native/wgpu/target/release/captures-wgpu-workbench --scene capture-controls --capture-controls-recording
apps/native/wgpu/target/release/captures-wgpu-workbench --scene update --update-state available --update-tray top
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
`xdg-open` (provided by `xdg-utils`); Show in Folder first asks a session-bus
`org.freedesktop.FileManager1` implementer to select the file.

## Implemented probes and deliberate gaps

| Scene | Exercise | Not implemented / not accepted |
| --- | --- | --- |
| Preferences | Persisted appearance/presets/custom colors, capture/media defaults, folder picker, Find, save errors/retry | OS integrations, full font/visual/input parity |
| History | Empty/100/1,000 rows, filters, virtualized scrolling, selection, image-backed rows | Real files, open/delete, thumbnail cache pressure: rows intentionally share one synthetic texture |
| HUD | Running/paused/muted/busy/no-microphone/saving/failed fixture matching the bounded live controls; fixed glass palette even in light mode; live Hide/temporary notice/restore | Screenshot during recording |
| Preview | Cold/reused texture, fade/settle, reset mid-animation, explicit Reduce motion, optional transparent native window | **Not the shipping dust effect**: no isolated-chip blur, dust trajectories, source treatment or pile/drag/hit-region parity |
| Editor | 2048×1152 synthetic image, clipped canvas, pan/zoom/rotate, separate outline/text layers, editable text field | Real document, layer editing/undo/export; outlines/text do not rotate with the image |
| Capture Controls | Unified Screenshot/Record and Region/Window/Full screen controls over one prepared session; recording options, frozen/live previews, aspect/display pickers, keyboard confirm/cancel and draggable toolbar | Fixture uses synthetic pixels; real recording editing/export is a separate History action |
| Region | Deterministic blank/draw/move/corner-resize/aspect/Shift/cancel selector fixture using the live component | Fixture uses synthetic pixels and does not request screen permission |
| Window | Deterministic blank/frontmost-overlap/window/shell/display/cancel fixture using the live component and shared hit testing | Fixture uses synthetic pixels and does not request screen permission |
| Update notice | Shared `captures_app::update_notice` copy for available/stacked notes, Hide / What’s new, open-captures warning, downloading, restart countdown, error/Try again, checking and up to date; `--update-state` picks the starting status and `--update-tray top\|bottom\|none` the tray position used for shared placement. The notice opens in its own transparent window with a caret. Escape dismisses it unless busy. | Stub status source only: no updater, download, install or relaunch; links are reported, not opened. Real tray-icon placement is not connected |
| Idle | Hidden native window; no scheduled application work except optional quit deadline | Process/GPU teardown after last window; production tray lifecycle |

The current screens are token-styled fixtures, not pixel-parity reproductions.
Default bundled fonts differ from the shipping system-font stack. AccessKit is
enabled, and the capture overlays and setup cards carry shipping/AppKit names, but
screen-reader navigation and IME need real platform testing; painted images/canvas
layers lack full semantic nodes. The X11 mini preview never takes keyboard focus
(override-redirect), so its controls are pointer-only there. Reduce motion is an explicit probe
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

**Wayland hidden startup is connected, not physically accepted.** The private
winit/eframe patches now support compositor-acknowledged unmapping and hidden
resident startup, exercised on disposable Sway. Physical resource acceptance is
still open; backend implementations without visibility acknowledgement report
unsupported rather than measuring a visible window. On X11/Windows the workbench
re-hides the root after eframe's automatic first paint and verifies visibility at
the quit deadline. A transient startup map remains possible. Resolving this is a
renderer gate.

### Native Wayland desktop shortcuts

An actual Wayland window uses `org.freedesktop.portal.GlobalShortcuts`; macOS,
Windows and X11 retain the direct manager. The seven stable action IDs are not
promises that the desktop grants every action or accepts the requested keys.
Preferences displays returned `trigger_description` strings and unbound rows,
without local recorders or writes to saved chords. Configure uses portal v2;
v1 keeps bindings read-only. Denial, session closure or owner loss needs explicit
Retry. Configure failure keeps the current grant. There is no OS-key takeover,
X11 fallback or automatic consent retry. Region/window screenshot and region
recording limitations still apply even if their IDs are granted.

The worker activates and pins the portal owner, subscribes before requesting,
validates owned request/session handles and rejects peer signals (including
directed signals). Bind runs once per session; a partial ShortcutsChanged clears
held/queued routes, then ListShortcuts replaces complete membership. Cancellation
is terminal and closes owned resources. Native Quit cannot launch a queued
capture after worker drain.

```sh
cargo +1.95.0 build --manifest-path apps/native/wgpu/Cargo.toml --locked --bins
cargo +1.95.0 build --manifest-path apps/native/wayland_drag_probe/Cargo.toml --locked
/usr/bin/python3 apps/native/wayland_shortcuts_smoke.py \
  --binary apps/native/wgpu/target/debug/wayland_shortcuts_probe
/usr/bin/python3 apps/native/wayland_shortcuts_host_smoke.py \
  --binary apps/native/wgpu/target/debug/captures-wgpu-workbench \
  --injector apps/native/wayland_drag_probe/target/debug/captures-wayland-drag-probe \
  --output /tmp/native-wayland-shortcuts-new
```

The protocol suite has 24 private-bus cases. The host suite owns Sway, Swaybar,
a private bus and disposable profiles; it checks 14 dark/light Pending, Bound,
Unavailable, v1 and configuration-error cases at 880×660 and minimum 560×440,
long descriptions, Configure/Retry, screenshot pixels/source retention, a
window-only failed recording and confirmed Delete, and pending/event-heavy Quit.
No DISPLAY is set. The layout probe records only readiness/enabled/error booleans
and control geometry, not desktop keys or installed paths. Inspect its PNGs.
These are scripted desktop grants, not physical key delivery or consent UX.

Same-role remapping requires the fresh initial-configure fix; Sway 1.7 lacks it
and is unsupported. For Debian 12's old compositor, build the pinned headless-only
fixture with `apps/native/build_wayland_compositor_fixture.sh`, then prepend its
printed launcher directory to PATH for these smokes. Build prerequisites are
Meson/Ninja, libffi/expat, libdrm, Pixman, xkbcommon, json-c, pcre2, Cairo/Pango,
libevdev, libinput headers and libsystemd development packages. It installs only
inside a cache, never as the user's compositor or portal. `wayland_visibility_smoke.py`
also checks immediate post-present hide, cancelled remaps and exact pixels.
The private patch serializes unmap on the event queue, filters stale configure
events before SCTK auto-ACK, and distinguishes render readiness from acknowledged
visibility. Decoration objects retain their mode rather than replaying set_mode
during remap. Physical GNOME/KDE, accessibility and mixed-DPI gates stay open.

### Wayland screenshot acquisition diagnostic

`captures_capture::portal_screenshot` acquires a still through the public
Screenshot portal without X11 monitor enumeration or direct compositor fallback.
It subscribes before requesting, verifies the response's handle and unique portal
owner, returns cancellation without capturing again, and sends Request.Close on
local cancellation or timeout. It reads only a local file URI and never deletes
or changes the portal-owned image. Run it on a worker, after unmapping any windows
that must be excluded. Screenshot and owner lookup use at most five seconds or
the remaining deadline; subscription uses five seconds, Close one second. Timeout
and cancellation checks cannot interrupt bus connection setup or image decoding.

The diagnostic creates no Captures window. It does not prove own-window exclusion
in the resident host. Portal consent, image extent and cursor inclusion are
backend-controlled; there is no named-display mapping or assumed desktop origin.
In xdg-desktop-portal 1.16, version-1 Screenshot backends bypass its permission
store check. The orb's wlr 0.7 backend is version 1 and returned success even with
the disposable permission set to “no”; this is not a client fallback or proof of
consent enforcement. The smoke reports the backend version/policy it exercised.
Native window targeting, selectors and preview placement
remain gated. Portal still capture, display/window recording and desktop shortcuts
are connected to the host as described in their sections.

```sh
cargo +1.95.0 build --manifest-path apps/native/wgpu/Cargo.toml --locked --bin wayland_screenshot_probe
/usr/bin/python3 apps/native/wayland_screenshot_smoke.py \
  --binary apps/native/wgpu/target/debug/wayland_screenshot_probe
```

The smoke needs Sway, swaybg, grim, PipeWire, xdg-desktop-portal and its GTK/wlr
backends, system Python dbus/gi, ImageMagick, sudo and util-linux. It creates a
private mount namespace, D-Bus, compositor, runtime and permission store; consent
is pre-granted only in disposable data. The private `/tmp` also contains older
wlr backends' fixed screenshot path. It checks early and alternate-handle
responses, wrong sender/path signals, cancellation, timeout, failed method replies,
URI rejection, exact RGBA and source retention. The real frontend → wlr backend →
headless Sway test compares every pixel of an asymmetric 310×170 desktop against
independent expectations. This is not physical GNOME/KDE or native-host acceptance.

### Wayland video acquisition diagnostic

`captures_recording_xcap::PortalVideoSource` requests one display or window through
the public ScreenCast portal and connects only to its granted PipeWire remote. CreateSession,
SelectSources and Start responses are matched to the pinned unique portal owner
and request handle. Denial/cancellation ends the request without xcap, unrestricted
PipeWire or compositor fallback. No restore token or persistent permission is
requested. Cursor inclusion requires the advertised hidden/embedded mode; click
highlights, keystrokes and region targeting are unavailable. Window requests use
only source bit 2, require portal v3+ advertising window sources, and reject absent
or non-window `source_type` before opening the remote. The portal chooses the
window; Captures does not enumerate window IDs or crop a display grant.

The worker accepts bounded, single-plane CPU-mapped RGBA/BGRA/RGBx/BGRx frames.
It validates chunk offsets, sizes, positive strides and dimensions, rejects
DMA-BUF/corruption/format changes, and exposes a bounded one-frame queue, dropped
frame count and stream warning. Newer portals' `pipewire-serial` selects the node;
older portals use the granted node ID with reconnection disabled. Stopping or
dropping joins the worker, releases the remote and closes the portal session.
Consent has a 120-second deadline; calls take at most five seconds, polling
50 ms and cleanup calls one second. Cancellation cannot interrupt bus connection
setup or a blocking call. Compositor-space stream properties are not treated as
pixel dimensions or invented monitor geometry.

```sh
cargo +1.95.0 build --manifest-path apps/native/wgpu/Cargo.toml --locked \
  --bin wayland_video_probe --bin wayland_recording_probe
backend="$(apps/native/build_wayland_portal_fixture.sh)"
/usr/bin/python3 apps/native/wayland_video_smoke.py \
  --binary apps/native/wgpu/target/debug/wayland_video_probe \
  --recording-binary apps/native/wgpu/target/debug/wayland_recording_probe --wlr-backend "$backend"
```

The private fixture needs the screenshot-smoke dependencies plus Meson, Ninja,
libinih/libdrm/libsystemd development headers, wayland-protocols, WirePlumber and
system Python GTK/Cairo (`python3-gi-cairo`, `gir1.2-gtk-3.0`). Its pinned wlr
0.7.1 source receives a **fixture-only SHM format-guard backport**: stock 0.7.x
demands DMA-BUF even with a valid SHM format, while 0.8.x requires DMA-BUF during
initialization (upstream issue #289). The later upstream constraints rewrite also
renegotiates every frame before its follow-up fix. The helper neither installs
the backend/service nor changes selection, consent or remote grants. This is not
stock-backend or physical consent acceptance.

The smoke uses a private mount namespace, bus, Sway and PipeWire/WirePlumber with
DISPLAY unset. Display and window protocol cases cover peer spoofs, early responses,
denial/cancellation, lost replies, legacy portals/cursors, unsupported source/cursor modes,
wrong or absent window source types, invalid streams and session cleanup. The
scripted window grant reaches the remote-open call, but intentionally returns an
error there: it is protocol coverage, not a real captured window. A separate animated Wayland client supplies
two asymmetric frames. The real portal/PipeWire path must see both phases and
match every output pixel across repeated sessions, with cancellation during an
active stream leaving no output. Terminating the backend while the granted stream
is running must end the source promptly, without output or a frame timeout; this
checks transport loss, not physical permission revocation. A still desktop can legitimately stop delivering
new frames until damage occurs. The no-window probe writes a diagnostic PNG only
after collection.

The separate `wayland_recording_probe` uses the shared native session, H264/MP4
encoder, media assembly and private History/recovery stores. It requires a **new**
output directory and never imports/overwrites profile data. `--scenario` accepts
`video`, `gif`, `pause`, `restart`, `discard`, `recover` or `stream-loss`; the last
requires externally ending the granted stream. `--duration-ms` defaults to 450;
`--cancel-after-ms` cancels consent/start or discards an active session. Both probes
accept `--target display|window` (default display) and reject DISPLAY. Portal sessions
serialize `{"type":"portal_display"}` or `{"type":"portal_window"}` without
monitor/window IDs or origins; dimensions come only from validated pixel frames. Cursor
pixels are portal-owned, and unsupported click/keystroke overlays are rejected.

Passing `--recording-binary` repeats both targets' protocol cases through the session,
then checks MP4/GIF publication, asymmetric pause/resume durations (excluding an
800 ms pause), restart replacement, paused-draft recovery, discard/cancellation,
and refusal to overwrite existing outputs. FFmpeg independently decodes both
expected color phases at four asymmetric interior locations, within a 15-channel
lossy-codec tolerance. Video source bundles are removed only after publication;
GIF sources remain editable. Backend termination must fail the session promptly,
then recovery must publish its decodable partial MP4 without reopening capture.
Explicit local disconnect unregisters callbacks before cleanup and is not failure.
Automatic controls exclusion, real Wayland audio/cursor, stock-backend consent,
mixed-DPI/rotation and physical GNOME/KDE acceptance remain open; the resident
controls follow-up below connects MP4 recording in the development host.

### Native Wayland recording controls

In `--live` Wayland History, **Record display…** and **Record window…** prepare an MP4 take from the
recording preferences, verify FFmpeg/ffprobe on the worker, and show a compact
native countdown without monitor IDs, origins or geometry. Before portal consent,
the host unmaps History, Preferences and the countdown and waits for compositor
acknowledgements. A new session uses only the portal-granted PipeWire remote.
The normal HUD connects pause/resume (a fresh grant), confirmed restart/delete,
Stop, and History publication. Focused Escape/closing cancels the countdown;
portal cancellation discards the empty initial bundle. A cancelled resume leaves
accepted media paused. Source loss stops the timer/controls, releases the session
owner, restores the workspace and exposes retained partial media in recovery.

Window recording requires portal v3+ with window-source support. A window grant
shares only the selected window; each resume/restart obtains fresh consent and
can select a different window. Unsupported backends fail visibly without display
fallback, retaining a failed empty take for Retry/Delete.
Linux display capture includes the recording controls in video. Hide is available only with a
working tray restoration path; the HUD states when it is unavailable. Screenshots
during recording, region targeting and click/keystroke overlays are not
supported on Wayland. Window placement after remapping is compositor-controlled.

```sh
/usr/bin/python3 apps/native/wayland_recording_host_smoke.py \
  --binary apps/native/wgpu/target/debug/captures-wgpu-workbench \
  --injector apps/native/wayland_drag_probe/target/debug/captures-wayland-drag-probe \
  --wlr-backend "$backend" --output /tmp/native-wayland-recording-new
```

Use a new disposable output directory and the video fixture's pinned backend.
The dark/light real-window smoke uses compositor geometry and real pointer input,
checks the window action at normal/minimum sizes and the actual wlr backend's
unsupported-window error, failed `portal_window` manifest, Retry/Delete and no media/fallback,
checks countdown/unmapping, exactly frozen paused time, pause-free MP4 duration,
and independently decoded pixels beneath the pre-capture windows. It closes the
countdown, sends a public-portal cancellation, terminates the active backend,
and checks stable playable recovery bytes after a clean timed app exit.
Duration uses independently observed running/paused/Stop wall-time bounds, excluding
grant latency from the lower bound. The held pause must exceed the entire bounds
gap plus rounding tolerance, so including it in output necessarily fails.
CI retains its logs/media/screenshots. This is software-rendered Sway with
non-interactive fixture consent, not physical GNOME/KDE, audio/cursor, mixed-DPI
or stock-portal permission UX acceptance. Successful GNOME/KDE window capture is
unverified. macOS/Windows/direct-X11 paths remain
on their existing engines; no Tauri cutover or native release follows.

### Native Wayland portal screenshots

In `--live` History, **Take screenshot…** works without a tray or X11. The host
unmaps every current Captures viewport and waits for compositor-processing
acknowledgements before requesting a portal still on its worker. Success persists
one shared History screenshot and optionally copies it; cancellation, failure,
session loss and normal Quit close pending requests without adding History media.
Windows restore afterward, but their compositor-assigned positions may change.
Portal policy controls consent, image extent and cursor inclusion. There is no
guessed named-monitor geometry and no mini preview; captures return to History.
History **Edit** opens the normal screenshot editor directly on Wayland, without
first placing a preview. **Restore** is disabled with a platform limitation tooltip.
Screenshot commands honor the configured delay in the shared fixed-glass,
compositor-placed countdown. Focused Escape/close cancels before consent. The host
keeps its hidden viewport alive until the native unmap acknowledgement, then
removes it after portal completion. Region/window screenshots fail explicitly;
display/window recording uses the controls path above.

The recording HUD's Screenshot action (or a granted display screenshot shortcut)
can request a separate desktop-portal still while the take runs or is paused.
It owns a child generation, not a second recording, and waits for all Captures
windows including the HUD to unmap. Completion restores the controls through a
hidden-root UI pass, retaining the take, elapsed clock and manual Hide state;
the workspace stays out of the video. Cancellation/failure publishes no still
and preserves the take. Pending child requests close on Quit before the accepted
recording is finalized. Nonzero countdown works for these children too, preserving
the paused clock and cancellation ownership. It is excluded from the still but
can appear in an ongoing display take, like the HUD. The portal controls
extent, cursor and consent, and the still goes to History without a floating preview.
The dark/light recording-host fixture checks exact still pixels while active
and paused, unchanged paused clocks, child cancellation/failure, and Quit with
a pending screenshot preserving one playable video. Its transparent private cursor
theme makes every pixel independent of cursor timing; physical cursor policy is unverified.

The private eframe patch keeps hidden-root logic running without presenting
buffers, refreshes child visibility, preserves compositor occlusion, and recreates
Wayland GPU surfaces after remapping. Normal Windows/X11 rendering is retained.
The capture wake poll runs only while the process capture gate is owned.

```sh
cargo +1.95.0 build --manifest-path apps/native/wgpu/Cargo.toml --locked --bin captures-wgpu-workbench
cargo build --manifest-path apps/native/wayland_drag_probe/Cargo.toml --locked
python3 apps/native/wayland_visibility_smoke.py
/usr/bin/python3 apps/native/wayland_native_capture_smoke.py \
  --binary apps/native/wgpu/target/debug/captures-wgpu-workbench \
  --injector apps/native/wayland_drag_probe/target/debug/captures-wayland-drag-probe
```

The host smoke uses the same disposable portal/Sway environment as the diagnostic.
It verifies dark/light real and repeated captures against every independently
expected desktop pixel with History, Preferences and screenshot editor excluded;
cancelling a nonzero countdown restores all three windows and permits a retry.
Normal Quit during the countdown requests no consent and publishes no still;
cancellation/failure and simulated session lock write nothing, normal Quit closes
the pending request, and second-instance media cannot remap excluded windows.
It opens a captured screenshot through History **Edit** in both appearances and
checks that floating-preview **Restore** is disabled.
After cancellation, failure-dialog dismissal or simulated lock, a successful
request in the same process must save exactly one independently expected image.
The fixture keeps one virtual pointer alive to avoid an old wlroots device-removal
crash and hides its idle cursor for the exact desktop-pixel comparisons.
The visibility probe also covers initially hidden windows, rapid hide/show and
hidden redraw suppression. These are software-rendered integration checks, not
physical GNOME/KDE, multi-display, mixed-DPI or accessibility acceptance.

### Native Wayland resident lifecycle

Quiet live launches keep History unmapped when an SNI tray exists. The startup
notice expires; tray Preferences/Feedback, empty relaunch and image/recording media
open only the requested child. Closing those children retains residency. Missing
or lost trays expose reachable History with normal close-to-quit. General's
development login toggle is explicit and OS-authoritative on Wayland too.
The notice points to History in the tray menu instead of advertising an
ungranted requested Wayland global shortcut.

```sh
/usr/bin/python3 apps/native/wayland_lifecycle_smoke.py \
  --binary apps/native/wgpu/target/debug/captures-wgpu-workbench \
  --injector apps/native/wayland_drag_probe/target/debug/captures-wayland-drag-probe
```

Use the builds/dependencies above and the installed Swaybar SNI tray. This smoke
owns its compositor, bus, XDG configuration and autostart files. It acknowledges
window-event subscription before launch and drains events at each check, rejecting
even transient History mapping. It exercises real tray menu actions, input-driven
login enable/disable, GIO entry relaunch, Unicode media and normal Quit. It neither
sends Feedback nor changes installed data or login registration. Optional
`--screenshots DIRECTORY` captures review states. Physical GNOME/KDE, sign-in,
tray-icon notice placement/focus and whole-app resource acceptance remain open.

**Transparent Vulkan windows failed under the orb's Xvfb/Mesa llvmpipe setup.**
The countdown's GPU readback was correct, but the compositor displayed no content.
`WGPU_BACKEND=gl` rendered the live overlay correctly with picom; this is a test
workaround, not a production backend decision. Verify transparent windows on real
Linux and Windows GPUs. The countdown fades in and out with the shipping
keyframes; complete visual parity remains open. Cancellation and timing are
shared Rust behavior.

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
It checks History Restore (the mini-preview window opens, a second Restore adds no
window and Delete all removes it), two-step Delete all confirmation, Escape/Cancel,
preserved exports, empty history, and a real permission-denied partial failure
followed by retry. Light
and dark captures are emitted for inspection. Run it as an unprivileged user:

```sh
python apps/native/x11_history_smoke.py --binary apps/native/wgpu/target/release/captures-wgpu-workbench --output native-x11-history
```

Live History also lists interrupted recording bundles separately from saved
artifacts. Recover publishes MP4/GIF through the shared worker, then selects and
opens the recovered recording unless selection changed. Discard permanently
requires confirmation; corrupt/unavailable bundles stay read-only. Cancellation
stops preparation, not committed publication. These are isolated native capture
bundles, not recording-editor drafts or installed Tauri data.

The real-media recovery exercise checks both appearances, discard confirmation,
encoder cancellation, History permission failure/retry, decoded red/blue output,
editor opening and preservation of unrelated History and corrupt bundles:

```sh
python apps/native/x11_recovery_smoke.py --binary apps/native/wgpu/target/release/captures-wgpu-workbench --output native-x11-recovery
```

The private PulseAudio microphone exercise varies live volume, checks empty
silence/paused/muted meters, resumes sampling after unmute, and independently
decodes the saved audio. It requires `pulseaudio` and `pulseaudio-utils`; this is
virtual-audio evidence, not physical microphone or Windows/Wayland acceptance.

```sh
/usr/bin/python3 apps/native/x11_recording_smoke.py --restart-only --virtual-microphone --appearance dark --binary apps/native/wgpu/target/release/captures-wgpu-workbench --output native-x11-microphone-dark
```

Segment reopen tests use two different virtual tones. `default` verifies that a
paused recording picks up the new default microphone on resume and saves both
tones. `explicit` removes the selected microphone endpoint before resume and
requires failure with completed audio preserved, never substitution of the other
microphone. These test reopen behavior, not in-stream hot switching or physical
unplug notifications.

```sh
for device in default explicit; do
  /usr/bin/python3 apps/native/x11_recording_smoke.py --device-change "$device" --binary apps/native/wgpu/target/release/captures-wgpu-workbench --output "native-x11-device-$device"
done
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

Outbound preview file dragging uses a private pinned winit patch on Linux and
`drag` 2.1.1 on Windows; see [vendor provenance and update steps](vendor/README.md).
Run `/usr/bin/python3 apps/native/x11_preview_smoke.py --drag-only --binary
apps/native/wgpu/target/release/captures-wgpu-workbench --output /tmp/native-drag`
from the repository root. Repeat with `--reduced-motion` and a new output folder.
The disposable X11 receiver checks original bytes, Unicode saved paths, self-drop,
cancellation, rejection, timeout, target loss, and subsequent Copy/drag input.
It records the self-drop animation for inspection.

`python3 apps/native/wayland_drag_smoke.py` builds an isolated protocol probe and
starts headless Sway with DISPLAY unset. It requires GTK3's system Python bindings
(`gir1.2-gtk-3.0`, `python3-gi`) and verifies exact URI/file bytes against an
independent GTK receiver, two transfers from the same source process, rejection,
cancellation, self-drop classification, disappearing targets and missing-Finished
timeout recovery. This is not full Wayland capture-host
or physical-compositor acceptance. Windows OLE transfers, mixed-DPI destination
classification and full physical-host interaction remain open validation gates.
