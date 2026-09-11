# Native experiment parity audit

Reviewed September 11, 2026, against the shipping Tauri source at the merge of
[#514](https://github.com/joswayski/captures/pull/514). This is a source audit with
targeted regression checks, **not certification of every feature or OS**. Keep
the Tauri Preview as the distributed app. None of these fixes changes its bundle,
profile, release channel, or backend.

## Changes reviewed

| PR | Scope and evidence limits |
| --- | --- |
| [#512](https://github.com/joswayski/captures/pull/512) | Expanded Linux GTK3/Cairo frontend. Real isolated X11 interaction checks and matched Preferences/editor measurements; not Wayland or physical desktop certification. |
| [#515](https://github.com/joswayski/captures/pull/515) | Windows port of that GTK frontend. Cross-build and Wine checks, not real Windows capture, audio, DPI, or performance evidence. |
| [#514](https://github.com/joswayski/captures/pull/514) | Separate SwiftUI/AppKit frontend and Rust bridge. Apple Silicon CI compilation/editor tests and Preferences renders, not a physical-Mac capture/permissions test or benchmark. |
| [#509](https://github.com/joswayski/captures/pull/509), [#510](https://github.com/joswayski/captures/pull/510) | Tauri preview drag/dust performance changes. Native previews have their own implementations; changes to React/CSS do not automatically update either native frontend. |
| [#506](https://github.com/joswayski/captures/pull/506) | Shipping Open With support is an important parity baseline: opening a path from the CLI is not installed OS file association support. |

The [Linux](linux-native-implementation.md), [Windows](windows-native-implementation.md),
and [macOS](macos-native-implementation.md) reports remain the detailed historical
implementation/measurement records. This audit consolidates their open gaps and
distinguishes newly confirmed defects from untested behavior.

## Defects corrected in this follow-up

The GTK recording settings also allowed **any video FPS from 1 through 60**,
but `crates/captures-recording/src/model.rs::RecordingOptions::validate` accepts
only 15, 30, or 60 for video. Preferences now offers exactly those values. Old
values round up to the next supported rate (24 → 30, 31 → 60); valid choices and
independent GIF rates remain unchanged. Regression tests cover both sides of
each boundary, rejection outside the old range, and actual GTK selection and
persistence. The shipping Tauri and macOS-native pickers already restrict video
FPS to these rates.

| OS | Confirmed problem and source | Correction and regression coverage |
| --- | --- | --- |
| Linux + Windows | `native-ui/src/native/preferences.rs` offered WebM, `settings.rs` accepted it, but `recording.rs::recording_output_format` rejects it. Saving this preference makes ordinary recording fail. | Offer MP4/GIF only; load old `webm` settings as MP4 without resetting other choices; reject new unsupported settings. Rust migration test plus actual GTK format selection/persistence check. WebM encoding itself is still absent. |
| Windows | `native-ui/src/native/settings.rs` inherited Linux's New Capture, display screenshot, and region recording keys, differing from `apps/desktop/src-tauri/src/models.rs`. | Defaults now use Ctrl+Shift+Space, PrintScreen, and Win+Alt+R. Migrate only the complete inherited factory set, and only if the replacement has no duplicate bindings. Preserve partial customizations and all Linux defaults. Pure migration tests also execute on Linux; actual Windows registration remains unverified. |
| Linux | `recording.rs::RegionBorder::for_target` treated a display-local region as desktop-global. The guide appeared on the wrong monitor when its origin was not zero. | Add the selected display's logical origin to all four edges. Test asymmetric rectangles and a negative-x/nonzero-y monitor origin. Preserve Windows' separate physical-coordinate/DPI placement. Physical multi-monitor rendering remains unverified. |
| macOS | Preferences enumerated microphones through screen discovery but never requested microphone authorization; the bridge started selected inputs without Tauri's explicit permission preflight (`src-tauri/src/lib.rs`). | Independent microphone discovery/authorization, an explicit first-time permission action, denied-state System Settings link, and start-time checking of persisted selections. Permission work stays off the recording worker and Swift control queue. Portable permission-decision/routing tests; actual TCC requires a Mac. |
| macOS | `Models.swift::Shortcut.defaults` and `Shortcuts.swift` omitted shipping Cmd+Shift+6 GIF capture. | Add GIF region routing and migrate missing shortcut actions while preserving customized bindings. Portable Swift tests cover routing, migration, and repeated migration. Carbon conflicts are still reported rather than changing OS shortcuts. |
| Documentation | `DEVELOPMENT.md` still said no macOS native frontend existed after #514 merged. | Point to the implemented, separately packaged macOS experiment. |

Paths beginning `native-ui/` above are relative to `experiments/`; Swift paths
are under `experiments/macos-native/Sources/CapturesNative`.

## Functional and distribution gaps still open

“Missing” means an absent or deliberately restricted code path, not merely lack
of hardware evidence. These remain replacement blockers after the fixes above.

| Area | Linux GTK | Windows GTK | macOS SwiftUI |
| --- | --- | --- | --- |
| Desktop capture | X11 only; startup rejects Wayland. Shipping Tauri has a limited Wayland path. | Mixed-DPI and negative-origin handling is implemented, not hardware-certified. | Frozen window screenshots crop the visible display, including occluding windows, instead of unobscured window contents. |
| Capture interactions | Existing presets/selector implemented; full input parity still needs review. | Inherits GTK selector behavior. | No region aspect presets or persistent recording-region guide. Repeated capture shortcuts recreate the selector instead of capturing/switching the existing overlay as Tauri does. |
| Recording controls | No live microphone peak measurement; hide controls manually because reliable capture exclusion is unavailable. | Same peak-meter gap. Exclusion code exists, but requires actual Windows testing. | **Restart still begins immediately** despite the configured countdown and confirmation text. `CaptureOverlay.swift::restart` calls `record_restart`, whose bridge implementation immediately calls `begin_segment`; initial countdown belongs to the Swift selector. Must implement a cancellable restart countdown with lock/cancel tests before claiming parity. |
| Recording editor | New-file export only, not source overwrite. Compression comparison is explicit rather than automatic on each relevant edit. | Same GTK limitations. | No persisted recording-edit drafts or live encoded-size estimate. GIF comparison is a decoded still, not synchronized animation. |
| Formats | No native WebM export, despite shipping Tauri's broader export path. | Same native WebM gap. | No WebM export; AVPlayer WebM decoding depends on OS/codec. PNG maximum-size requests use lossless encoding rather than palette quantization and may fail at a size Tauri can meet. |
| Image editor | Secondary layer properties remain in a scrolling sidebar, not all Tauri per-layer menus. | Same GTK differences. | Missing color-wand background removal, text/brush shadow controls, rotation handle/snap, and some pan/zoom and off-canvas expansion affordances. |
| Preview motion/input | GTK effects and four-corner snapping are not browser-identical gravity/sway/filters. | Same effects; transparent margins may intercept clicks because GTK Win32 lacks the X11 input-shape API. | Custom AppKit stack/dust implementation; visual and animation equivalence unverified. |
| History/preferences | Smaller independent metadata model; not complete archived snapshots or every shipping preference. | Same GTK model. | Separate history/settings; not a production-profile migration. |
| OS shortcuts | Does not automatically clear GNOME/KDE screenshot bindings as Tauri does. Resolve desktop conflicts explicitly. | Win+Shift+S interception exists; Print Screen/Snipping Tool conflict handling and takeover need Windows review. | Never automatically unbind Apple's Screenshot shortcuts, unlike Tauri. Conflicts are intentionally visible. |
| Packaging/integration | No native installer, updater, file associations, or shipping onboarding. | Portable folder only: no installer, updater, signing, or Open With registration. Redistribution license/source audit is incomplete. | Ad-hoc-signed local-test bundle, not notarized Preview distribution. No updater, feedback/crash reporting, launch/update notices, or shipping setup walkthrough. |
| Media dependencies | System FFmpeg/ffprobe/ffplay on PATH, unlike packaged Tauri sidecars. | Portable packager supplies media tools and GTK DLLs; source runs need the documented MSYS2 environment. | Prepared sidecars or local FFmpeg/ffprobe required; the existing CI ZIP does not prepare bundled sidecars. |
| Recording annotations | Backend/platform limitations still apply; do not infer features from shared settings alone. | Backend/platform limitations still apply. | Click highlights and keystroke overlays are not enabled. |

Two additional risks need focused follow-up:

- Windows media helpers are launched with ordinary `Command::new` in native
  preview/playback/editor paths and `crates/captures-media/src/toolchain.rs`.
  Console-subsystem FFmpeg tools may flash console windows from the GUI app.
  Only the GTK loader helper currently specifies `CREATE_NO_WINDOW`. Confirm on
  Windows and apply no-console flags consistently, including shared media code;
  this may also affect Tauri, so it is not established as a native-only regression.
- The isolated workspaces do not participate in `npm run check` or root
  `cargo test --workspace`. The macOS PR workflow checks its experiment; root
  gates alone do not certify either native app. Run the standalone gates too.

No automatic production-profile migration should be added casually: separate
data, identity, permissions and startup registrations let these experiments
coexist safely. Promotion needs an explicit migration/recovery plan.

## Required OS acceptance before replacement

- **Linux:** GNOME and KDE; X11 multi-monitor/HiDPI with negative and nonzero
  origins; tray panels, global conflicts, lock/logout/resume; real Pulse/PipeWire
  microphone and monitor sources; long recordings and audio sync. Wayland needs
  a separate implementation, not a removed startup guard.
- **Windows 11:** actual x64 app outside MSYS2, including paths with spaces;
  clean portable extraction and relocation; repeated settings/draft saves;
  100/150/200% monitors and negative origins; crop pixels, cursor and exclusion;
  transparent-area click-through, clipboard/file drag/Open With expectations;
  tray, login startup, Print Screen and Win+Shift+S conflicts; lock/disconnect,
  audio and long recording sync. Wine is not sufficient evidence.
- **macOS:** fresh and denied Screen Recording/microphone TCC states; reopen
  after granting permissions; persisted microphone with revoked permission;
  GIF shortcut defaults/customization; Retina and multiple displays; lock during
  selection/recording; audio pause/mute/restart/recovery; login items and Finder
  Open With. Test supported runtime versions and Intel separately from Apple
  Silicon macOS-26 CI. Test restart countdown specifically; it is still open.
- **All:** light/dark/system appearance, keyboard-only access, screen readers,
  reduced motion, pointer hit targets, preview drag/delete/collapse, and a
  side-by-side capture/editor/recording workflow using the same files. Inspect
  rendered states; matching tokens or successful compilation is not UI parity.

The Linux reports show lower process-tree memory and faster first-window mapping
for specific workloads. They do **not** establish capture/encoding throughput,
energy savings, complete-application readiness, or macOS/Windows performance.
Re-run matched measurements after closing feature gaps, with no builds or test
automation running concurrently. Do not trade missing work for a performance win.

## Follow-up validation in the Linux orb

- `npm run check`: passed (80 release/script tests, 830 desktop tests, 34 web
  tests, typechecking/lint and production web build). Disposable Git fixtures
  used command-local `commit.gpgsign=false`; no global Git setting changed.
- `cargo fmt --all -- --check`, `cargo test --workspace`, and
  `cargo clippy --workspace --all-targets -- -D warnings`: passed; 366 tests
  passed and one database integration test remained normally ignored.
- Native GTK `cargo test --locked --manifest-path experiments/native-ui/Cargo.toml
  --no-default-features --features native --bin captures-linux-native`: 58 passed.
  Native-only build, standalone formatting and all-targets Clippy with denied
  warnings passed. These used the development profile, not a new release benchmark.
- `native_parity_check.py` in the isolated X11/DBus desktop passed: exact
  screenshot pixels, region/window/display capture, lock rejection, global
  shortcut/second-instance routing, Preferences and history. New AT-SPI checks
  enumerate exactly MP4/GIF and 15/30/60-fps choices and check actual persisted
  changes after loading old WebM/24-fps settings. MP4/GIF renders were inspected.
  The harness's local `target/release/captures-linux-native` path pointed to the
  development binary for this run; no optimized performance result is implied.
- A separate single-display recording check waited for enabled controls and
  `00:01`, asserted all four X11 guide-window bounds for a 360×260 region at
  (720,120), inspected the render, and confirmed deletion closed the HUD. This
  does not verify negative-origin hardware, audio, or long recordings.
- `SWIFTC=/path/to/swiftc bash experiments/macos-native/check.sh`: passed with
  official Linux Swift 6.2.4. Covers Swift syntax/Foundation models, four Python
  tests, 17 Rust bridge tests, bridge formatting and release Clippy. Does **not**
  typecheck AppKit/SwiftUI, link Apple frameworks or run the macOS editor tests.
- Desktop release-scope helper: `shouldRelease: false`. No Preview packaging,
  production profile migration, deployment, or OS permission reset was performed.

Windows build/runtime and actual macOS UI/TCC were not run in this orb. The
hardware acceptance list above remains open; these results do not replace it.
