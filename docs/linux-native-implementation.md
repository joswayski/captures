# Linux-native implementation: working workflows, not yet parity

The Linux experiment now has a real GTK3/Cairo frontend, not a webview and not
just the original screenshot probe. It reuses Captures' Rust capture, recording,
media, and session crates. The shipping Tauri/React app is unchanged.

**Native does not require losing particles, stacking, transparency, or custom
design.** This implementation renders an animated preview stack and image-chip
dismissal with Cairo in a transparent native window. Native widgets do not give
us the existing React interactions automatically: we must reproduce and verify
them. The current result is visibly different and has substantial feature gaps.
It does **not** satisfy a no-regressions replacement requirement yet.

This is a Linux-first review milestone. Do not retire Tauri or treat these results
as approval to start Windows/macOS replacements. The earlier **46 ms / 18.6 MiB**
figures apply only to the [minimal probe](native-ui-evaluation.md), not this app.

## Feature status

“Exercised” below means real native GTK input/capture/export checks, not a mocked
backend. “Implemented” without that qualifier means the path exists but does not
have equivalent end-to-end evidence. The browser harness is only a visual
reference for Tauri surfaces; it is never used for the resource benchmark.

| Feature | Native implementation and verification | Remaining differences / gaps |
| --- | --- | --- |
| Capture menu | Screenshot, video, GIF; region/window/display choices; audio toggles and countdown settings | Two-step native form/selector, not the same floating menu; no global capture shortcuts or tray |
| Region screenshot | Real reverse-drag 380×220 PNG exercised; frozen screen, Shift-square, Enter/Escape implemented | Primary display only; no preset aspect ratios, live-selection preference, or auto-start; no screenshot pointer composition |
| Window screenshot | Real X11 window targeting/capture exercised | Primary-display selector; no proven multi-monitor/DPI parity; live window capture rather than exactly preserving the frozen window frame |
| Full-display screenshot | Real 1280×800 display PNG exercised | No display picker; primary display only |
| Mini previews | Expand/collapse, stacking, hover transition, 220-chip particle dismissal exercised; files survive dismissal; copy, edit and file drag implemented | Different layout/buttons; at most three visible cards and twelve retained images; incomplete per-card actions, Clear all, drag/edge placement and exact animation parity |
| Image editor | Draw, arrow, rectangle, ellipse, text, imported image layers, hit testing/moving, undo/redo, crop, whole-image 90° rotation, canvas resize, PNG/JPEG/WebP and clipboard implemented | No layer panel/reorder, resize/rotation handles, complete style controls, eraser, drafts, save-over-original, compression comparison or full keyboard/accessibility parity |
| Image correctness | Arrow changes output pixels; undo restores the exact source; redo, reverse crop, PNG/JPEG export exercised; asymmetric alpha/channel/crop unit tests | Not a complete test of every drawing tool, import, clipboard or WebP option |
| Canvas / asset authoring | Native New canvas dialog, opaque/transparent background, shared image editor; transparent 960×540 PNG creation/export exercised | Basic canvas authoring, not a reproduction of every asset/compositing workflow |
| MP4 recording | Real full-display pause/resume/stop/finalization and window recording exercised; dimensions/duration verified with ffprobe | No restart, live mic changes, click/keystroke overlay controls, screenshot-during-recording workflow, region recording border or complete failure/draft recovery |
| GIF recording | Real reverse-drag region recording produced a 380×220 GIF; codec/duration verified | Other target/format combinations are not individually exercised; no throughput comparison |
| Desktop audio / microphone | Existing recording crate options connected to toggles | No real audio device or loopback recording verified in this orb; device selection and live mixing remain incomplete |
| Session safety | Existing fail-closed session checks reused; lock during selection cancels without a saved capture and restores menu; lock during window recording automatically pauses, then manual resume works | Test uses an isolated DBus ScreenSaver authority, not an actual GNOME/KDE lock screen; real-desktop transition latency remains unverified |
| Recording controls | Native pause/resume/stop/discard/hide controls | Hide replaces the bar with a smaller notice, not total capture exclusion; Linux compositor exclusion is not solved |
| Video/GIF editor | Real extracted-frame scrubbing, numeric trim/crop/scale/audio controls, MP4/GIF export and cancellation implemented; generated-video trim/crop/scale unit test verifies output dimensions and duration | **Playback opens the system player**, not an embedded player; WebM export disabled; no graphical trim/crop handles, compression comparison, full audio-track controls or quality UI |
| History | Separate output-directory list; reopen images/recordings; saved files survive preview dismissal | No production-library migration, thumbnails/filter/search, 30-day retention or draft management |
| Preferences | Separate persisted appearance, directory, FPS, countdown, recording pointer and preview corner settings | Not the full Tauri preferences set; system appearance sampled on launch rather than live OS-change tracking |
| OS integration / distribution | Linux X11 development executable; file opening via UI/CLI | No Wayland implementation, installer, updater, tray, global shortcuts, single-instance routing, file associations, autostart, diagnostics or onboarding parity; Windows/macOS explicitly gated |

