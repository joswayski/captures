# Browser-free desktop migration

Status: **cross-platform foundations in progress; renderer selection still open**.
This rewrite covers macOS, Windows, and Linux, feature by feature rather than one
complete OS at a time. AppKit and an experimental shared Rust/wgpu fixture
frontend exist; no production native capture workflow or replacement is released.
The shipping Tauri application remains available. No WebView, JavaScript runtime,
localhost server, or Tauri dependency belongs in the replacement. The website is
unaffected.

## Inventory and acceptance checklist

Inventory baseline: the desktop command registry in
`apps/desktop/src-tauri/src/lib.rs`, routes in `apps/desktop/ui/src/App.tsx`,
`ScreenshotEditor.tsx`, `Onboarding.tsx`, `Feedback.tsx`, the design harness in
`DEVELOPMENT.md`, and the root README. Checkboxes mean **accepted end to end on all
supported platforms**, not that a mock screen exists. All remain open.

| Gate | Existing behavior and required non-default cases | Source / regression oracle |
| --- | --- | --- |
| [ ] Lifecycle | Background tray/menu bar, relaunch opens Preferences, launch at login, single instance, normal shutdown vs crash recovery, session lock/inactive disables capture | `src-tauri/src/lib.rs`, `state.rs`, `crates/captures-session` |
| [ ] Onboarding | Screen/microphone permissions, deny/retry/restart, optional desktop audio, persisted completion; no permission prompts on fixture launch | `ui/src/Onboarding.tsx`, onboarding commands in `src-tauri/src/lib.rs` |
| [ ] Capture overlay | Region/window/display, empty initial selection, resize/move, aspect constraints, Shift square, Enter/Esc, auto-start, frozen/live preview, repeated shortcut captures Captures UI | `CaptureOverlay.test.tsx`, `App.tsx`, `crates/captures-capture` |
| [ ] Coordinates/color | Mixed DPI, negative display origins, display unplug, window disappears, rounded windows, cursor inclusion, macOS profile → sRGB | capture geometry/model tests; capture commands in `src-tauri/src/lib.rs` |
| [ ] Countdown | Screenshot and recording countdown, cancel while another app has focus, stale session cancellation, target revalidation | screenshot/recording countdown routes; `crates/captures-session` |
| [ ] Recording selector | Screenshot/record switch, targets, audio device choice/unplug, desktop audio and mic, cursor/click highlights, capabilities explained | `RecordingSelector.test.tsx`, `src-tauri/src/recording.rs` |
| [ ] Recording HUD | Running/paused/muted, pause/resume/restart/stop/discard, hidden controls notice, saved notice, region indicator, screenshot during recording | `RecordingHud.test.tsx`, recording routes |
| [ ] Screenshot editor | Text/font/layout/background/shadows, images, shapes/arrows/freehand, rotate/snap, crop/erase/expand, layers/order/lock, duplicate, undo/redo, pan/zoom, proportional resize, off-canvas clipping | `ScreenshotEditor.test.tsx`, `lib/screenshotEditor*.ts`, `imageBackground.ts` |
| [ ] Screenshot output | PNG/JPEG/WebP, maximum/compress and quality presets, size estimate/comparison, alpha flattening, copy/save/overwrite, draft restore/discard | `src-tauri/src/screenshot_editor.rs`, `crates/captures-image` |
| [ ] Recording editor | Playback/seek, timeline thumbnails, trim/crop/resize, audio/quality controls, size estimate/comparison, MP4/GIF/WebM export, cancel/error/retry, recover drafts | `RecordingEditor.test.tsx`, `lib/recordingEditor.ts`, `crates/captures-media` |
| [ ] Mini previews | Copy/save/reveal/trash/dismiss/open, native drag to other apps, internal self-drop shake, clear all preserves history/files, transparent hit regions | `Thumbnail.test.tsx`, `lib/thumbnail*.ts`, `styles/mini-preview.css` |
| [ ] Preview layout/effects | All four corners, pile/fan/expand, move pile, overflow, incoming capture, dust/settle, reduced motion, cancellation mid-effect, monitor/scale changes | same sources; reference PR #529 |
| [ ] Viewer/history | Empty/loading/error, 30-day retention, screenshot/video/GIF filters, restore/delete/clear, drafts, missing files, large virtualized collections | `CaptureHistory.test.tsx`, `src-tauri/src/storage.rs`, `models.rs` |
| [ ] Preferences | System/light/dark, all accent/signal themes and custom colors, settings search, shortcuts/collisions/migration, capture/recording defaults, output directory, updates | `Preferences.test.tsx`, `src-tauri/src/models.rs` |
| [ ] OS integration | Open With all six file types, multiple files, clipboard formats, native drag, reveal/trash/save dialogs, global shortcuts, Escape across focus, OS shortcut takeover | `src-tauri/src` platform adapters; README shortcuts |
| [ ] Notices/updates | Launch caret top/bottom, fixed glass vs solid update surface, full/compact notes, checking/downloading/restarting/error, signed install, drafts survive restart | startup/update routes, `src-tauri/src/updates.rs` |
| [ ] Feedback/privacy | Optional feedback, unavailable/offline/submit error, no captures attached, redacted crash diagnostics, no account needed, no new telemetry | `Feedback.tsx`, `crates/captures-feedback` |
| [ ] Accessibility/input | Keyboard-only every action, focus visible, screen-reader names/roles/value changes, text input/IME, pointer/pinch, reduced motion/contrast, 1×/2×/fractional scale | platform accessibility inspection plus interaction tests |
| [ ] Distribution | macOS signing/notarization/permissions identity, Windows installer/signing, Linux deb/AppImage/desktop entry, updater rollback and storage migration | `docs/releases.md`, existing packaging scripts |

