# Native macOS implementation experiment

`experiments/macos-native` is an opt-in SwiftUI/AppKit/AVKit frontend with an
in-process Rust static library, not a Tauri window or a webview wrapper. Like the
Linux-native experiment, it is separate from the shipping cross-platform app.
The Preview workflows, bundle identity, installed app, and profiles are unchanged.
See [build instructions](../DEVELOPMENT.md#macos-native-experiment).

## Implemented paths, not a claim of complete parity

The native sources implement:

- A menu-bar app, document windows, Open With, launch argument forwarding,
  configurable Carbon shortcuts, permissions, and opt-in login startup.
- Display, region, and window screenshots; private frozen region/display frames;
  cursor inclusion; cancellable countdown; region dragging and resizing.
- ScreenCaptureKit MP4/GIF recording, microphone/system audio, pause, resume,
  restart, mute, hidden/restorable controls, screenshots while recording,
  console-lock auto-pause, segmented private drafts, and recovery.
- Floating corner previews with collapsed/expanded stacks, dragging files,
  copy/save/open actions, explicit trash confirmation, and dust dismissal.
- Layered image editing: source/imported images, text, shapes, arrows, freehand,
  selection/resize/rotation/opacity, lock/visibility/order/duplicate/delete,
  undo/redo, crop/canvas resize/rotate/trim, brush erase/restore, and private drafts.
- PNG/JPEG/WebP export via Rust, real encoded before/after comparison, quality
  and maximum-byte controls. Export only creates new files. Only explicitly
  confirmed image Save replaces the source, after successful staging.
- Native recording playback, draggable trim handles/playhead, looping, crop
  move/eight resize handles/numeric sizing/aspect lock, resolution/quality/audio
  controls, MP4/GIF export and actual encoded frame comparison.
- Separate 30-day history and preferences with light/dark/system appearance,
  nine accent palettes and custom colors. Neutral surfaces, typography/spacing
  metrics and easing derive from the shipping CSS tokens at build time.

These are implemented code paths, not evidence that capture, audio, permission
prompts, playback, or animations passed on a physical Mac. In particular,
**this is not yet a visually verified, feature-complete replacement**:

- SwiftUI/AppKit control rendering, SF Symbol icons, typography, window chrome,
  focus, hit targets and animation timing still need side-by-side native review.
  Sharing color/spacing/easing tokens alone does not prove pixel parity.
- Frozen window screenshots crop the visible display, including occlusion, rather
  than extracting unobscured window contents. Re-invoking a capture shortcut
  recreates the overlay rather than capturing
  the existing capture menu. Region aspect presets and a persistent recording
  region indicator are not implemented.
- Image editing does not yet reproduce the shipping color-wand background
  removal, text/brush shadow controls, rotation handle/snap, or all pan/zoom
  gestures and off-canvas expansion affordances.
- PNG compression is lossless, without palette quantization. A maximum-size
  request fails if lossless PNG exceeds it; it does not silently resize pixels.
- The reused media crate does not export WebM; the native picker disables it.
  AVPlayer's WebM playback support is OS/codec-dependent. GIF comparison uses a
  decoded still, not synchronized animated before/after playback.
- Recording click highlights and keystroke overlays are not enabled. Live
  encoded-size estimation and recording-edit draft persistence are absent.
- No updater, feedback/crash reporting, launch/update notices or shipping setup
  walkthrough. No automatic OS shortcut changes. These differences are deliberate
  while the app has a separate experimental identity.

## Safety boundaries

Swift dispatches capture/lifecycle operations off the UI thread, separately from
stateless encoding. The Rust recording thread retains its console safety tick
during exports. The bridge rejects capture when the console is locked/inactive;
AppKit also hides capture surfaces on session resign/lock notifications.

New exports publish atomically without replacing an existing destination. Private
comparison outputs are not inserted into history. Settings/history/drafts are
local to the experiment; no captures, telemetry or feedback are uploaded. FFmpeg
is local. Dependency/sidecar preparation and GitHub CI still use the network.
See the [bridge protocol](../experiments/macos-native/bridge/PROTOCOL.md) for exact
operation fields, units, ownership, cleanup and failure behavior.

## Before/after pictures and benchmarks

No native macOS after picture or performance result was collected in the Linux
orb. Browser harness screenshots are **before design references only**, not a
native macOS baseline. Linux timings/RSS cannot establish macOS performance.
The repository includes an executable Mac measurement procedure instead of
invented numbers.

Build both release apps. Use the same Mac, screen/scaling, window content size,
appearance, fixture, audio sources and settings. Quit the other app before each
run to avoid shortcuts and background work interfering. For each implementation,
review light and dark Preferences, the same layered image fixture, the same
trimmed recording, and collapsed/expanded previews in all four corners. Record
short clips for stack expansion, dismissal, countdown and recording controls;
screenshots cannot verify motion. Also test Reduce Motion, multiple displays,
Retina crop coordinates, session lock, permission denial and export failures.

For settled document-window RSS/CPU diagnostics and a screenshot:

```sh
mkdir -p experiments/macos-native/build
xcrun swiftc experiments/macos-native/window_probe.swift \
  -o experiments/macos-native/build/window-probe

# Start the app yourself, open the desired window, then obtain its PID from
# Activity Monitor. Run the probe to list its visible window IDs and dimensions.
experiments/macos-native/build/window-probe 12345

python3 experiments/macos-native/measure.py \
  --pid 12345 --window-id 67890 --label native --state preferences-light \
  --window-probe experiments/macos-native/build/window-probe \
  --settle 10 --samples 10 --interval 2 \
  --output .amp/in/artifacts/macos-comparison/native-preferences-light-run1.json
```

Replace example PID/window ID with actual values. Repeat with `--label tauri`
and a new output name, alternating implementations for at least three runs. Use
`--fixture /path/to/same-file.png` for editors; the JSON records its SHA-256, not
the file contents. Grant the calling terminal Screen Recording permission for
window screenshots. Inspect each paired PNG before using its measurements.

The sampler records raw process-tree RSS and CPU deltas plus OS/hardware/window
metadata. It refuses missing/changed windows or changing process trees. **RSS is
not physical footprint or PSS**; summing process RSS double-counts shared pages.
WebKit XPC processes can belong to launchd instead of the app's process tree;
unattributed WebKit names are reported and `complete_app_memory` is always false.
Do not report these medians as total app RAM, startup time, FPS, energy or a
percentage native performance improvement. Whole-app memory/energy and animation
performance need Instruments with explicit WebKit attribution and matched work.
Capture/export latency needs timed matched operations, separately from idle
sampling. Results remain pending until those workloads run on macOS.

## Validation scope

`check.sh` covers Python token/measurement tests, portable Swift model tests,
Swift parsing, and bridge Rust tests/fmt/clippy. On macOS it additionally builds
and signs the app and runs direct XCTest model tests for asymmetric transforms,
undo, locked layers and no-overwrite exports. PR CI uses the macOS 26 SDK and
uploads a local-test ZIP only after those checks pass. A successful compilation
still does not verify native UI fidelity, system permissions or actual capture.
