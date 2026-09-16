# Captures GPUI evaluation

**This is an incomplete port, not a feature-equivalent copy or a Preview replacement.**
The goal is to retain the shipping Tauri app's UI, behavior, and animations while
evaluating a Rust/GPUI frontend. That acceptance criterion has not been met.
Do not use the resource measurements to justify replacing Tauri yet.

This standalone Cargo workspace pins GPUI 0.2.2. It uses the existing capture,
session, recording, image, media, and feedback crates. The portable document,
draft, and encoder modules come from `experiments/windows-native`; **none of that
experiment's Win32 UI is used**. Text rendering and audio-edit filter logic are
shared with the existing Rust crates. Shipping downloads, release automation,
and Tauri settings are unchanged.

## Run

Follow the root development prerequisites first. Linux additionally needs
`libxkbcommon-x11-dev`, Fontconfig (`fc-match`) for the shipping font fallback,
a Vulkan-capable graphics driver, and a supported window system.
FFmpeg and ffprobe must be on PATH for video editing, tests, and export;
this experiment does not package the shipping application's media sidecars.

```sh
cargo build --release --manifest-path experiments/gpui/Cargo.toml
cargo run --release --manifest-path experiments/gpui/Cargo.toml -- --view preferences --light
cargo run --release --manifest-path experiments/gpui/Cargo.toml -- --view screenshot-editor --open /path/to/image.png
cargo run --release --manifest-path experiments/gpui/Cargo.toml -- --view recording-editor --open /path/to/video.mp4
cargo run --release --manifest-path experiments/gpui/Cargo.toml -- --view thumbnail --open /path/to/image.png --mock
cargo run --release --manifest-path experiments/gpui/Cargo.toml -- --view background --profile /path/to/experiment-profile
```

### Matched Mac component adapter

