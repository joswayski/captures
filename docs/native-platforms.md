# Native platform migration

Captures is moving toward platform-specific native presentation around the
existing Rust capture, recording, media, and session engines. The shipping Tauri
Preview remains the comparison baseline and distributed app until the native
frontends pass their replacement checks. This work does not change installers,
update channels, production profiles, or OS permissions.

## Presentation choices

| Platform | Native presentation | Implementation location |
| --- | --- | --- |
| Linux | Rust, GTK4, custom Captures styling and rendering | `experiments/linux-native` |
| Windows | Rust, Win32, DirectComposition, Direct3D/Direct2D/DirectWrite | `experiments/windows-native` |
| macOS | Swift, AppKit/SwiftUI, Core Animation; shared Rust bridge | `experiments/macos-native` |

The earlier `experiments/native-ui` GTK3 frontend remains a historical baseline,
not the new Windows implementation. GPUI and Qt Quick are not part of this
migration. Native controls do not define the product's appearance: app-owned
menus, confirmations, sliders, and previews must follow the same Captures design.
OS permission prompts and system file pickers remain OS-owned.

## Visual and behavioral replacement checks

The source of truth is `apps/desktop/ui/src`, its component styles, and the tokens
in `shared/design.css` and `shared/themes.css`. Compare actual rendered states,
not screenshots substituted into a native window. The historical
[parity audit](native-parity-audit.md) records known gaps in the earlier prototypes;
it is not a certification of the new implementations.

- Capture: screenshot/recording selection, region presets, compact/expanded
  menus, cursor/audio toggles, highlighted inclusion warning, countdown, and
  lock cancellation.
- Screenshot editor: the tool rail, flyouts, layer/property controls, custom
  color picker, cropping/rotation, compression comparison, and export footer,
  including the Save as new file toggle. Check light/dark and narrow windows.
- Previews: collapsed/expanded stacks, corner/gravity behavior, hover actions,
  copy/save/drag, clear/dismiss versus destructive deletion, custom confirmation,
  blur and image-fragment disintegration. Check nonactivation and click-through.
- Recording: pause/resume/restart countdown, mute, hide/restore, region outline,
  session-lock safety, recovery, playback, trim/crop, and encoded exports.
- Preferences/history: appearance/tokens, real persisted settings, filter and
  restore behavior, aligned actions, and clearly destructive delete controls.
- Platform integration: tray, shortcuts, file opening, DPI/multiple monitors,
  capture exclusion and accessibility. Wayland restrictions must be stated,
  not hidden behind an X11 screenshot.

Every exported or deleted file in automation must be a disposable fixture.
Never point a native experiment at the shipping app's settings/history directory.
No automatic profile migration is part of these comparison runs.

## Remaining replacement blockers

These are implemented experiments, not drop-in replacements. In particular:

- **Linux:** the full X11 interaction suite is not green: GTK4 accessibility
  visibility/coordinates can become stale during automation. Focused real-window
  checks do not establish full-suite success. Recording exports still preserve
  the source rather than implementing the shipping overwrite workflow. Physical
  multi-monitor, hardware GPU, and Wayland behavior remain unverified.
- **Windows:** screenshot-editor layout and selected-shape transforms still
  differ from Tauri. Recording playback uses a CPU FFmpeg decoder feeding D2D,
  not Media Foundation/D3D video decoding; playback audio, recording split/video
  crop, and microphone/countdown parity remain incomplete. Recording segment
  assembly still blocks during stop. Hosted D3D fixture captures do not establish
  physical DPI, capture, clipboard, drag, or cross-process click-through behavior.
- **macOS:** Swift builds, XCTest, and cached layout renders exercise different
  guarantees from actual AVPlayer presentation and dust animation. Compositor
  evidence must pass independently. Capture permissions, microphone/audio,
  session recovery, and physical multi-display behavior still need native use.

## Matched measurements

