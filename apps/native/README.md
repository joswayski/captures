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
default is not an engine integration: global shortcuts, microphone discovery,
login items, feedback and update actions remain visibly unavailable. Full
Preferences visual/input parity and the other checklist gates remain open.

## Live display-capture slice

Launch with `--live [--history-root PATH]` on either native host. This is an
explicit opt-in to real desktop capture, not a synthetic benchmark. The default
history is beside the separate Captures Native settings file, never installed
Preview history. Choose a display, request screen access if needed, and capture.
The host hides its window before capture and restores it on success or failure.
Permission and locked/inactive session checks remain in force.

Both hosts use `captures-app` for display enumeration, PNG/thumbnail persistence,
history recovery, save and delete. Image files cross the ABI as paths, not base64.
Capture, file work and preview decode run off the UI thread. **Save image** uses
the output folder and PNG/JPEG/WebP format selected in Preferences, without
overwriting an unrelated file. Repeat Save reuses the existing export; a missing
export can be recreated from history. History always retains the lossless PNG.
JPEG composites alpha onto white; WebP saves losslessly, using the same encoders
as the shipping application. Deleting history preserves all exported formats.
Captures remain available on reopening the workspace.

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

The AppKit workspace also offers **Capture region** on the selected display.
Draw from an empty selection, move it or resize its corners, choose Free/1:1/4:3/
3:2/16:9/9:16, and hold Shift for a square while dragging. Release Shift to restore
the chosen aspect. Confirm with Enter/Capture, or enable automatic start on
selection in Preferences. Escape covers preparation, selection and countdown.
Freeze follows Preferences; zero countdown uses the retained frame/cursor, while
any countdown captures fresh pixels. The native panel borrows Rust-owned pixels
through an image provider and releases them after closing/worker completion.
This adds no full-desktop temporary file. `--scene region` provides a synthetic
selector using the same view without screen access; `--exercise` covers draw,
move, aspect, resize and Shift release. Windows/Linux selector integration is
in progress separately; real OS capture, mixed-DPI and VoiceOver acceptance is
still required. Selector blur, magnifier and the full capture-menu UI remain open.

This slice has no window capture, recordings, editor or
mini previews; those stored preferences do not apply yet. Full UI parity remains
open. The wgpu Wayland backend cannot verify hiding its root window, so capture
is disabled there rather than photographing the app itself. Linux X11 needs an
active, unlocked desktop session; bare Xvfb normally has no session service and
must refuse capture. Verify real permission, clipboard ownership, multi-display
behavior and exported pixels on each OS before accepting the slice.

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