The native app keeps its own data under `captures-linux-native`, configurable via
`CAPTURES_NATIVE_DATA`; it does not migrate or overwrite the production library.
Export creates a new file. Recording/export uses a private staging directory and
same-filesystem non-overwriting publication; failed finalization retains stopped
segments for an in-process retry. This is not crash-recovery/draft parity.

## Before/after review

Each comparison labels **Tauri before** and **GTK native after**. Images are actual
renders, not generated UI concepts. They are scaled to fit, not pixel-equivalent
screenshots. Tauri overlay/menu/stack/history/video images come from the documented
React dev harness with representative mock data. Native screenshots are real GTK
windows on an isolated 1280×800 X11 desktop. Preferences and the image-editor
resource states use the actual locally built Tauri application.

The image-editor benchmark uses the same 960×540 input on both implementations.
Other visual examples use representative content, so their text/dimensions/history
entries need not match. The native fixture itself is a screenshot from this repo;
UI visible *inside that image* is not another native implementation.

### Capture entry and targeting

![Capture menu comparison](images/native-ui/capture-menu.webp)
![Region selection comparison](images/native-ui/region.webp)
![Window selection comparison](images/native-ui/window.webp)
![Full-display selection comparison](images/native-ui/display.webp)

Full-display capture shares these selectors. The native app currently opens the
primary display directly; it does not reproduce Tauri's display switcher.

### Stacking and particles are possible without a webview

![Collapsed preview comparison](images/native-ui/previews-collapsed.webp)
![Expanded preview comparison](images/native-ui/previews-expanded.webp)

The [Tauri clip](images/native-ui/tauri-dust.webm) and
[native clip](images/native-ui/native-dust.mp4) show dismissal in both implementations. The Tauri
clip triggers dismissal in the browser harness; the native clip exercises its
GTK action and Cairo animation. They demonstrate the effects, not equal animation
frame rates, exact motion parity, or end-to-end pointer accessibility. Stack images
crop the relevant desktop area; background windows can be cut off by that crop.

### Image editing and canvas authoring

![Image editor comparison on identical content](images/native-ui/image-editor.webp)

![Native transparent canvas](images/native-ui/canvas.webp)

The blank canvas uses the native image editor above. This additional native view
is not presented as a separate feature-equivalent Tauri asset-creator screenshot.
The export has real alpha transparency; its current preview is white rather than
a transparency checkerboard, another visual difference to address.

### Recording and video editing

![Recording controls comparison](images/native-ui/recording.webp)
![Recording editor comparison](images/native-ui/video-editor.webp)

The Tauri video reference is a browser-harness render with a decoded local clip.
The actual Tauri/WebKit player in this orb returned `NotSupportedError` on Play,
even after Linux H.264 decoder packages were installed. That failing player is
excluded from the benchmark, rather than counted as a native playback win.

### History and preferences

![History comparison](images/native-ui/history.webp)
![Preferences comparison](images/native-ui/preferences.webp)

## What the measurements can establish