The Linux comparison helper accepts explicit release binaries so the GTK4 app
cannot accidentally be confused with the GTK3 baseline:

```sh
python3 experiments/native-ui/native_comparison.py \
  --lab /tmp/captures-native-platform-lab \
  --native /absolute/path/to/native-release-binary \
  --native-label 'GTK4' \
  --tauri /absolute/path/to/current-tauri-release-binary \
  --states preferences image-editor --runs 5 --settle 15 --idle 2 \
  --artifacts /tmp/native-platform-comparison > /tmp/native-platform-results.json
```

Use the disposable X11/DBus desktop documented in
[DEVELOPMENT.md](../DEVELOPMENT.md#optional-native-ui-experiment). Run `--inspect` first
and inspect its screenshots in light and dark mode. GPU output must be captured
from the compositor; a correctly sized blank backing pixmap is not evidence.
The helper checks process IDs and actual client dimensions. Both apps use the
same image, appearance, disabled mini previews for document-only measurements,
and viewport sizes. Samples alternate app order after discarded visual warmups.
Use a 1600×1000 or larger desktop so the window manager's decorations fit around
the 1280×760 editor client. `xwininfo` supplies absolute client coordinates;
`xdotool` can double-count frame offsets on reparented windows.

Reported Linux memory is whole-process-tree PSS, RSS, and private memory; it
includes Tauri helper processes but not desktop services or GPU allocations.
Mapping latency is not content-ready latency. Idle CPU is not animation FPS.
The helper records binary/input hashes and raw trials. Do not run builds or
screen recordings concurrently with resource sampling. Video-editor measurements
require separately verified playback in both apps; an error/black player cannot
be counted as equivalent work.

macOS and Windows need their own native desktop runs. Linux software-rendering
numbers cannot establish their memory, latency, energy use, or GPU performance.
Native compilation in CI is useful evidence, but does not validate physical
capture permissions, audio, high-refresh frame pacing, or mixed-DPI desktops.

### Measured results: September 12, 2026

Five alternating release trials per app/state, following discarded visual
warmups, at [the measured revision](https://github.com/joswayski/captures/commit/922eaecc756731441dee5f6b4743125e40c6457d).
Both apps used the same 960×540 fixture, dark appearance, disabled mini previews,
and client sizes of 980×720 (Preferences) or 1280×760 (editor). Each trial settled
for 15 seconds before a 2-second idle sample. Screenshots were captured from the
compositor and inspected; stale-binary and clipped-capture attempts were rejected
and are not included.

| Platform / state | Tauri PSS | Native PSS | Native reduction | First mapped window, Tauri → native | Idle CPU, Tauri / native |
| --- | ---: | ---: | ---: | ---: | ---: |
| Linux / Preferences | 572.5 MiB | 237.1 MiB | 58.6% | 1,305 → 707 ms | 0.5% / 0.5% |
| Linux / image editor | 589.2 MiB | 246.1 MiB | 58.2% | 1,330 → 680 ms | 0.5% / 0.5% |
| macOS | Not measured | Not measured | — | Not measured | Not measured |
| Windows | Not measured | Not measured | — | Not measured | Not measured |

All numbers are medians. PSS apportions shared pages across processes, avoiding
the double-counting of summed RSS; the samples include one GTK4 process versus
six Tauri processes. CPU is percent of one logical core with coarse 2-second
sampling. These are Linux/X11 software-rendering results, not physical GPU,
animation, recording, video playback, energy, or content-ready benchmarks.
Native macOS/Windows runtime fixtures exist, but no matched whole-app benchmark
was obtained on those platforms. No cross-platform performance claim follows.

Measured binary SHA-256 values:

- GTK4: `625603a601b6884c777fc48bc04e34e7d3a61a47a0e28a3336126d6adc474911`
- Tauri: `cc594b7e8741edbb15e81e9bb6047a915291b9894fe5de39cccec9c7acc68ef1`
