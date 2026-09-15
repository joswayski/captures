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

`--profile DIR` overrides the isolated experiment directory. `CAPTURES_GPUI_DATA`
does the same. The default is `captures-gpui-experiment` under XDG data, or
`~/.local/share`, or LOCALAPPDATA when HOME is unavailable. Do not point this at
a shipping Captures profile. The screenshot editor defaults to **Save as new file**
and refuses to replace an existing file in that mode. Turning that switch off
allows Save to replace the chosen file, including the opened source.
Mini-preview Delete **does delete the source file after confirmation**; Dismiss
only removes the card. Mock preview deletion never deletes the fixture.

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
`docs/reference/main-2026-09-13`. The GPUI editor chrome is visibly different.

| Area | Implemented | Still missing or different |
| --- | --- | --- |
| Preferences | Live appearance/accent settings, custom colors, selects, shortcut recording and conflict rollback, microphone picker, search/navigation, persisted settings | Complete keyboard/accessibility parity and native permission flows remain unverified |
| Screenshot editor | Drawing tools, transform handles, pan/zoom, editable header dimensions, grouped Shapes flyout, layers-first sidebar with image thumbnails, color/size/opacity controls, multi-image import, blend/rotation, clipboard, undo/redo, drafts, filename/format/save controls, PNG/JPEG/WebP export, real encoded before/after comparison, text wrapping/alignment/plates/outlines/shadows | Exact text/image/crop inspectors and export popover; original background cannot be unlocked; vector thumbnails use tool icons; numeric steppers and arbitrary background colors; bold/italic use synthetic raster treatments, not native font variants; CPU raster work can block interaction on large documents |
| Screenshot capture | Region/window/display targets, frozen/live frames, scaled crops, cursor/format/countdown settings, auto-start, copy/save/preview routing, session gate | Exact selector/menu rendering and in-place cross-monitor transitions; mixed-DPI acceptance; capture exclusion outside tested X11 regions |
| Recording | Native recording, pause/resume segments, countdown/restart cancellation, stop/delete, mic controls, session clock, screenshot during recording, hide/restore, 430×102 bottom-center HUD, passive region guide | Crash recovery; full-display controls exclusion on Linux; exact pulse/compositor equivalence; native macOS/Windows acceptance |
| Recording editor | Cancellable background preparation, video/audio preview, filmstrip/waveforms, trim/crop/resize, track edits, export settings/progress/cancellation | Exact timeline/interaction equivalence, hardware audio acceptance and recovery of interrupted native sessions |
| Mini previews | Four-corner stacks, mixed images/GIF/video posters, real image-fragment dissolve, rejection shake, reduced motion, edit/copy/save/dismiss/delete, native X11 file drag | macOS/Windows outbound drag is implemented but unverified; Wayland outbound drag unavailable; hovered animated GIF uses a blurred first frame; exact compositor/blur/frame-pacing parity |
| History | Durable chronological index, type filters, image/video previews, retention, confirmed file deletion, correct editor routing | Durable linkage of permanent exports to history; native recording crash recovery; exact grid/metadata parity |
| Feedback/onboarding | Text input, explicit feedback submission through shared client, persistence | Shipping multi-step onboarding, real permission actions/status, feedback categories/complete UX |
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

All commands above passed in the evaluation orb (77 standalone tests; the root
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
The [dissolve CPU microbenchmark](results/linux-effects.json) measured 0.0891ms
median and 0.6472ms p95 across 765 samples, excluding texture upload/presentation.
These refreshed samples include the GPUI native integration; they supersede the
earlier 115 MiB result. Memory was lower, but idle CPU was higher in this run.

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
