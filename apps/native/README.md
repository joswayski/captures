# Native desktop workbenches

These are implementation stages of the [native rewrite](../../docs/native-rewrite.md),
**not replacement downloads**. The existing Tauri Preview is unchanged.
macOS is Swift/AppKit + Core Animation with Core Image explicitly backed by Metal.
No WebView, React, JavaScript runtime, Rust sidecar or network service is used.
Fixture launches do not request screen access. The opt-in capture workspace below
connects the existing Rust capture engine in process. The
[shared wgpu candidate](wgpu/README.md) adds Windows/Linux fixture windows and
cross-platform resource diagnostics; it is not a production renderer selection.
Its fade/settle probe is not equivalent to AppKit dust. DirectComposition/GTK
comparators and full parity gates remain open. The instructions below cover
AppKit; the candidate README has Windows/Linux build and test commands.

## Persisted native Preferences

Both native hosts use the same `captures-settings` Rust types, migrations,
validation, atomic persistence, and custom-theme derivation as the shipping
adapter. AppKit calls a versioned in-process C ABI; Windows/Linux calls Rust
directly. Preferences stores appearance and capture/media defaults in a separate
**Captures Native** development identity. `--settings-file PATH` selects an
explicit test file. Malformed/newer files report an error instead of resetting
them. `--exercise` uses disposable data. No installed Preview settings are imported.

Without `--live`, capture, recording, history, and editor scenes use fixtures. Saving a
default is not an engine integration: fixture login items, feedback submission and
update actions remain visibly unavailable, with copy that says why. Both hosts draw
the shipping Preferences layout, controls and copy, shared through
`captures-app::preferences`; physical input, screen-reader and Windows/Wayland
acceptance and the other checklist gates remain open.

Preferences records all seven stored shortcut fields using the shared Rust
key/modifier, display, cancellation and validation policy. Escape (including
modified Escape), focus loss or leaving the recorder cancels without saving.
Modifier-only input previews the chord; invalid keys show an inline error.
Focused Preferences releases all seven capture OS registrations so a recorder
can receive an existing global chord. Edits remain unregistered until Preferences
loses focus; registration failures are reported rather than treated as success.
New Capture and all six screenshot/recording target actions are connected in live mode.
Fixture Preferences never registers global keys. The shipping TypeScript policy
supplies 390 recording and 195 platform-display differential test vectors.

On X11, `x11_preview_smoke.py --lifecycle --shortcut-editing` exercises all seven
storage paths with native input, registered-key delivery, invalid/cancel/blur,
duplicate rejection, restart persistence and a saved global launch chord. AppKit
XCTest renders both appearances and checks controller/bridge input. Synthetic
virtual-key mapping does not prove physical Mac media/external-keyboard input;
physical Windows/macOS, Wayland and screen-reader acceptance remain open.

## Shared recording runtime

`captures-recording-platform::RecordingSession` owns a durable recovery bundle
and the existing platform engine across start, pause/resume, restart, stop, discard and
MP4/GIF finalization into private History. Run its blocking methods on a worker.
Hosts still own permissions, countdown presentation, window exclusion and the
capture-generation cancellation gate passed to `start`; `prepare` never records.
Failed assembly/publication keeps source segments. Successful video publication
removes the draft only after Ready metadata is saved; GIFs keep editable sources.
Post-publication housekeeping failures return the saved artifact with a warning.
Restart discards only that session's active and completed segments, retains its
target/options, resets elapsed time, and returns to the stored countdown. The
same capture generation rearms global Escape for that countdown before either
host can open the replacement engine.
Both hosts connect Video-only Record controls and region/window/display recording
shortcuts. From idle the keys open Record on that target; in an open selector,
screenshot and recording keys switch mode/target in place. Busy recording phases
and focused Preferences suppress capture shortcuts. Hidden recording controls are the exception:
only New Capture is routed, and it restores the same generation without launching another capture.
The HUD Screenshot action opens the existing region selector under a temporary child
generation while the accepted recording keeps running or paused. Escape cancels only
the selector/countdown, and a completed still follows normal History, mini-preview
and auto-copy behavior. AppKit/Windows apply capture exclusion; X11 hides the HUD and
guide from the still but cannot exclude the selector from ongoing recording pixels.
Native recording editing,
GIF conversion and media-tool bundling remain unconnected. History **Save file**
copies original video/GIF bytes to the configured output folder without encoding
or overwriting another file. Repeated Save reuses the export; deleting it allows
another copy from private History. **Show in Folder** reveals the exported copy,
which survives deleting or clearing History. The poster remains the native preview.