Paths abbreviated above are under `apps/desktop` unless prefixed with `crates`.
Existing platform limitations are not new regressions: Wayland lacks window
targeting/cursor/click highlights and pointer polling; Linux cannot exclude the
recording HUD from captures. Test X11 and Wayland separately. Unsupported actions
must be explicit, not silently successful.

## Architecture and ownership

- **Rust owns domain state:** capture/recording sessions, settings migrations,
  history/artifact lifecycle, editor documents/undo, media jobs/cancellation and
  capability checks. Reuse the existing capture, image, recording, media, session,
  video and feedback crates. Do not rewrite their engines in Swift.
- **Native hosts own OS/UI:** windows, input, accessibility, text/IME, clipboard,
  drag/drop, dialogs, shortcut registration, tray and render resources. macOS starts
  with Swift/AppKit and Core Animation; use Core Image on Metal for image effects.
- Extract `models.rs` / `storage.rs` and orchestration out of `AppHandle`-coupled
  modules incrementally. Keep Tauri as an adapter to the same Rust core until
  cutover. Port pure TypeScript editor behavior with saved fixtures from its tests;
  do not embed a JS engine to reuse it.
- Proposed first binding is a narrow versioned C ABI around an in-process Rust
  static library. Specify opaque handle ownership, buffer release, error codes,
  cancellation and main-thread delivery before implementing it. No per-frame JSON
  or full-frame base64. UI receives immutable snapshots/events; media stays in
  owned native buffers/files. Benchmark copies before selecting a GPU-sharing ABI.
- `shared/design.css` and `shared/themes.css` remain the token source during
  migration. The workbench compiles resolved token resources at build time; it
  does not parse CSS or run a browser at runtime. Share assets and golden fixtures.
  Platform components may differ internally but must meet the same appearance,
  input and accessibility contracts.
- The workbench reads synthetic fixtures only. It does not touch installed
  settings/history, register shortcuts, request capture access, or install updates.
  Production data migration requires backup, version checks and rollback tests.

## Reviewable stages and exit gates

The unit of delivery is a **cross-platform feature slice**, not a finished macOS
app followed by ports. Implement domain behavior once in Rust; implement its
presentation and OS adapters on each platform. Shared behavior does not require
identical component implementations or a common UI framework.

