# Native UI evaluation

This is an experimental feasibility study, not a replacement for Captures Preview
or a commitment to ship a native frontend. The shipping application is unchanged.

## What “native” would change

Captures is already a compiled Rust desktop application using native capture APIs.
Tauri hosts the React UI in the system webview; it is not an Electron bundle with
its own Chromium. macOS recording already uses ScreenCaptureKit/VideoToolbox;
Windows/Linux already use xcap/OpenH264. Removing the webview does **not** replace
those engines or automatically accelerate recording or encoding.

There are three different goals:

1. **Actual platform UI:** AppKit/SwiftUI on macOS, WinUI on Windows, GTK on Linux.
2. **No webview, shared Rust UI:** egui or Slint; usually custom-rendered or
   platform-styled controls, not actual AppKit/WinUI/GTK widget instances.
3. **Better desktop integration:** retain Tauri and use native windows/platform
   APIs selectively. This does not require rewriting the entire frontend.

## What was implemented

[`experiments/native-ui`](../experiments/native-ui) is a standalone, non-shipping
Rust workspace with two release executables:

- `captures-gtk-probe`: actual Linux GTK3 labels, buttons, and image widget.
- `captures-tauri-probe`: a minimal Tauri 2 webview with equivalent controls.

Both call the **same existing `captures-capture::XcapBackend`** to capture the
primary/default display, encode PNG using the same Rust function, show a scaled
preview, and save the exact PNG bytes. Both deliberately include their own window
in the screenshot. Capture work runs off the UI thread. Neither changes production
settings/history or registers global shortcuts. Save creates a new file rather
than overwriting an existing capture. The default destination is the temporary
directory's `captures-ui-probe` subdirectory, not the production capture library.

This proves that the existing capture crate can power a real OS-widget frontend
without Tauri command handlers. It does **not** implement region/window selection,
recording, clipboard, annotations, history, preferences, permissions onboarding,
session-lock gating, tray, updater, or installers. The GTK frontend is explicitly Linux-only. The Tauri
control is vanilla JavaScript, not the production React UI, and uses a PNG data URL
rather than production's custom media protocol.

GTK3 was selected to isolate widget-vs-webview costs on the same toolkit already
required by Linux Tauri. It is **not the recommendation for a new long-lived Linux
frontend**; evaluate GTK4 separately. GTK uses its OS theme, and the web control
uses basic browser controls with similar geometry, not a new Captures design.

## Measured Linux results — 10 September 2026