Successful finalization also opens a nonactivating fixed-glass **Recording ready**
notice at the display work area's top right. It reuses History's Save file operation
and changes to **Recording saved** / Show in Folder after export. Save failure and
missing-file reveal errors remain retryable; Dismiss and the 15.2-second expiry
never delete media. Expiry pauses during saving and restarts on completion/error.
A new capture clears the notice, and stale callbacks cannot reopen a dismissed
or replaced notice. Because native recording editing is not connected, this appears
after finalization rather than the shipping editor-close trigger. Physical input,
compositor, multi-display and accessibility acceptance remain open.

Linux CI records an asymmetric 310×170 region on a private Xvfb display, pauses,
resumes and restarts from running/paused, independently decodes replacement pixels
with FFmpeg, publishes video
and GIF History entries, and exercises cancellation and persistence failures:

```sh
env -u WAYLAND_DISPLAY XDG_SESSION_TYPE=x11 CAPTURES_TEST_PRIVATE_X11=1 \
  xvfb-run -a -s '-screen 0 640x480x24 -nolisten tcp -noreset' \
  cargo test -p captures-recording-platform \
  private_x11_records_pause_resume_pixels_and_cancelled_start -- --ignored
```

This needs `xvfb`, `xauth`, `hsetroot`, FFmpeg and FFprobe. It is not physical
Windows/macOS, audio-device, multi-display or Wayland acceptance.

Failures follow the shipping HUD. If the engine cannot start a take (for example
the selected microphone is missing), the HUD stays up as **Failed** with the error
on one line below the controls, and offers **Retry recording** (no confirmation) and
Delete. If resume or a microphone change cannot reopen the engine, the take stays
paused with that error, ready to resume or save. Stop shows **Saving…** until the
take is published. A failed save still closes the HUD and keeps the recovery bundle.
Engine warnings share the error line, which clears when the next HUD action starts.
HUD buttons use the shipping fixed-glass tooltips, which appear immediately on hover
or focus. `x11_recording_smoke.py --start-failure` exercises these states on X11.

The native recording Hide slice removes the AppKit or wgpu HUD without changing
the accepted session, timer, pause/microphone state, capture generation or media.
A 6.2-second click-through fixed-glass notice explains restoration; no collapsed
replacement strip remains. Menu bar/tray actions, app reactivation and the configured
New Capture shortcut restore controls under the persisted capture-exclusion policy.
Linux enables Hide only while a real SNI host supplies a restoration path, and tray-host
loss restores the HUD and workspace. AppKit and Windows still require physical-host
compositor and accessibility acceptance; Wayland remains gated.

## Resident lifecycle and screenshot shortcuts

Only `--live` creates the macOS menu-bar item or Windows/Linux tray and registers
the persisted New Capture and region/window/display shortcuts. The menu uses the
shipping labels, order and separators: New Capture…, Screenshot
Region/Window/Display, Record Region/Window/Display, Capture History…, Open Save
Location, Preferences, Send Feedback…, a disabled Check for Updates… (signed
updates are not connected) and Quit Captures. Capture items show the saved
shortcuts as accelerators where the platform menu displays them (Linux SNI hosts
may not), and any tray capture action brings hidden recording controls back. Closing the root hides it when a usable tray is available; previews and
accepted work stay alive.
Quit cancels pending capture, drains accepted file work and removes shortcuts/tray.
Timed and framebuffer-screenshot completion also explicitly quit, not hide.
Captures launched from a hidden root leave it hidden on success or cancellation.
macOS Dock reopen shows an existing visible window or Preferences.

