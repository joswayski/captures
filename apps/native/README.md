# Native desktop workbench — stage 1

This is the first implementation stage of the [native rewrite](../../docs/native-rewrite.md),
**not a usable capture application**. The existing Tauri Preview is unchanged.
macOS is Swift/AppKit + Core Animation with Core Image explicitly backed by Metal.
No WebView, React, JavaScript runtime, Rust sidecar, network service or capture
permission is used. Rust engine integration is the next stage, not implemented
here. Windows and Linux renderer choices remain open pending prototypes.

## Build and try on macOS

Requires macOS 13+, Xcode command-line tools with Swift 5.9+, and Node 24 at build
time. Node compiles existing design tokens and test fixtures; it is not bundled.

```sh
bash apps/native/macos/build.sh
apps/native/macos/.build/release/CapturesNative --scene preferences
apps/native/macos/.build/release/CapturesNative --scene history --history-count 1000
apps/native/macos/.build/release/CapturesNative --scene history --history-count 0
apps/native/macos/.build/release/CapturesNative --scene hud --appearance light
apps/native/macos/.build/release/CapturesNative --scene preview
# Compare identical workbench content using the original per-chip filter strategy:
apps/native/macos/.build/release/CapturesNative --scene preview --reference-chips
```

Close each instance before starting another. Cmd+Q quits. Nothing installs into
Applications or changes the installed app's data, permissions, shortcuts, or updater.
The executable needs its SwiftPM resource bundle; run it from the build directory.

The sidebar switches fixture scenes. Appearance/theme buttons work in memory;
history uses reusable native table rows, with an empty-state switch. HUD Pause /
Resume changes fixture state; the timer is deliberately static. Preview has cold
and warm dissolve buttons plus Reset. Reduce Motion uses an immediate change.
`--exercise` runs six scripted actions (appearance changes, history end-to-end
scroll, HUD pause/resume, or alternating cold/warm dust); `--quit-after 30` exits
automatically. `--scene idle` creates no visible window.

These screens preserve the token palette and basic hierarchy, **not pixel or
functional parity**. They omit full Preferences/search/custom colors, image-backed
history rows, real recording, transparent desktop windows, preview controls/pile,
and editor surfaces. They must not be used to claim whole-app savings. System
appearance is resolved at scene construction; live OS appearance changes are a
later parity item.

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