[Raw feature-state measurements](../experiments/native-ui/results/linux-native-features.json)
retain every trial and binary/input hashes. `native_comparison.py` compares the
expanded native executable against a release build of the unchanged Tauri source
at [2113ac2](https://github.com/joswayski/captures/commit/2113ac2cad3ab4d488409a4bfd7ad4d159e4f5ec),
with the production asset protocol enabled. The packaged Preview cannot run on
this orb's older glibc, so this is a source-build comparison, not an installer test.

Final run, 10 September 2026; medians independently recomputed from all 20 samples:

| Metric | Tauri source app | Expanded native app |
| --- | ---: | ---: |
| Preferences process-tree PSS | 481.96 MiB | 27.38 MiB |
| Preferences PSS range | 476.86–483.13 MiB | 27.23–27.50 MiB |
| Image editor process-tree PSS, same 960×540 PNG | 594.98 MiB | 26.76 MiB |
| Image editor PSS range | 507.88–598.35 MiB | 25.61–26.86 MiB |
| Preferences first mapped window | 1,115.8 ms | 128.0 ms |
| Image editor first mapped window | 1,078.5 ms | 148.5 ms |
| Preferences CPU in the two-second sampling window, one-core scale | 14.0% | Below clock resolution |
| Image editor CPU in the two-second sampling window, one-core scale | 39.5% | Below clock resolution |
| Processes in either sampled state | 6 | 1 |
| Stripped executable, excluding libraries/media tools | 34.44 MiB | 8.53 MiB |

These are roughly 94–96% lower PSS **for these partial-native states**. This is
evidence of lower frontend overhead, not proof that a feature-complete replacement
will keep that reduction. In particular, the native editor is much simpler, and
CPU after five seconds is a short settling/idle sample, not a long steady-state
measurement. A zero CPU sample means below the timer's resolution, not no work.

- Debian 12, Xvfb/Openbox/xcompmgr, 1280×800, software graphics, two virtual CPUs.
- Fresh disposable profile each time; shared PNG input; one unmeasured visual
  warmup, then five alternating trials per app/state. No builds, browser automation
  or smoke tests during measurement.
- Five seconds after the first mapped target window, sample process-tree memory;
  then sample CPU for two seconds. Timing is **first mapped window, not content
  readiness**. No new 46-ms readiness claim is made.
- PSS apportions shared pages across processes. Summed RSS double-counts shared
  libraries. WebKit child processes count; the X server, shared desktop services,
  file cache and GPU/kernel allocations do not.
- The current app includes startup-created hidden webviews and many more features.
  Its difference from this partial native app is **not a proven complete-migration
  saving**. Binary size excludes GTK/WebKit, FFmpeg, installers and other shared
  libraries. Idle/settling CPU is not battery or steady-state recording performance.
- No valid matched video-playback, capture-latency, stack-animation FPS, recording
  FPS/dropped-frame, export-throughput, cold-launch or energy benchmark is available.
  Both frontends reuse the Rust capture/media engines; removing the webview does
  not imply faster encoding. Do not extrapolate these Linux numbers to other OSes.

## Pros, cons, and the decision this supports

**Pros:** the webview-free app uses real GTK controls, directly reuses Rust media
code, and proves custom transparent stacks/particles can coexist with a native UI.
Its measured UI states have much lower process-tree memory. Three separately
maintained frontends are a valid product choice; the objection is not that AI
cannot help write them.

**Cons:** this implementation currently gives up substantial UI and interaction
parity. Standard GTK controls do not reproduce the polished editor automatically.
Custom Cairo canvases need their own hit-testing, keyboard and accessibility work.
Three frontends still need platform-specific runtime testing, packaging, permissions,
updates and a shared behavior contract, even when AI writes most of the code.
GTK3 was chosen because it shares the toolkit used by Linux Tauri; this does not
resolve the longer-term GTK4/Wayland design. A GTK4 port must revisit positioning,
capture selection and compositor integration rather than assuming GTK3 APIs carry
over. See the [original evaluation's references](native-ui-evaluation.md#authoritative-references).

**Conclusion:** custom visual effects are feasible; a no-regressions migration is
not yet demonstrated. Review the Linux visuals and gaps before proceeding with
Windows/macOS. Keep Tauri shipping. The remaining Linux work should target the
missing interaction/OS-integration rows above, then repeat measurements at matched
feature completeness on a real X11 and Wayland desktop.

## Verification and reproduction

See [DEVELOPMENT.md](../DEVELOPMENT.md#optional-native-ui-experiment) for build,
isolated-desktop, real UI-check and measurement commands.

- `npm run check`: passed (release/script tests, desktop types/lint/tests, web
  tests and production web build). Temporary-repository Git tests required
  command-local `commit.gpgsign=false`; global Git settings were not changed.
- `cargo fmt --all -- --check`: passed.
- `cargo test --workspace --release`: 366 passed, one normally ignored database
  integration test. No ignored test counted as passed.
- `cargo clippy --workspace --all-targets --release -- -D warnings`: passed.
- Standalone locked release build, formatting, 11 native tests plus two original
  probe tests, and release Clippy with warnings denied: passed.
- `native_check.py`: transparent canvas export; session-lock cancellation/retry;
  pixel-checked arrow/undo/redo/crop/PNG/JPEG; region/window/display capture;
  preview stacking/dismissal preservation; display MP4 pause/resume, region GIF,
  window recording and automatic session-lock pause/manual resume: passed.
- Actual rendered states and the dismissal recording inspected. Standard GTK
  controls expose AT-SPI names and actions used by the checks; that does not
  establish full screen-reader or custom-canvas accessibility.
- Orb setup executed twice; Python syntax, JSON structure, measurement medians
  and binary hashes checked separately.
- Not verified: actual GNOME/KDE lock screens, Wayland/multi-monitor/HiDPI,
  hardware audio/GPU, external player integration, installed packages, complete
  editor parity, screen readers, Windows or macOS. Those are not shipped claims.