The shared Rust dispatcher queues release-triggered screenshot actions and
temporary Escape cancellation without competing process-wide handlers. Native
event loops drain actions on their UI thread. Active capture/preparation and
focused Preferences suppress screenshot shortcuts; hidden or unfocused
Preferences does not. Invalid/colliding shortcuts report errors. This does not
implement OS shortcut takeover or recording actions,
launch at login, or complete lifecycle parity.

Live hosts now elect one process per canonical History root. Subsequent launches
forward media paths or request native reactivation without creating UI or capture
workers. Fixtures remain independent. See [DEVELOPMENT.md](../../DEVELOPMENT.md#native-frontend-migration)
for limits, acknowledgement semantics and cross-platform process tests.
Optional [development Open With packages](../../DEVELOPMENT.md#native-development-open-with)
stage a separate AppKit `.app`, Windows per-user alternate registration files or a
Linux desktop entry. Nothing is installed or registered by staging; opt-in steps
and removal are documented separately. Both CLIs support `--live -- FILE...`.
The bundle defaults to live mode and collects cold Apple events before election.
These packages do not close physical lifecycle or installed-release acceptance.

Linux uses SNI/KSNI over session D-Bus, not XEmbed or GTK/AppIndicator. Building
needs pkg-config and libdbus-1-dev; runtime needs a registered StatusNotifier host
and `xdg-open` for folder fallbacks; Show in Folder first asks a
`org.freedesktop.FileManager1` implementer to select the saved file. No watcher/host means an explicit error and
normal close-to-quit. Losing the tray host restores the root instead of stranding
the process. XEmbed-only trays require an SNI bridge. Wayland capture/hidden-window
support remains gated; a tray does not remove that limitation. Physical macOS,
Windows, mixed-DPI and accessibility acceptance remain open.

## New Capture controls

New Capture starts a screenshot selector in Region mode. Its fixed-glass toolbar
switches between Region, Window and Full screen without replacing the prepared
desktop snapshot or discarding settled selections. An actual display change
prepares a replacement session and clears display-local selections while retaining
target mode and aspect ratio; stale replies cannot reopen a cancelled selector.
Capture is disabled until the selected target
is valid. Enter confirms, Escape cancels, and aspect/auto-start/freeze/countdown
preferences use the existing Rust geometry and capture policies. The root returns
to its prior visibility; a background capture does not reopen Preferences.

One Rust `WindowSession` also accepts a region target, reusing region crop/cursor
validation without another full-screen copy. A nonzero countdown refreshes pixels
for all three targets. Existing direct screenshot actions remain available.

Menu copy and small policies come from `captures-app::capture_menu`, so both hosts
share the shipping labels. The footer note reports whether "these controls" show
in screenshots or recordings from the recording capabilities; where the platform
can exclude them (macOS/Windows) it and the "Auto-capture is on" notice link to
Preferences, which scrolls to and briefly highlights that row. Linux shows the note
as plain "will show" text. Full screen shows the display name and size (plus FPS in
Record). Record mode shows labelled FPS / Max resolution selects, Show cursor /
Show clicks / Desktop audio switches (On/Off/Unavailable with a reason tooltip;
clicks imply the cursor) and the microphone select. The primary button hides under
auto-start unless a start failed. Guidance stays until a window is selected, hides
while dragging and fades when the pointer comes within 28 points. Segmented-control
animation/icons, the panel entrance animation and Wayland remain open; physical
displays, platform input and accessibility acceptance remain open.

Configured Region/Window/Full screen global shortcuts switch the open selector's
target without replacing its session. Like the shipping keyboard path, every
target key clears hover; Region/Full screen clear the selected window, while
Window retains it. Settled region/aspect remain. Keyboard Full screen does not
auto-start; pointer selection still follows that preference. New Capture cannot
re-enter an open selector. Shared generation checks reject queued/held keys when
selection ends or display preparation starts; countdown and capture stay blocked.

The toolbar drags from blank/footer space, clamps inside the display, and fits a
768-point viewport without hiding the picker or Capture action. Region guidance
hides during a selection drag. AppKit XCTest renders empty, selected, auto-start
and narrow controls; inspect those native pixels alongside the assertions.
`x11_capture_smoke.py --controls` reuses the exact-pixel capture oracle through
New Capture, including target retention, toolbar drag, frozen/live/countdown
sources, occluded windows, full-screen capture and cancellation. Its seven
scenarios persist 15 captures using real X11 input and simulated session state;
add `--target-shortcuts` to exercise target keys, window clearing/reselection,
same-selector identity, keyboard auto-start suppression and countdown isolation.
This is not hardware or physical multi-display acceptance.

## Capture History

The live workspace renders History like the shipping `CaptureHistory` window:
an "On this device" / **Capture History** header with the 30-day lede, counted
All/Screenshots/Video/GIF filter pills (hidden while History is empty), the
**Interrupted recordings** card, and an auto-fill grid of cards (minimum 252 pt,
16 pt gaps, 168 pt thumbnail). Each card shows the thumbnail (`contain` fit),
date ("Sep 26, 2026, 3:04 PM" in local time), "W × H · size" plus duration for
recordings, and dropped-frame warnings. Screenshots offer **Edit** and **Save
image**, recordings **Edit** and **Save file**; after export the second action
becomes **Show in Folder**. Clicking the thumbnail opens the editor. A recording
whose media is gone shows **File missing**, no actions, and is removed with one
click. Otherwise the trash control arms **Delete forever** for four seconds and
deletes on the second click. Secondary click lists the card's commands, including
**Copy image** for screenshots. Loading, empty ("No captures yet") and load/delete
error states use the shipping copy.

Copy, card details, actions, the missing-media rule and grid metrics come from
`captures_app::history_view`; AppKit reads them through the `history_copy`,
`history_cards` and `history_grid` settings operations. Both hosts virtualize
cards by row and decode thumbnails off the UI thread with bounded residency.
Unlike shipping, History never selects a card on load; an explicit selection
(click, arrow keys, a new capture or import) shows the accent ring, arrow keys
move it, Return opens it and Escape backs out of an armed deletion. Shipping's
**Restore** (reopen a floating preview) is not connected: the native workspace
keeps Save/Show in Folder instead. The workspace also keeps its native capture
controls above the grid; the window is not resizable on macOS.

## Live display-capture slice

Launch with `--live [--history-root PATH]` on either native host. This is an
explicit opt-in to real desktop capture, not a synthetic benchmark. The default
history is beside the separate Captures Native settings file, never installed
Preview history. Choose a display, request screen access if needed, and capture.
The host hides its window before capture and restores its prior visibility afterward.
Permission and locked/inactive session checks remain in force.

Both hosts use `captures-app` for display enumeration, PNG/thumbnail persistence,
history recovery, save and delete. Image files cross the ABI as paths, not base64.
Capture, file work and preview decode run off the UI thread. **Save image** uses
the output folder and PNG/JPEG/WebP format selected in Preferences, without
overwriting an unrelated file. Repeat Save reuses the existing export; a missing
export can be recreated from history. History always retains the lossless PNG.
JPEG composites alpha onto white; WebP saves losslessly, using the same encoders
as the shipping application. Deleting history preserves all exported formats.
**Delete all** arms **Delete all forever** (with Cancel) for four seconds; the
second click deletes the workspace's screenshot, video and GIF history copies,
including entries outside the selected filter. It leaves exported files, recording
recovery drafts and other history roots untouched. Cancel, Escape or the timeout
leave history unchanged. Both hosts reload after a failure, including partial
deletion, and keep the error visible. This also works with existing
local history on Wayland; the live capture restriction is separate.
Captures not deleted remain available on reopening the workspace.

Automatic copy follows Preferences (enabled by default, like shipping Captures).
Turn it off to leave the clipboard untouched by a new capture; explicit Copy
still works. A clipboard failure does not discard the captured image. Only an
explicit capture action can trigger automatic copy, never loading history or a
fixture screenshot. Save/capture report settings errors rather than silently
using different output or clipboard defaults.

Screenshot countdown follows the stored 0–10-second preference. The selected
display shows a native, fixed-media-palette countdown. Escape is registered only
for an active capture and cancels even when another app has focus. If registration
fails, capture is refused rather than losing cancellation. A desktop-session
watcher invalidates the pending capture on lock/inactivity; unlocking does not
resume it. Monotonic deadlines and capture generations are shared Rust logic.
The overlay closes before the host hides/settles and captures. Late worker replies
cannot restart a cancelled capture. Once the captured pixels commit to saving,
Escape no longer claims cancellation; accepted history/file work drains at quit.
Timers stop and Escape is released after completion/cancellation (the Windows
low-level capture hook stays installed but disarmed). Interactive permission,
focus, mixed-DPI, compositor, and screen-reader acceptance still needs real OS tests.

Cursor inclusion follows `show_cursor_in_screenshots`: the shared Rust engine
samples the pointer after countdown/window hiding and composites it before
history, copy, and export. This preserves shipping behavior: macOS system pixels
and hotspot, a synthetic arrow on Windows/X11, and no overlay when the pointer
is outside the selected display or unavailable. Wayland-only cursor acquisition
remains unsupported. Exact cursor shapes on Windows/Linux are not claimed.

Both native hosts offer **Capture region** on the selected display.
Draw from an empty selection, move it or resize its corners, choose Free/1:1/4:3/
3:2/16:9/9:16, and hold Shift for a square while dragging. Release Shift to restore
the chosen aspect. Confirm with Enter/Capture, or enable automatic start on
selection in Preferences. Escape covers preparation, selection and countdown.
Freeze follows Preferences; zero countdown uses the retained frame/cursor, while
any countdown captures fresh pixels. The AppKit panel borrows Rust-owned pixels
through an image provider and releases them after closing/worker completion.
This adds no full-desktop temporary file. `--scene region` provides a synthetic
selector using the same view without screen access; `--exercise` covers draw,
move, aspect, resize and Shift release. The Windows/X11 candidate uses the same
Rust `RegionSession`; its [private-X11 integration test](wgpu/README.md#validate-and-collect-evidence)
checks repeated captures, exact saved pixels and cancellation with simulated
session state. Real OS capture, mixed-DPI and accessibility acceptance is still
required. Selector blur, magnifier and the full capture-menu UI remain open.

Both hosts also offer **Capture window**. Hover a window or desktop, click to
select and confirm with Enter/Capture; automatic start confirms on click. Shared
Rust hit testing handles front-to-back targets, shell strips, half-open bounds
and platform coordinates. The retained `WindowSession` owns target descriptors,
frozen pixels/cursor and safe composited-crop/native-surface selection. If another
window covers the target, capture uses its native surface rather than saving the
covering pixels; this fallback reads current pixels even in frozen mode. A
countdown refreshes window geometry and pixels. A target that disappears or moves
to another display fails rather than saving stale bounds. Desktop/shell selection
saves display-mode history. AppKit calls the allocation-free Rust hit-test and
corner-radius ABI, not a Swift copy of the targeting or masking policy.

The private-X11 test checks two captures in each region/window scenario, exact
asymmetric PNG pixels and metadata, frozen versus fresh countdown and live
capture, an occluded window's native surface, a disappearing countdown target,
desktop fallback, clicked-target confirmation after moving the pointer, automatic
start without Enter, cross-application Escape and simulated lock/unlock. This is
software-rendered X11 integration evidence, not Mac/Windows hardware, Wayland, accessibility or
real login-manager acceptance. `--scene window` is a permission-free fixture on
both hosts. The slice remains experimental and all platform acceptance gates
stay open until real desktop/input tests pass.

This slice has no recordings or editor; those stored preferences
do not apply yet. Full UI parity remains
open. The wgpu Wayland backend cannot verify hiding its root window, so capture
is disabled there rather than photographing the app itself. Linux X11 needs an
active, unlocked desktop session; bare Xvfb normally has no session service and
must refuse capture. Verify real permission, clipboard ownership, multi-display
behavior and exported pixels on each OS before accepting the slice.

## Screenshot mini-preview stack

Both live workbenches retain recent screenshots in fixed-glass native cards.
Copy uses full-resolution pixels, Save uses current screenshot preferences and
becomes Reveal after export. Trash moves only that export to the OS trash,
then dismisses the card; an unsaved card only dismisses. Errors keep the card
available for retry. Private History files and metadata remain untouched.
macOS uses Finder (which may request automation permission), Windows uses the
Recycle Bin, and Linux uses its desktop trash specification. Actual Finder and
Recycle Bin behavior still needs physical-host acceptance.
Edit opens the exact screenshot in its native editor without showing a hidden
workspace, switching Preferences, or changing the selected History row. Repeated
Edit focuses the existing editor without resetting its pending edits.
Dismiss closes only the targeted card. Clear all dismisses a snapshot of the stack, preserving history,
exports and any later capture. There is no automatic dismissal timer or count cap.

Stacks start expanded, with newest cards nearest the configured top/bottom edge.
Overflow scrolls without dropping captures; chevron cues at the stack edges
scroll one card at a time. Show less parks a compact pile with
the newest card in front; clicking it expands the stack. Incoming captures and
capture cancellation preserve the parked state. Collapsed piles drag within
their capture display and fan on hover, respecting reduced motion. Native file
drag, the remaining 3D/exit effects and dust are not connected.

Show mini previews, all four placement corners and Include mini previews in
captures use the shared settings. Turning previews off hides retained cards and
resets collapse; enabling them restores an expanded stack. By default the host
hides the stack before preparing/capturing pixels, restoring it on cancellation/
error. Out-of-order decodes preserve capture order; late results cannot resurrect
dismissed cards. AppKit keeps actions alive
when Preferences replaces the workspace; closing the app closes the panel too.

The implementation reuses `captures-app::preview` for corner placement,
work-area/DPI math, membership, card poses, scroll content height and capture/
decode visibility generations. AppKit calls this policy through the versioned
`captures_preview_*_v1` ABI, `NativePreviewStack` and `NativePreviewPolicy`;
Windows/Linux calls Rust directly. Work areas come from `NSScreen.visibleFrame`,
the audited Windows `rcWork` query, or X11 EWMH properties clipped to the monitor.
If the candidate cannot query usable bounds, the screenshot stays in history and
the host reports that it cannot position a preview rather than guessing.

macOS uses a nonactivating panel; X11 uses an unmanaged notification window to
avoid activation by the window manager. Private-X11 tests cover real pixels,
placement, focus and actions; `x11_preview_smoke.py --stack` additionally checks
per-card routing, compact arrivals/cancellation and nondestructive Clear all in
all four corners, plus eight-card overflow at bottom-left. CI runs this stack
mode. macOS CI covers AppKit/ABI lifecycles and renders.
Physical macOS/Windows desktops, mixed-DPI monitors, compositor behavior,
transparent hit-region parity and screen-reader/keyboard access still need
acceptance. Wayland live capture/preview positioning remain unsupported.

## Build and try on macOS

Requires macOS 13+, Xcode command-line tools with Swift 5.9+, Rust 1.94, and Node
24 at build time. Node compiles design tokens and test fixtures; it is not bundled.

```sh
bash apps/native/macos/build.sh
apps/native/macos/.build/release/CapturesNative --scene preferences
apps/native/macos/.build/release/CapturesNative --live
apps/native/macos/.build/release/CapturesNative --scene history --history-count 1000
apps/native/macos/.build/release/CapturesNative --scene history --history-count 0
apps/native/macos/.build/release/CapturesNative --scene hud --appearance light
apps/native/macos/.build/release/CapturesNative --scene preview
# Compare identical workbench content using the original per-chip filter strategy:
apps/native/macos/.build/release/CapturesNative --scene preview --reference-chips
# Update notice fixture (stub status source; no updater is connected):
apps/native/macos/.build/release/CapturesNative --scene update --update-state error
```

Close each instance before starting another. Cmd+Q quits. Nothing installs into
Applications or changes the installed app's data, shortcuts, or updater.
Only the explicit live screen-access action requests capture permission.
The executable needs its SwiftPM resource bundle; run it from the build directory.

Preferences has section navigation and a Capture History fixture action.
Appearance/theme changes save automatically; history uses reusable native table
rows, with an empty-state switch. HUD Pause /
Resume changes fixture state; the timer is deliberately static. Preview has cold
and warm dissolve buttons plus Reset. Reduce Motion uses an immediate change.
`--exercise` runs six scripted actions (appearance changes, history end-to-end
scroll, HUD pause/resume, or alternating cold/warm dust); `--quit-after 30` exits
automatically. `--scene idle` creates no visible window.
`--scene update` opens the update notice in a transparent panel below a
simulated menu-bar icon. `--update-state` accepts `available`, `single`, `closing`,
`manual`, `downloading`, `restarting`, `error`, `checking` or `up-to-date`.
Buttons in the window switch states. Update now and Try again run the shared stub
through download progress and the restart countdown. Nothing is downloaded,
installed or relaunched, and links are logged rather than opened. Hide / What’s
new persists only to an explicit `--settings-file`.

These screens are **not full pixel or functional parity**. Native Preferences
includes Find, custom colors, persisted defaults and live system appearance.
Image-backed history, real recording, transparent desktop windows, preview
controls/pile and editor surfaces remain incomplete. Do not claim whole-app
savings from these development scenes.

## Checks and measurements

```sh
node --test scripts/native-tokens.test.mjs
python3 -m unittest discover -s apps/native -p 'test_*.py'
# Included by build.sh; generated resources must exist first:
swift test --package-path apps/native/macos -c release

# About 40 minutes: 10 workloads × (warmup + 3 trials) × 60 seconds.
# Output must name a new directory. No screenshots or capture access requested.
python3 apps/native/profile.py \
  --binary apps/native/macos/.build/release/CapturesNative \
  --output /tmp/captures-native-results
```

The runner records raw CPU counters, RSS samples, binary hash, readiness, and
separate effect/scene-construction timings. It fails on missing actions, premature
exit and effect errors. **RSS is not physical footprint**. CPU is process-only;
there is no WindowServer/helper accounting, presentation timing, wakeup/energy
measurement, or original-Tauri comparison in this runner. Use the tested #529
coalition/resource and frame-observer protocol for renderer comparisons, then
Instruments Time Profiler / Animation Hitches / Energy Log for the broader matrix
in `docs/native-rewrite.md`. Readiness and scripted-action timings measure CPU
submission, not first displayed pixels or hardware input latency.

For visual review, capture the specific workbench window using macOS Screenshot
(not your whole desktop), in light/dark, history empty/populated, HUD running/paused,
and preview before/during/after deletion. Test both 1× and 2× screens, keyboard
focus/Space activation, Reduce Motion, reset during an effect, moving between
screens and closing the preview. Send the captures and raw diagnostics together.

## Effect provenance and unresolved gates

`DustMath.swift` is imported unchanged from the
[tested AppKit reference](https://github.com/joswayski/captures/commit/9fa5698528ffafa59fc4a71df9a7c50c8cc6e42e).
Build-generated particles and expected poses come from shipping
`thumbnailExit.ts`; the large pose oracle is bundled only into tests, not the app.
The Swift differential test covers uneven times and each delay boundary.

The atlas candidate filters isolated, padded chips once instead of issuing one
Core Image render per chip. A 1×/2× static chip pixel test compares it to per-chip
filtering under both normal and flipped preview parents, and rejects blank output.
CI uploads representative rendered chip pairs as `native-pixels` for inspection;
set `CAPTURES_TEST_ARTIFACTS` to an output directory to retain them locally.
A missing Metal device explicitly skips these tests; a skip is **not** acceptance.
The full original WindowServer visual gate is
still required: this workbench's synthetic image and sampled Core Animation
keyframes differ from #529, and its source fade does not yet reproduce the original
blur/brightness/scale treatment. No performance or visual-parity result is claimed.

All cold texture preparation and animation construction are timed on the main
thread in this experiment so setup cost cannot disappear from reports. This is
not the production scheduling policy: if batching still blocks input, prepare
bounded resources off-main before use and supply a responsive fallback for a
cache miss. Warm reuse holds only one surface's textures, invalidated on backing
scale changes; switching scenes releases them. Core Animation plays submitted
keyframes without a recurring application timer/display link; completion has one
cleanup callback. Profile keyframe construction and residency too, not just blur.
