# Captures GPUI evaluation

**This is an incomplete port, not a feature-equivalent copy or a Preview replacement.**
The goal is to retain the shipping Tauri app's UI, behavior, and animations while
evaluating a Rust/GPUI frontend. That acceptance criterion has not been met.
Do not use the resource measurements to justify replacing Tauri yet.

This standalone Cargo workspace pins GPUI 0.2.2. It uses the existing capture,
session, recording, image, media, and feedback crates. The portable document,
draft, and encoder modules come from `experiments/windows-native`; **none of that
experiment's Win32 UI is used**. Shipping application code, downloads, release
automation, and Tauri settings are unchanged.

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
```

`--profile DIR` overrides the isolated experiment directory. `CAPTURES_GPUI_DATA`
does the same. The default is `captures-gpui-experiment` under XDG data, or
`~/.local/share`, or LOCALAPPDATA when HOME is unavailable. Do not point this at
a shipping Captures profile. Source images are not overwritten by editor exports.
Mini-preview Delete **does delete the source file after confirmation**; Dismiss
only removes the card. Mock preview deletion never deletes the fixture.

`--mock` enables explicitly synthetic screenshot/preview fixtures and disables
capture actions; it is not a fallback when real capture fails. `--capture`
opens the real selector. Real capture still uses the shared fail-closed desktop
session checks. Closing recording controls stops the native segment, but recovery
of those segment files is not yet exposed in the UI. Quit the terminal process
when finished; there is no tray lifecycle integration.

## Parity inventory

“Implemented” below describes code, **not cross-platform certification**. The
source UI remains the reference: `apps/desktop/ui/src/App.tsx`, its routed
components, `shared/design.css`, `shared/themes.css`, and
`docs/reference/main-2026-09-13`. The GPUI editor chrome is visibly different.

| Area | Implemented | Still missing or different |
| --- | --- | --- |
| Preferences | Semantic light/dark colors, preset/custom theme parsing, persisted settings, folder picker, Ctrl+F card filtering | Live system appearance/accent propagation across windows; custom color fields; shipping search navigation/highlights and Cmd+F; real select menus; some settings have no consumers |
| Screenshot editor | Source loading, pen/rectangle/ellipse/arrow gestures with preview, text input with embedded system font, move, crop, undo/redo, layer visibility/lock/duplicate/order/opacity/blend/rotation/delete, flatten/trim, draft persistence, PNG/JPEG/WebP export | Shipping layout/icons; transform handles/pan/canvas resizing; multi-image import, full text styling, clipboard, export settings/destination UI; rendering and autosave still block the UI thread |
| Screenshot capture | Region/window/display targets, scaled crop, visible region outline, fullscreen selector minimized before capture, session gate | Freeze-frame, cursor and format preference wiring, screenshot countdown, automatic copy/preview/output-folder behavior, exact overlay/menu and selection rendering; selector exclusion unverified outside X11/Openbox |
| Recording | Shared OS recorder, countdown, separate segments for pause/resume, finalization/assembly, continuous session gate, connected controls window | Shipping HUD and capture exclusion; restart/mic mute/device selection, shortcuts/tray; segment recovery and retention policy; controls may appear in captured media |
| Recording editor | FFmpeg frame decoding, play/pause/seek, bounded latest-frame storage, generation-fenced seek, trim handles, MP4/GIF export, cancellation/progress | Audio preview, shipping timeline/frame strip/waveforms, crop/resize controls, track edits, full export presets/WebM UI, draft recovery |
| Mini previews | Image cards, stack expansion/collapse, real image-fragment dissolve, edit/save/dismiss/delete | Exact chrome/geometry, blur/compositing equivalence, all corners, drag-and-drop/clipboard, rejection shake, reduced-motion support, video cards |
| History | Read profile captures, image previews, type filters, open appropriate editor, confirmed file deletion | Shipping grid/metadata, durable dismissal, retention, recording draft recovery, thumbnails for video |
| Feedback/onboarding | Text input, explicit feedback submission through shared client, persistence | Shipping multi-step onboarding, real permission actions/status, feedback categories/complete UX |
| Other surfaces/OS integration | CLI view dispatch; unimplemented views fail explicitly | Tray/global shortcuts/autostart/updater/package/file-association integration; launch/update/saved/hidden notices and region indicator |

The dissolve's grid, radial delays, cubic easing, and poses follow
`thumbnailExit.ts`, with numeric tests derived independently from that TypeScript.
It currently rasterizes fragments on the CPU and uploads each frame through
GPUI. This is not evidence of identical blur, antialiasing, GPU cost, or displayed
frame pacing. No effect is implemented as a screenshot of another UI.

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

All commands above passed in the evaluation orb (20 standalone tests; the root
desktop suite contains 830 tests). The root release-version tests required
per-command `GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=commit.gpgsign
GIT_CONFIG_VALUE_0=false`: they create temporary commits, and the orb has no
signing key. No global Git configuration was changed.

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
  exported a 5.436s H.264/AAC file (frame/container rounding). Audio preview is
  unavailable even though audio survives export.

Runtime verification is on Linux x64 in an orb, Xvfb + Openbox, software graphics.
GPUI 0.2.2 initially presented blank windows until a real resize in this setup;
the X11 startup path nudges width by one pixel and restores it. That workaround
needs validation on normal desktop drivers. Adding xcompmgr in the tested lab
produced blank clients, so transparent wallpaper composition is **not verified**.
Neither macOS nor Windows was built or run here; Wayland, Retina/HiDPI, mixed-DPI
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
| Proportional resident memory (PSS) | 115.47 MiB | 541.78 MiB |
| Private resident memory | 114.27 MiB | 396.00 MiB |
| Summed process RSS (double-counts shared pages) | 120.14 MiB | 1131.34 MiB |
| Process count | 1 | 6 |
| Idle CPU, percentage of one core | 0.40% | 0.60% |

Raw samples and executable/screenshot hashes:
[GPUI](results/linux-gpui-preferences.json),
[Tauri](results/linux-tauri-preferences.json).
The [dissolve CPU microbenchmark](results/linux-effects.json) measured 0.0951ms
median and 0.7376ms p95 across 765 samples, excluding texture upload/presentation.

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

A whole Tauri process has tray/shortcut/update services and pre-created webviews;
this GPUI app does not. Even with identical window sizes these workloads are not
feature-equivalent. A useful migration decision still needs matched workflows
after parity, repeated launches, actual Mac/Windows GPU measurements, animation
frame pacing, and capture/encoding tests on those machines.