The separate `captures-gpui-parity` binary implements the version 1 protocol from
the [matched macOS harness](https://github.com/joswayski/captures/pull/529).
Build this checkout, then pass its absolute executable path to that harness:

```sh
cargo build --release --manifest-path experiments/gpui/Cargo.toml --bin captures-gpui-parity
bash /path/to/harness-checkout/experiments/macos-parity/run.sh \
  --gpui "$(pwd)/experiments/gpui/target/release/captures-gpui-parity"
```

Direct invocation is `captures-gpui-parity CONFIG_JSON OUTPUT_JSON` with absolute
paths. It supports all four dust/settle scenarios, exact frozen checkpoints and
live cycles, a borderless 640×720-point window at 2× backing scale, atomic readiness
and completion markers, and the actual macOS WindowServer window number. It stays
open until terminated. It never opens a capture profile or installs tray/shortcuts.
The ordinary `cargo run` command still launches the full GPUI experiment.

This adapter shares the production fragment renderer, centered cover sampler and
580ms settle curve. It uses **CPU rasterization plus GPUI texture uploads**, not a
GPU particle implementation. Setup measurements include rebuilding dust resources
each cycle. Callback intervals measure actual main-thread callbacks, **not
presented FPS**. macOS build/runtime acceptance must come from native CI and the
on-screen Mac run; Linux/Chromium comparisons cannot establish it.

The 24-checkpoint Linux diagnostic comparison still fails the harness pixel gate:
20 pass and 4 fail. Image elements now use device-snapped cover bounds, matching
WebKit's paint geometry; canvas dust retains its separate floating cover bounds.
The remaining failures are the 290ms survivor slide (also 2090ms after delete):
GPUI 0.2.2 floors physical image positions instead of retaining CSS-like fractional
translation. Blur/antialiasing also remain visually different. The gate was not
relaxed and no comparable performance trial was accepted. See
[raw diagnostic results](results/linux-component-checkpoints.json). The harness
must pass its visual checks on the Mac before its performance results are used.

### Full-app profiles and lifecycle

`--profile DIR` overrides the isolated experiment directory. `CAPTURES_GPUI_DATA`
does the same. The default is `captures-gpui-experiment` under XDG data, or
`~/.local/share`, or LOCALAPPDATA when HOME is unavailable. Do not point this at
a shipping Captures profile. The screenshot editor defaults to **Save as new file**
and refuses to replace an existing file in that mode. Turning that switch off
allows Save to replace the chosen file, including the opened source.
Mini-preview Delete removes the saved export after confirmation but keeps Capture
History for recovery; Dismiss only removes the card. Mock preview deletion never
deletes the fixture.

`--mock` enables explicitly synthetic screenshot/preview fixtures and disables
capture actions; it is not a fallback when real capture fails. `--capture`
opens the real selector. Real capture still uses the shared fail-closed desktop
session checks. Tray and global shortcuts remain available after the last visible
window closes. Closing/hiding recording controls keeps recording; New Capture
restores them. Use the tray's Quit action or the app's platform quit shortcut.
Launch-at-login uses a profile-specific `Captures GPUI <hash>` registration and
does not change the shipping app's startup registration.

## Parity inventory

“Implemented” below describes code, **not cross-platform certification**. The
source UI remains the reference: `apps/desktop/ui/src/App.tsx`, its routed
components, `shared/design.css`, `shared/themes.css`, and
`docs/reference/main-2026-09-13`. Editor dimensions and the inspected controls now
follow those sources; complete visual and interaction acceptance is still pending.

| Area | Implemented | Still missing or different |
| --- | --- | --- |
| Preferences | Live appearance/accent settings, custom colors, selects, shortcut recording and conflict rollback, microphone picker, search/navigation, persisted settings | Complete keyboard/accessibility parity and native permission flows remain unverified |
| Screenshot editor | Drawing/transform/pan/zoom, staged crop with aspect presets, original-image unlock, text presets/native font face selection/durable font metadata/wrapping/alignment/plates/shadows, image size/position steppers, real annotation thumbnails, layer labels/image rename/drag ordering and appearance/arrange/combine panel, pixel-preserving quarter-turn/flip with fresh-photo canvas rotation, custom background colors, clipboard/undo/drafts, output scale/custom dimensions and aspect lock, quality presets/size limits, draggable encoded comparison and PNG/JPEG/WebP export | Some numeric controls and keyboard/accessibility behavior differ; unavailable font traits still require synthesis; CPU raster work can block large-document interaction; exhaustive visual acceptance pending |
| Screenshot capture | Region/window/display targets, source-derived glass menu with draggable bounded placement and anchored dropdowns, six aspect choices/centered refit/aspect resize/live Shift snapping, FPS/resolution/audio/microphone controls, animated segmented controls/switches/panel/ready pulse, frozen/live frames, scaled crops, cursor/format/countdown settings, auto-start, copy/save/preview routing, session gate | Cross-monitor transition and mixed-DPI acceptance; selector keyboard/accessibility coverage; exact compositor equivalence; capture exclusion outside tested X11 regions |
| Recording | Native recording, durable session/segment journal, interrupted-recording recovery, pause/resume segments, countdown/restart cancellation, stop/delete, mic controls, session clock, screenshot during recording, hide/restore, 430×102 bottom-center HUD, passive region guide | Full-display controls exclusion on Linux; exact pulse/compositor equivalence; native macOS/Windows acceptance |
| Recording editor | Cancellable preparation, video/audio preview, source dropped-frame warning, filmstrip/waveforms, keyboard trim, crop numeric fields/steppers and aspect-locked handles, output presets/custom dimensions, independent track gain/mute/mono, quality/size-limit modes, sampled encoded estimates and draggable comparison, save-new/replace/progress/cancellation | Exhaustive timeline/interaction equivalence and hardware audio acceptance |
| Mini previews | Four-corner stacks, centered image/GIF/video crops, animated blurred GIF hover frames, source/fragment crossfade with expanding clip and backing-scale rasters, eased survivor settle with frozen outgoing positions, rejection shake, reduced motion, edit/copy/save/dismiss/delete, native X11 file drag | macOS/Windows outbound drag is implemented but unverified; Wayland outbound drag unavailable; exact compositor/blur/frame-pacing parity; normal/hover media caches currently rasterize at 2× |
| History | Responsive source-sized grid, header typography, hover elevation/motion, contained image/video posters, metadata and filter counts, timed Restore feedback, recording Save/Show in Folder, durable recovery/saved-path linkage, dropped-frame warnings preserved through recovery/export, missing-recording posters/direct removal, confirmed Delete all and individual deletion, retention, live refresh, interrupted recording recovery/discard | Dates and numeric counts use fixed formatting rather than OS locale formatting (dates do use local timezone); exhaustive keyboard/accessibility/compositor acceptance remains outstanding |
| Feedback/onboarding | Original single-screen permissions layout, native macOS screen/microphone request/settings/restart paths and status polling, completion persistence; feedback categories/contact/metadata, submit/pending/success/cooldown states through shared client | Native macOS permission prompts/restart and cross-platform visual acceptance remain unverified |
| Native integration/notices | Tray, shortcuts, zero-window keepalive, profile-isolated startup, launch notice, recording-ready/save/error and controls-hidden notices | Linux global shortcuts require X11; Linux launch notice has no tray anchor; updater/package/file-association integration and a GPUI release channel are not implemented |

The dissolve's grid, radial delays, cubic easing, and poses follow
`thumbnailExit.ts`, with numeric tests derived independently from that TypeScript.
It currently rasterizes fragments on the CPU and uploads each frame through
GPUI. This is not evidence of identical blur, antialiasing, GPU cost, or displayed
frame pacing. No effect is implemented as a screenshot of another UI.

The screenshot-editor visual correction uses the shipping 52px header, 56px rail,
320px sidebar, deeper light canvas well, and 77px export footer. The Shapes menu
uses the original 3×2 icon grid; layers and properties scroll independently.
Its SVGs are rasterized through GPUI's existing resvg version with explicit
premultiplied-RGBA → straight-BGRA conversion: GPUI 0.2.2's inline SVG decoder
otherwise swaps red and blue. Tests cover opaque and translucent colored pixels.
These corrections do not establish whole-app visual or animation parity.

## Validation and platform limits

```sh
cargo fmt --manifest-path experiments/gpui/Cargo.toml --all -- --check
cargo test --manifest-path experiments/gpui/Cargo.toml
cargo clippy --manifest-path experiments/gpui/Cargo.toml --all-targets -- -D warnings
npm run check
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The standalone tests include real FFmpeg fixture generation/decode/seek, exact
particle reference values, trim boundaries, image edit/undo/export, settings
persistence and malformed input, and preserving the source when saving onto
itself/a hard link. Missing FFmpeg fails the playback test rather than silently
skipping it.

All commands above passed in the evaluation orb (138 app tests and 15 component
adapter tests, including shared renderer tests in both binaries; the root
desktop suite contains 830 tests). The root release-version tests required
per-command `GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=commit.gpgsign
GIT_CONFIG_VALUE_0=false`: they create temporary commits, and the orb has no
signing key. No global Git configuration was changed.

The GPUI PR workflow runs this standalone workspace's tests, clippy, formatting,
and release build on native macOS, Windows, and Linux runners. It publishes no
release and is not a substitute for interactive native rendering checks.

Executed UI checks, not just code inspection:

- Light/dark Preferences and Ctrl+F filtering, screenshot annotation/text,
  undo/redo and PNG export, preview stack expand/collapse and fragment dissolve.
- Updated native 2× previews: GIF playback continues under blur/hover controls
  with its 310ms/690ms frame delays. A real confirmation/delete produced an
  in-place dissolve and delayed survivor slide, leaving two correctly placed
  cards and the fixture intact. The bottom toolbar no longer overlaps a card.
  The confirmation now displays its full two-line message in the narrow window;
  Linux Escape and default Enter cancel, Tab visibly selects Delete, and Tab then
  Enter removes exactly one card. Escape still cancels after selecting Delete.
  macOS/Windows keep their native prompts. Software-rendered animation still
  visibly stutters.
- Real display capture: 1600 × 1000 PNG containing only the known desktop color.
  Real region capture: dragging (110,160) to (730,480) saved a 620 × 320 PNG,
  also with no selector pixels; both opened the screenshot editor.
- Real region recording: countdown, pause, resume, stop, editor and MP4 export.
  Two H.264 segments (33.760333s and 3.028s) assembled/exported to 36.789s at
  620 × 320. The HUD was outside that region; this does **not** verify capture
  exclusion for whole-display recording.
- Video fixture playback, paused seek, trim and export: a 3.02–8.42s selection
  exported a 5.436s H.264/AAC file (frame/container rounding). Audio-preview
  filtering is implemented; the orb has no physical audio device for listening.
- Real recording HUD/guide stacking, Hide → configured New Capture shortcut →
  restore → Stop → editor. Escape during restart ended the session; a subsequent
  New Capture opened a new selector rather than restoring a stopped controller.
- Screenshot text outline, rounded plate/shadow, scrollable shadow inspector,
  and actual encoded compression comparison with byte estimates.
- Native font family/face/traits persist in the draft. Reopening the bold/italic
  monospace fixture retained its label and rendered canvas pixels (zero differing
  pixels in the restart comparison). Font menus were inspected opening below
  at 800px client height and above at 760px, with all four choices visible.
- Layer rows use actual colored annotation thumbnails and source-measured row
  geometry, checkerboards, labels, and hidden/locked states. Native image rename
  was exercised with Enter, Escape, and click-away, including a locked background
  and restart persistence. Dragging above/below a row changed persisted paint
  order; Undo restored it. Hidden annotations disappeared from the canvas while
  retaining dimmed thumbnails. The portable model's 84 tests include large-coordinate
  thumbnails, rotated/color/opacity previews, reorder boundaries, metadata and undo.
- Selector: a drawn 600 × 400 region refitted to a centered 400 × 400 square.
  Pressing/releasing Shift without moving the pointer changed a live 600 × 400
  drag to 600 × 600 and back. The full dragged panel remained inside the screen
  and the FPS list stayed anchored. A real capture through its recording button
  produced a 400 × 400, 23.297-second MP4, indexed it, and opened the editor.
  Inspected motion capture shows panel expansion/collapse, sliding action thumb,
  switch movement, and ready pulse. Tests cover interrupted motion, aspect
  resizing across the anchor, and frontmost-window hit testing.
- Cropped screenshot export decoded as 400 × 260. Locked original-image rotation
  changed both canvas and output from 960 × 540 to 540 × 960; Undo restored them.
  Unit tests also check asymmetric source pixels, both flip directions, restored
  eraser pixels, and one-step canvas undo (80 portable document/encoder tests).
- Custom screenshot output decoded as 600 × 338 with aspect lock and 600 × 150
  without it. Typing/stepper/focused ArrowUp produced a 602 × 339 PNG. Maximum
  size conversion preserved 0.05 MB = 50 KB; the saved PNG was 15,335 bytes.
- Screenshot and recording exports appeared in persisted history with saved
  locations. Deleting the screenshot's private copy through History left its
  permanent export intact. Tests cover legacy metadata, unindexed sources,
  format changes, retention boundaries, and non-regular/private path rejection.
- History was inspected in light/dark at 620, 880, and 1280px client widths,
  including portrait/landscape PNGs, GIF/MP4 posters, two/three/four-column grids,
  missing files, and the empty state. The shipping native Tauri window was also
  inspected: Chromium's font fallback differed in this orb. The heading retains
  shaped glyph kerning while applying the source letter spacing; description
  width uses the actual font's `ch` measurement. Hover uses the source 200ms
  curve, two-pixel lift, and small/medium elevation tokens, without moving other
  cards. Restore created a floating preview and showed timed feedback. Save
  persisted a permanent recording path. Delete all confirmation/cancellation
  and timeout were exercised; deletion preserved permanent exports. Tests
  cover missing-file metadata/posters, saved-file fallback, invalid index paths,
  a removed recovery directory, and same-timestamp directory changes.
- Dropped-frame warnings were inspected in light History with 17, 1, and zero
  drops, and in light/dark recording editors. Zero omits the warning and one uses
  singular text. A real FFmpeg recovery test retains 3 + 7 drops from playable
  segments but excludes an unusable segment reporting 123; metadata survives
  restart, export format changes, and loss of the private recovery file.
- A native 620 × 320 recording was paused, the app process terminated, and
  History reopened. Recover assembled a 24.083-second MP4, indexed it, opened
  the editor, and retired the draft. The first Discard click preserved media
  and showed confirmation. A separate native pause/resume/stop session also
  saved through the same journal/assembly path. Recover probes unfinished
  segments; an unplayable final partial segment cannot be reconstructed.
  GIF recovery retains its source segments for the shared 30-day retention.
- A five-second H.264 fixture trimmed to 1.000–4.000 seconds exported as a
  960 × 540, 3.018-second MP4. Crop fields were exercised by typing, steppers,
  and arrow keys. Comparison displays actual decoded encoded frames.
- Feedback submitted to a local test receiver; Linux onboarding completion
  persisted and closed the window. These checks do not exercise the production
  feedback service or native macOS permissions.
- Native X11 preview drag into a GTK file-drop receiver, Escape cancellation,
  clipboard file transfer, and source preservation. Recording-ready Save wrote
  a distinct MP4; save errors remained actionable instead of auto-dismissing.

Runtime verification is on Linux x64 in an orb, Xvfb + Openbox, software graphics.
GPUI 0.2.2 initially presented blank windows until a real resize in this setup;
the opt-in `CAPTURES_GPUI_X11_RESIZE_WORKAROUND=1` shrinks then restores the native
window. It is off on normal desktops. Adding xcompmgr in the tested lab
produced blank clients, so transparent wallpaper composition is **not verified**.
Windows cross-checking stopped before project code because MinGW GCC was absent;
macOS cross-checking stopped before project code because the Apple SDK/compiler
was absent. Neither OS was run here. Wayland, Retina/HiDPI, mixed-DPI
multi-monitor behavior, accessibility, and native permissions remain unverified.
The presence of platform-specific recorder code does not prove those platforms
work. There is no new stable or Preview release.

## Resource measurements

Build optimized first. Start one app in a disposable profile, exercise the
desired state, wait until it settles, capture and **inspect** its window, then:

```sh
python3 experiments/gpui/measure.py PID \
  --state 'GPUI release; Preferences light; 880x660 client; idle; isolated profile' \
  --screenshot /path/to/inspected-window.png > /tmp/gpui-preferences.json
experiments/gpui/target/release/captures-gpui --benchmark-effects
```

The Python probe reuses the earlier experiment's `/proc` process-tree sampler.
It reports RSS, PSS (shared pages apportioned), private resident memory, and CPU
as a percentage of one core, including children. Tree changes reject a sample.
Three five-second intervals are repeated observations in **one warm process**,
not three independent launches. It does not measure first-paint latency, GPU
memory, power, or hardware frame pacing. Screenshot existence is required, but
the caller remains responsible for checking that the workload actually rendered.

The effects microbenchmark has one complete animation warmup, then five repeats
of 153 CPU-raster frames at sampled 60Hz animation times. It excludes GPUI upload,
composition and presentation. Its timings are **not application FPS**.

### Observed results — not a feature-equivalent comparison

September 15, 2026; Linux x64, 2 logical CPUs, Xvfb/Openbox without a compositor,
Mesa 25.0.7 llvmpipe/LLVM 15 software graphics. Both apps displayed an inspected
880 × 660 light Preferences client. Medians of three five-second intervals:

| Metric | GPUI incomplete port | Existing Tauri app |
| --- | ---: | ---: |
| Proportional resident memory (PSS) | 149.53 MiB | 543.79 MiB |
| Private resident memory | 146.91 MiB | 398.23 MiB |
| Summed process RSS (double-counts shared pages) | 158.89 MiB | 1131.00 MiB |
| Process count | 1 | 6 |
| Idle CPU, percentage of one core | 1.40% | 0.40% |

Raw samples and executable/screenshot hashes:
[GPUI](results/linux-gpui-preferences.json),
[Tauri](results/linux-tauri-preferences.json).
The historical [dissolve CPU microbenchmark](results/linux-effects.json) measured 0.0891ms
median and 0.6472ms p95 across 765 samples, excluding texture upload/presentation.
It predates the corrected per-chip filtering/source crossfade/clip renderer and
does **not** describe current dissolve performance.
These refreshed samples include the GPUI native integration; they supersede the
earlier 115 MiB result. Memory was lower, but idle CPU was higher in this run.

The [corrected 1× renderer diagnostic](results/linux-effects-corrected.json)
measured **4.09ms median, 18.54ms p95, 23.37ms maximum** over the same 765 CPU-only
samples, after 153 warmup frames. Correct filtering/composition costs substantially
more than the earlier incomplete effect. These numbers exclude texture upload and
presentation, do not cover Retina 2×, and do not establish frame-rate parity.

On September 16, both optimized apps displayed the same unmodified 960 × 540
fixture in a 1280 × 760 light screenshot editor: one locked original layer,
Save as new enabled, and no expanded menus or export panel. Both windows were
captured and inspected; the Tauri pre-created notice was unmapped and the editor
repainted to remove an X11 obstruction before sampling. No build/encoder ran
during measurement. Medians of three five-second intervals in one warm process:

| Metric | GPUI incomplete port | Existing Tauri app |
| --- | ---: | ---: |
| Proportional resident memory (PSS) | 153.45 MiB | 710.76 MiB |
| Private resident memory | 150.55 MiB | 539.39 MiB |
| Summed process RSS | 162.82 MiB | 1327.41 MiB |
| Process count | 1 | 6 |
| Idle CPU, percentage of one core | 2.00% | 5.60% |

[GPUI editor samples](results/linux-matched-gpui-editor.json) and
[Tauri editor samples](results/linux-matched-tauri-editor.json) include exact
binary/screenshot hashes. These are matched visible editor states, not equivalent
application services or repeated-launch distributions. Resizing/repainting and
software-renderer allocation history affect the measurements. They do not prove
the GPUI port is faster during editing or on a hardware GPU.

An earlier post-export GPUI workload at
1280 × 760 measured **202.16 MiB PSS**, **199.55 MiB private resident memory**,
**211.21 MiB RSS**, and **1.40% idle CPU** (one process; the same three warm
five-second intervals). The 960 × 540 fixture had just been exported at
602 × 339 and the export panel was closed. No build/encoder ran during sampling.
[Raw editor sample](results/linux-gpui-screenshot-editor.json) records exact
binary and inspected screenshot hashes. This sample predates recording-journal
integration and has no matched Tauri editor workload; do not compare it to the
Preferences figures as if they measured the same state.

The Tauri reference is the source at
[`d4d2016`](https://github.com/joswayski/captures/commit/d4d2016d29ea1f882d6b98fc9656d02d5638fbb0),
with its real embedded frontend, not an empty dev-server window:

```sh
npm run build --workspace @captures/desktop
cargo build --release -p captures-desktop --bin captures --features tauri/custom-protocol
```

Tauri used a disposable HOME/XDG profile; outbound HTTP was blocked through an
unavailable proxy and the orb had no audio devices. GPUI used its separate
`--profile`. No production account, captures, or settings were used. These
measurements do not characterize normal hardware GPU memory or CPU overhead.

A whole Tauri process has tray/shortcut/update services and pre-created webviews.
GPUI now has tray/shortcut/startup services and a hidden native keepalive, but no
updater or pre-created webviews. Identical window sizes still do not make these
workloads feature-equivalent. A useful migration decision needs matched workflows
after parity, repeated launches, actual Mac/Windows GPU measurements, animation
frame pacing, and capture/encoding tests on those machines.