Source baseline: [2113ac2](https://github.com/joswayski/captures/commit/2113ac2cad3ab4d488409a4bfd7ad4d159e4f5ec).
Debian 12 orb, x86-64, two virtual Xeon CPUs, approximately 4 GiB RAM,
Xvfb 1280×800, `LIBGL_ALWAYS_SOFTWARE=1`, GTK 3.24.38, WebKitGTK 2.50.6,
Rust 1.94.0, Tauri 2.11.5, xcap 0.9.6. Both probes use optimized, stripped release
builds; the Tauri probe explicitly enables the production `custom-protocol` feature.
No builds or other CPU-heavy checks ran during these samples.

Ten measured warm launches per probe, alternating order, after one discarded
warmup each. The idle window has **no screenshot loaded**. Memory is sampled two
seconds after readiness; idle CPU is sampled over the following three seconds.

| Metric | Native GTK prototype | Matched minimal Tauri control |
| --- | ---: | ---: |
| Approximate UI readiness, median | 46.3 ms | 375.8 ms |
| Readiness range | 43.1–52.2 ms | 344.5–440.5 ms |
| Idle process-tree PSS, median | 18.58 MiB | 253.45 MiB |
| PSS range | 18.50–18.65 MiB | 253.19–253.91 MiB |
| Summed RSS, median (shared pages counted repeatedly) | 25.32 MiB | 421.04 MiB |
| Private memory, median | 16.36 MiB | 168.93 MiB |
| Processes | 1 | 3 |
| Idle CPU | Below clock resolution | Below clock resolution |
| Stripped executable, excluding shared libraries | 5.38 MiB | 11.20 MiB |

GTK used approximately **93% less idle PSS in this minimal-window experiment**.
That supports a Linux UI-overhead advantage, not a prediction for a complete
native Captures implementation. Do not turn the readiness ratio into a claimed
first-presented-frame speedup: the two readiness milestones differ as described
below. Neither the CPU samples nor this test establish an energy or recorder gain.

[All paired samples, summaries, versions, and binary hashes](../experiments/native-ui/results/linux-x11.json)
are retained. The medians were independently recomputed from the twenty samples.
Only an external DBus accessibility-daemon stdout preamble was removed from the
JSON export; no measurement bytes were changed. The reproduction commands redirect
inside the DBus session to avoid that preamble.

### Current app reference — a different, much fuller workload

A local release build of the **unchanged current app source** was also run with
its embedded React UI, Preferences open, empty history, and the startup-created
hidden capture webviews. Its rendered screenshot was inspected to confirm actual
Appearance/Capture settings and navigation, not a connection-error page.

Three independent disposable profiles; ten seconds to settle after Preferences
becomes visible, then five seconds of idle CPU sampling. Outbound HTTP uses an
unavailable proxy to prevent remote update fetches; failed connection checks
remain part of this measured app state.

| Metric | Current app Preferences, median |
| --- | ---: |
| Process-tree PSS | 490.32 MiB (range 490.17–491.56 MiB) |
| Summed RSS | 1,064.89 MiB |
| Private memory | 355.29 MiB |
| Processes | 6 |
| Idle CPU, one-core scale | 0.80% (range 0.80–1.20%) |

[Current-app samples and binary hash](../experiments/native-ui/results/current-app-linux-x11.json).
This reference includes far more functionality and windows than either probe.
**Its difference from GTK is not a measured migration saving.** It is not hidden-tray
idle, recording, a packaged-Preview benchmark, or a macOS/Windows result.

## Benchmark scope and interpretation

The paired benchmark measures warm startup readiness, idle process-tree memory,
idle CPU, process count, and stripped executable size. Both release binaries are
built together, share dependency resolution and the same screenshot backend, and
run sequentially on the same X11 display. One launch per executable is discarded
as warmup; measured order alternates to reduce ordering bias.

- **Readiness** is process launch to a signal after GTK's first draw callback and
  subsequent idle callback, or the webview's double `requestAnimationFrame` plus
  a Rust command. These are approximate UI-ready milestones, **not identical
  compositor presentation timestamps**. The web measurement includes IPC.
- **PSS** apportions shared pages; it is the main memory comparison. Summed RSS
  double-counts shared libraries. Private memory is also retained in the raw data.
- All descendant processes, including WebKit's web/network processes, are counted.
  The shared X server, DBus services, file cache, and kernel/GPU allocations are
  not counted. Unreadable child memory fails the measurement rather than silently
  counting as zero.
- Idle CPU is elapsed CPU time over wall time, relative to one logical core. A
  zero sample means below the process CPU-clock resolution, not zero energy use.
- Executable size excludes system GTK/WebKit libraries, installers, and FFmpeg.
  Tauri already relies on a system webview, so binary size is not installation size.
- The startup/idle benchmark does not exercise screenshot transfer, capture
  latency, editing, multi-window memory, recording FPS, power, or accessibility.
  The UI capture/save smoke test is functional verification, not a throughput test.
- Xvfb/software graphics in a small Linux orb cannot predict GPU-backed desktops,
  GTK4, Wayland, WKWebView/AppKit, or WebView2/WinUI results.

The actual Preview `.deb` for the source commit was also downloaded and extracted
without installation. Its binary requires `GLIBC_2.39`; this Debian 12 orb has
glibc 2.36, so it cannot launch here. The current-app reference above therefore
uses a local source build, not that package. An initial direct-Cargo build without
`tauri/custom-protocol` displayed a development-URL error; visual inspection caught
it, and that invalid run was excluded. The corrected build embeds the production
UI. No production app source change was needed to run it.

## Verification

- `npm run check`: passed (80 script tests, 830 desktop tests, 34 web tests,
  desktop typecheck/lint, production web build). Temporary-repository Git tests
  required command-local `commit.gpgsign=false` because the orb had no signing key
  for those temporary repositories; no global Git configuration was changed.
- `cargo fmt --all -- --check`: passed.
- `cargo test --workspace --release`: 366 passed, one database integration test
  ignored by the normal suite. No ignored integration test was silently counted
  as passed.
- `cargo clippy --workspace --all-targets --release -- -D warnings`: passed.
- Standalone experiment: locked release build, release tests (two passed),
  formatting, and release Clippy with warnings denied: passed.
- Real X11 UI smoke test: both frontends captured a nonblank 1280×800 display PNG,
  saved identical bytes twice without overwriting, surfaced an invalid-destination
  error, and saved the retained capture after destination recovery. Empty,
  captured/saved, and error screenshots were inspected; final side-by-side render
  was also inspected.
- Process-tree sampler sanity check included a child process's 64 MiB allocation
  rather than counting only its parent. The GTK binary's dynamic dependencies
  include GTK, not WebKit/JavaScriptCore.
- Orb setup ran twice successfully; Python scripts compiled, both raw JSON reports
  parsed, and report medians/binary hashes were checked.
- Not verified: native macOS/Windows frontends (not implemented), GTK4/Wayland,
  hardware-GPU behavior, screen readers, recording/export performance, battery,
  installed packages, or full native feature parity.

## Options and tradeoffs

| Approach | Benefits | Costs and limitations |
| --- | --- | --- |
| Keep Tauri + React | Existing UI/editors, one frontend, Rust backend, working plugins and release pipeline; system webview rather than bundled browser | Webview processes, DOM/IPC overhead, differing platform web engines; controls are web controls |
| SwiftUI + AppKit, sharing Rust | macOS controls/conventions, accessibility defaults, direct window/menu integration; reuses current native recorder | New Swift/Xcode frontend and FFI boundary; macOS only; AppKit still needed for specialized windows; must measure on a Mac |
| WinUI 3 + Rust | Windows Fluent controls, UI Automation, native Windows integration | Official frontend languages are C#/C++; interop and UI-thread/COM ownership; separate Windows frontend, packaging/runtime decisions |
| GTK4 + Rust | Native GTK controls, Rust bindings, AT-SPI accessibility | Linux/desktop-specific conventions; not KDE widgets; GTK4/Wayland overlays require separate investigation; GTK3 probe results do not measure GTK4 |
| egui or Slint | Rust-oriented shared UI without a webview; direct shared-core integration | Usually custom-rendered/emulated controls, not OS widgets; editor/text/accessibility and platform behavior need verification; still need tray/updater/packaging integration |
| Hybrid by surface | Preserve mature editors/history/preferences while trying native surfaces with specific UX or performance problems | Two UI systems and window lifecycles; webview savings may be limited if web windows remain resident |

Standard native controls supply useful accessibility defaults, but custom native
canvases still need semantics and keyboard support. Conversely, an accessible DOM
or AccessKit-backed custom UI can expose native accessibility information without
being a native widget. Native appearance, native controls, and accessibility are
three separate claims.

## What can be reused, and what needs extraction

- `crates/captures-capture`: display/window discovery, screenshot buffers, crop,
  geometry, and cursor composition. The prototype consumes it directly.
- `crates/captures-recording*`, `captures-media`, `captures-video`, and
  `captures-session`: reusable recording/media/session logic and platform engines.
- `apps/desktop/src-tauri/src/state.rs`, `lib.rs`, `storage.rs`, and
  `screenshot_editor.rs`: application state, commands, persistence, and editor
  orchestration need a Tauri-independent interface; they are not all already a
  reusable core library.
- `crates/captures-macos-window`: explicitly depends on Tauri/WebviewWindow;
  specialized platform-window behavior needs adaptation, not blind reuse.
- React `App.tsx`, `ScreenshotEditor.tsx`, and associated editor components hold
  substantial interaction logic; these are a frontend rewrite, not a skin change.

A fully native replacement must own window positioning/focus/transparency,
multi-monitor/DPI behavior, capture exclusion, tray/status items, shortcut
conflicts, clipboard/drag-and-drop, dialogs, settings, permissions, image and video
preview, asynchronous progress/cancellation, accessibility, single-instance/open
file behavior, autostart, diagnostics, signed updates, and packaging on each OS.
Retaining FFmpeg/media crates also retains their distribution and licensing work.

## Recommendation

**Keep Tauri as the shipping application while testing a macOS-first native
vertical slice.** macOS is the primary development target, and the reason to
choose AppKit/SwiftUI should include actual macOS behavior, not just Linux memory.
The next useful slice is capture selection → screenshot → preview/save, sharing
the Rust backend, measured against the current app on the same Mac. Native overlays
must prove focus, menu-bar behavior, display scaling, capture exclusion, and
permissions, not only show an empty settings window.

Also measure a smaller Tauri optimization before committing to a rewrite:
`apps/desktop/src-tauri/src/lib.rs` eagerly creates the capture overlay, recording selector,
and thumbnail webviews during setup. Deferring some of those windows could trade
lower idle memory for higher first-capture latency. That tradeoff is a hypothesis
to benchmark, not an optimization implemented or a saving established here.

Extract a small application-service boundary only as that slice needs it. For a
Swift frontend, use a versioned C ABI/static library or a reviewed binding tool;
specify buffer allocation/freeing, errors, callbacks, main-thread delivery,
cancellation, and shutdown. Do not expose Rust structs or borrowed buffers across
FFI without an ownership contract. Keep the Tauri adapter working during migration.

Before committing to a rewrite, benchmark on macOS, Windows, and representative
Linux X11/Wayland machines: cold and warm launch; hidden tray idle; overlay open;
several previews/editors; 4K capture click-to-preview; video playback; recording
dropped frames/CPU/GPU; export throughput; and battery/energy. Use identical inputs
and settings, release builds, process-tree memory, multiple runs, and artifact
hashes. Verify keyboard/screen-reader workflows separately from performance.

Only retire Tauri after feature, accessibility, platform, and signed-update parity.
No performance target or migration completion date is promised by this experiment.

## Authoritative references

- [Tauri architecture](https://v2.tauri.app/concept/architecture/): system webviews,
  Rust host, and message passing.
- [Apple AppKit](https://developer.apple.com/documentation/appkit) and
  [SwiftUI accessibility](https://developer.apple.com/documentation/swiftui/accessibility-fundamentals):
  framework responsibilities, interoperability, standard and custom view semantics.
- [Rust FFI](https://doc.rust-lang.org/nomicon/ffi.html): C calling conventions,
  static/dynamic libraries, ownership, and asynchronous callback hazards.
- [WinUI 3](https://learn.microsoft.com/en-us/windows/apps/winui/winui3/) and
  [Windows accessibility](https://learn.microsoft.com/en-us/windows/apps/develop/accessibility).
- [GTK4 migration](https://docs.gtk.org/gtk4/migrating-3to4.html) and
  [GTK accessibility](https://docs.gtk.org/gtk4/section-accessibility.html):
  GTK4 changes include removed global-coordinate/window-positioning APIs.
- [egui](https://github.com/emilk/egui) and
  [its accessibility integration](https://github.com/emilk/egui/blob/main/docs/accessibility.md).
- [Slint styles](https://docs.slint.dev/latest/docs/slint/reference/std-widgets/style) and
  [backends/renderers](https://docs.slint.dev/latest/docs/slint/guide/backends-and-renderers/backends_and_renderers):
  “native” style aliases are not a guarantee of platform widget instances.
- Tauri [updater](https://v2.tauri.app/plugin/updater/),
  [tray](https://v2.tauri.app/learn/system-tray/), and
  [global shortcuts](https://v2.tauri.app/plugin/global-shortcut/): responsibilities
  to retain or replace.