1. **Inventory + AppKit reference (merged in [#531](https://github.com/joswayski/captures/pull/531)).**
   Shared tokens and fixture preferences/history/HUD/preview screens exist. The
   workbench runs on the maintainer's Mac and native CI passes; full visual and
   resource acceptance remains open. Mock screens are not feature parity.
2. **Cross-platform foundations (next).** Bring Windows and Linux renderer
   prototypes alongside AppKit using the same fixture scenarios below. Compare
   candidates before selecting production renderers. Make resources and scenario
   expectations platform-independent; keep backend measurement adapters separate.
   Shared-core extraction can proceed in parallel, preserving the legacy host's
   behavior, but do not build a backlog of Mac-only production features while
   other hosts lack the ability to render and exercise them.
3. **First shared feature slices.** Start with persisted appearance/preferences
   and host lifecycle, then display screenshot → preview → copy/save → history.
   Add region selection as its own slice. Each slice includes the shared Rust
   contract, all three native hosts, real engine integration, and platform checks.
   Test ownership, permission denial, errors, cancellation, session lock and DPI
   where relevant. A display-capture slice does not close the whole capture gate.
4. **Remaining workflow slices.** Work through onboarding, shortcuts, remaining
   capture modes/countdowns, preview pile/drag/effects, recording selector/HUD,
   history operations and notices. Close one narrowly defined behavior across
   platforms before treating it as complete; use the inventory for full coverage.
5. **Editor slices.** Port shared document math/persistence first, then editing
   actions and their native presentation across platforms. Start with screenshot
   editing, then recording playback/export. Differential fixtures, crash recovery
   and real media outputs gate each slice, not a mock editor shell.
6. **Cross-platform release cutover.** Packaging, updater, accessibility, energy
   and long-run tests, storage rollback, signed Preview testing. Only after parity
   is accepted remove Tauri/React desktop dependencies and the legacy frontend.
   No automatic stable release or installer replacement; the website may use React.

### PR size and platform acceptance

A small slice can fit in one PR covering all platforms. Larger slices may use a
behavior-preserving Rust extraction PR followed by focused host PRs for that same
slice. Do not duplicate domain logic in Swift or platform UI code to make one
host advance faster. Do not force unrelated OS changes into a shared-core-only PR.

Each implementation PR records the slice's behavior/non-default cases and status
for **macOS, Windows, Linux X11, and Linux Wayland**. Use explicit states:
`not implemented`, `implemented / unverified`, `verified` (with evidence), or
`unsupported` (with the existing capability limitation and visible fallback).
Mocks, stubs, compilation, and missing hardware are not functional acceptance.
When host work is split across PRs, link the companion work and keep the slice
open until its platform gates pass. Never silently drop an OS to close a gate.

Run platform compilation/tests in CI where available. Maintainer runs on Mac and
Windows supply real desktop/input/GPU evidence; Linux evidence must distinguish
X11 from Wayland and hardware from the orb's graphics environment. Each handoff
includes exact commands, expected behavior, captures and raw measurement output.
Hardware results pending need not block unrelated shared work, but must remain
visible and cannot justify a renderer selection or performance claim.

## Windows and Linux evaluation plan

No renderer is selected for these platforms yet. The same fixture scenes, token
resources, resource budgets, visual checkpoints and input scripts are mandatory.

The first domain slice now extracts shipping settings types/defaults/migrations
and persistence into `captures-settings`, with a versioned `captures-settings-ffi`
static library for AppKit. Both native Preferences screens edit a separate
development settings file. Shared custom-theme derivation is checked against
TypeScript-generated golden values. This advances settings persistence and
presentation, not lifecycle/capture integration or full Preferences acceptance;
all checklist gates above remain open until end-to-end verification.

The next shared-core slice moves history metadata, 30-day retention, atomic
artifact replacement, recording recovery, and basic sRGB PNG/thumbnail encoding
into `captures-history`. The shipping desktop delegates to it; callers provide
their own history root and presentation URLs. Native capture integration can use
the same lifecycle without accessing installed history. This extraction alone
adds no native capture UI and closes no platform gate.

The opt-in `--live` workspace now connects full-display PNG capture and local
screenshot history on both native hosts through `captures-app`. It includes
explicit copy, export, reveal and history deletion while keeping exports and the
installed Preview's data separate. Image decode and capture/file operations run
off the UI thread. It preserves permission/session checks and hides its window
before capture. Wayland capture remains gated by the candidate's missing window
visibility support. Capture preferences, regions/windows, recording, editor,
mini previews and full UI/UX parity are still open; this slice closes no complete
platform acceptance row. Hardware capture and clipboard tests remain required.

The first [shared wgpu candidate](../apps/native/wgpu/README.md) uses egui/winit
with retained image textures and event-driven immediate-mode UI, an additional
approach to evaluate against the retained/native candidates below. It implements
fixture preferences/history/HUD, a fade/settle probe, editor rendering and hidden
idle. It does **not** implement dust parity, production domain logic or complete
accessibility/IME/overlay behavior. No performance win or renderer decision follows
from its existence. Compare equivalent workloads only; the AppKit dust and wgpu
fade/settle probes are deliberately not equivalent effects.

The next implementation milestone is **comparable workbenches on all OSes**, not
the next Mac-only screen. Exercise each candidate with:

- Preferences: Captures-styled controls, light/dark/themes, editable search text,
  keyboard focus and scrolling; expose accessibility roles and values.
- History: empty, 100 and 1,000 image-backed rows, filtering/scrolling, bounded
  thumbnail residency and release after closing.
- A transparent desktop preview and HUD: hover/hit regions, running/paused/hidden
  states, cold/warm dust and survivor settle, cancellation and reduced motion.
- An editor rendering probe: large image, multiple layers, pan/zoom/rotate and
  editable text/IME. This tests renderer suitability, not editor feature parity.
- Lifecycle: hidden/minimized/occluded idle, mixed/fractional DPI, repeated
  open/close and resource recovery. No recurring redraw loop for static scenes.

Extend the AppKit workbench to these same cases where it is incomplete. Reuse
tokens/assets and deterministic effect fixtures; do not translate the entire app
into each candidate just to evaluate it. Preserve custom Captures styling and
compare equivalent work, including setup costs, rather than native stock widgets
against fully styled screens. Record missing input/accessibility support as a
candidate cost, not as a task deferred until after renderer selection.

| Platform | Candidates | Questions the prototype must settle |
| --- | --- | --- |
| Windows 11 | Win32 host + DirectComposition/Direct2D/DirectWrite; shared Rust retained scene renderer using wgpu + native text/accessibility adapters | Idle wakeups, GPU allocations, composition-only motion vs texture uploads, custom controls, UI Automation/IME, transparent click-through windows, mixed DPI, device loss |
| Linux X11 + Wayland | GTK4 host/custom snapshot nodes (not default widget styling); shared Rust wgpu renderer with winit/Wayland/X11 host and AccessKit | Fractional scale, text quality/IME, AT-SPI, transparent overlays, compositor/frame callbacks, portal permissions, tray support, occlusion, integrated GPU/software fallback |

GPUI remains a comparator only: its tested CPU-raster/texture-upload dust path was
expensive; that result does not disqualify every GPU implementation of the effect.
Use ordinary custom Captures controls, not stock GTK/WinUI visual styling. Prefer
shared components only after representative screens meet parity and resource
gates. If one scene needs platform rendering, isolate that component rather than
forcing all screens into the same renderer. Record toolchain, dependency/license,
binary size and accessibility cost with the performance decision.

## Measurement protocol

The prior [AppKit experiment](https://github.com/joswayski/captures/pull/529)
passed 24/24 dust/settle checkpoints. Its AC-run dust medians were 54 MiB / 8.0%
of one CPU core / 97.5 observed changed frames/s, versus 196 MiB / 27.5% / 40.9 for
Tauri. AppKit preparation was about 132 ms. These are **component results**, not
whole-app budgets, GPU presentation timestamps, or proof this workbench is faster.

Run release builds, same machine, power mode, scale and refresh rate. Separate
resource trials from screenshots/frame observation. One discarded warmup and at
least three measured trials, rotating renderer order. Keep raw results and build
identity. Compare equivalent content and functionality; a mock screen versus a
full application is not a valid whole-app performance comparison.

| Workload | Required observations |
| --- | --- |
| Cold launch → first usable screen | Process-tree physical footprint, startup latency, first input latency |
| No windows; Preferences idle; HUD idle; history idle | 60-second CPU time, wakeups, memory; no continuous display link or polling for static screens |
| Preferences | Search/type, focus, appearance/theme switching, scroll; input p50/p95, text and accessibility parity |
| History | Empty / 100 / 1,000 entries, scroll/filter/open; image cache residency and eviction |
| Editor | Large image + multiple layers, pan/zoom/rotate, text input; recording playback/seek/export in later stages |
| Transparent preview | Hover controls, pile expand/move, new captures, self-drop shake, cold/new-image dust, warm repeat dust, survivor settle, reduced motion |
| Recording | Running/paused/hidden HUD and indicator, mic changes, real recording overhead separated from UI |
| Lifecycle | Hidden/minimized/occluded, screen sleep/wake, 1×/2× display changes, 30-minute churn, memory after close/GC/resource release |

Measure preparation in distinct phases: decode, cover crop, atlas filtering,
layer construction, animation submission, and total first-action latency. Moving
work before a timer or prewarming does not remove its cost. The workbench batches
isolated padded chips into one filter atlas to reduce repeated Core Image renders;
this is an **unverified optimization candidate** until the original visual gate
passes and cold setup improves. Do not blur a whole source image and then slice:
that changes chip edges. Do not hide preparation in an unbounded cache.

Acceptance targets to validate on hardware: no application-owned recurring frame
callbacks when static/hidden; UI-only idle CPU median below 0.5% of one core;
p95 ordinary input response below 50 ms; animation work within the display budget
(16.7 ms at 60 Hz / 8.3 ms at 120 Hz); no sustained memory growth after repeated
open/close cycles. Cold dust preparation target is under 16.7 ms, with a responsive
fallback if it cannot meet the deadline. These are targets, not measured claims.
Preserve physical-footprint/process-tree measurement from #529 for comparisons;
the initial runner's `ps` RSS is only a diagnostic, never a substitute.
