# Native platform migration

Captures is moving toward platform-specific native presentation around the
existing Rust capture, recording, media, and session engines. The shipping Tauri
Preview remains the comparison baseline and distributed app until the native
frontends pass their replacement checks. This work does not change installers,
update channels or production profiles. Native setup requests its own OS
permissions; existing Tauri permission grants are not migrated.

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

- **Linux:** the full screenshot-editor X11 fixture now passes, including
  space-pan followed by drawing, inline rename, selected-color undo, layer-menu
  duplication, compression/maximum-size controls, exact undo, reverse crop,
  PNG/JPEG export, draft recovery, and source replacement. The corrected GTK4
  event adapter maps surface coordinates through the native/widget transforms
  and preserves press/motion/release propagation. A single key controller now
  pairs Space press/release so pan does not remain active; editable fields
  retain their keyboard input. The screenshot-editor result is not a pass for
  the entire capture/recording interaction suite: the capture fixture still
  stops at a mini-preview hover/Close lookup after verifying free/Shift-square/
  released selection and a saved capture. Recording replacement uses a synced stage
  and explicit confirmation; cancellation preserves the original. Physical
  multi-monitor, hardware GPU, and Wayland behavior remain unverified.
- **Windows:** canvas dimensions/background, fit/manual zoom, Ctrl-drag pan,
  output dimensions/quality/maximum-size settings and layer front/back/duplicate/
  delete/rename actions are implemented. Erase, background removal, merge/flatten
  and richer layer controls remain incomplete. Imported image layers use the shared raster renderer
  with native multi-select file picking, asynchronous decode, transformed hit
  testing, and batch undo/redo. The model supports locking, independent opacity,
  and six blend modes. Native image controls now expose dimensions, opacity and
  rotation rather than ignored stroke/fill properties. Preferences has General,
  Capture, Recording, Shortcuts and Appearance pages; some settings still need
  runtime consumers and shortcuts remain read-only. Image preview and encoding
  run off the message thread. Estimates retain only the newest pending job;
  completion checks fence document identity and revision. Save jobs exclude
  simultaneous writes for the same document or normalized destination, retain
  successful older artifacts in history, and never retarget a newer editor.
  Expanded-canvas cropping has independent pixel/undo coverage. Explicit worker-thread
  feedback has its own form; no startup request is made.
  Recording playback uses a CPU FFmpeg decoder feeding D2D,
  not Media Foundation/D3D video decoding; playback audio, video
  crop, and microphone/countdown parity remain incomplete. Recording segment
  assembly still blocks during stop. Hosted D3D fixture captures do not establish
  physical DPI, capture, clipboard, drag, or cross-process click-through behavior.
  [The accepted Windows run](https://github.com/joswayski/captures/actions/runs/34723107948)
  passed MSVC build, 45 tests, strict clippy, and 36 light/dark captures with
  hardware drivers and verified full desktop bounds. The imported-image renders
  contain real rotated translucent raster layers. Preferences and feedback
  layouts were inspected. The new increment fixes image-property values that
  overlapped stepper buttons and adds an HWND-message input fixture for canvas
  editing and shape drawing. Its 55 portable tests and host strict Clippy pass;
  MSVC tests and the new interaction renders await CI. The parent orb lacks
  MinGW GCC for its independent GNU cross-check. Static fixtures do not verify
  picker or physical text input.
  Inspected recording
  fixtures show decoded paused frames, not playback timing or hardware video decode.
- **macOS:** Swift builds, 19 XCTest tests, normal layouts, and independent real
  video/dust compositor checks passed in [the native CI run](https://github.com/joswayski/captures/actions/runs/34724671189).
  The inspected dust frame contains displaced source fragments and transparent
  holes; the video frame contains only Captures' custom controls. These checks
  do not establish performance or full interaction parity. Capture permissions, microphone/audio,
  session recovery, and physical multi-display behavior still need native use.
  Subsequent slices add first-run permission setup, explicit feedback, soft
  interpolated erase/restore, contiguous/global wand, snapping/guides, atomic
  bounded canvas expansion, and local crash review with explicit Send/Dismiss.
  Restart prepares its helper before clearing the session marker and restores
  tracking if cleanup fails. Bridge tests reject duplicate crash startup before
  it mutates session files. Native CI verified the onboarding restart protocol
  in app/test/reference source sets, new AppKit pixel/expansion tests, and a real
  76 KB recording estimate. Crash consent and feedback layouts were inspected;
  no request was submitted. Inspection found persisted drafts contaminating
  successive editor reference states. Reference-only models now start from fresh
  source documents. Native CI passed the isolation regression; inspected renders
  show exactly two layers in overflow/snap states with no inherited shapes or
  restored-draft banner, plus the expected expansion action and both guides.

## Feature parity acceptance

The full-feature pass compares executable shipping actions, not just visible
controls or the historical GTK3 audit. These action families must pass on each
native frontend before claiming application parity:

| Action family | Required behavior and evidence |
| --- | --- |
| Capture | Region/window/display; screenshot/recording selection; aspect presets; repeated shortcut behavior; countdown cancellation; session-lock rejection; capability-aware cursor/click/key/audio options. Verify saved pixels and actual options. |
| Image editing | Import images; draw/text/shapes; select/move/resize/rotate/snap; erase/restore/wand; text/brush styling and shadows; visibility/lock/rename/reorder/duplicate/opacity/blend/merge; crop/trim/expand/background; fit/zoom/pan. Exercise asymmetric documents, undo/redo, and restored drafts. |
| Image export | PNG/JPEG/WebP; output dimensions; quality/max-size; current estimate/comparison; clipboard; destination/filename; save-new versus safe source replacement. Decode exported bytes and verify dimensions/pixels, cancellation and source safety. |
| Recording session | Start/pause/resume/restart/countdown; microphone selection/meter/mute; screenshots while recording; hide/restore; discard/stop/recovery. Keep UI responsive during assembly and verify resulting media. |
| Recording editor | Actual playback/seek/loop; one trim timeline; crop/resize; audio gain/mute/mono; quality/max-size/comparison/estimate; cancel/export/reveal/source replacement. Verify decoded media and edits, not only paused screenshots. |
| Preview/history | Collapse/expand/corner placement/drag; hover/copy/save/edit/reveal; restore/filter; nondestructive dismiss versus confirmed delete; keyboard and accessibility routes. Check real pointer coordinates and persisted state. |
| Preferences/integration | Every shipping preference and its runtime consumer; theme/system appearance; shortcuts/conflicts; first-run setup; launch notice; consent-based feedback/crash behavior; file routing; tray/autostart. Reopen with isolated settings to verify persistence. |

Known implementation work remains in all three frontends. Windows has the
largest editor/capture/settings gap; Linux still lacks its live microphone
meter and first-run/launch flows; macOS still needs richer image-layer/viewport
operations. Its new live recording estimate uses the real save pipeline; a Linux
bridge check verified exact source/full-encode sizes and independently reproduced
sampled estimates. Native CI now verifies a successful Swift estimate display.
Shared native feedback transport now has loopback tests for exact request fields,
concurrent submission cooldown, retries, Unicode limits and response bounds;
frontend integration and consent flows remain in progress. It makes no automatic
requests. The optional local crash module retains profile-isolated unclean-session
and redacted panic evidence, and summarizes caller-supplied OS reports only after
native executable identity/timestamp checks. Tests cover a real subprocess panic,
retention across clean relaunch, path redaction, modern macOS IPS, UTF-16 Windows
WER and Linux Apport. macOS now integrates shutdown tracking and review/Send/
Dismiss UI, with native layout evidence but physical lifecycle verification still
pending; other platform integrations remain ongoing. An unclean marker alone is
not a proven crash.
Native installer/Open With registration,
signed updater/channel, and production-profile migration are separate rollout
gates, not satisfied by local command-line routing. Never install a Tauri update
over a native frontend. Publishing a replacement remains a maintainer decision.

Source review also corrected three misleading parity targets: shipping has no
recording split action (the split slider compares compression), no persisted
recording-editor edit draft, and no runtime system-audio mute control. Session
crash recovery and export-time audio controls do exist and remain required.
WebM is offered in shipping UI but rejected by the shared media toolchain;
implementing a new codec is not a native-only parity fix.

## Control parity pass

The native editors now follow the shipping control hierarchy more closely:
one Trim action, compact switches, icon-labelled primary actions, and
filename/format/export footers. Linux layer actions live in per-layer menus,
and its default inspector is empty. Windows has a separate Arrow tool and a
three-column Shapes menu with shared render/hit-test geometry. macOS uses
checkboxes for checkbox settings rather than oversized switches, with explicit
on/off/disabled control references. Recording timelines use one trim track with
two handles rather than duplicate decorative edges.

The Linux screenshot editor's default, selected-layer, menu-open, and
export-open states were rendered and inspected under software X11. The
[macOS parity run](https://github.com/joswayski/captures/actions/runs/34719051965)
passed Swift build, 11 EditorModel tests, 16 normal references, and the video/dust
compositor checks. Inspection confirmed corrected controls but found empty
recording filmstrip cells that the old preview-only validator missed. The fixture
now waits up to ten seconds for rendered thumbnails and requires source pixels
at both timeline ends, independently of the video preview. Failed filmstrip
captures are saved and uploaded for diagnosis. Readiness validation passed in
[the follow-up run](https://github.com/joswayski/captures/actions/runs/34720104041):
normal left/right thumbnail samples were 1,446/1,296 and compositor samples
1,448/1,299, above 40 per side. Inspection then found twelve decoded cells
overflowing their track; explicit equal cell widths fix that layout, pending
its next native render at that revision. The
[next run](https://github.com/joswayski/captures/actions/runs/34721634028)
now shows all twelve decoded cells fitting the strip, verified by direct image
inspection. The
[Windows parity run](https://github.com/joswayski/captures/actions/runs/34719051988)
passed 38 MSVC tests and rendered 26 fully contained light/dark hardware-driver
captures, including decoded paused video frames. Inspection found wrapped labels
and overlapping footer text; the follow-up corrects those and moves the fixture
playhead between the two trim handles for independent visual verification.
The [Windows follow-up](https://github.com/joswayski/captures/actions/runs/34720104027)
verified those corrections in both appearances and a separate one-third playhead.
Neither run verifies continuous playback, audio, or WARP rendering. Windows
eraser and editable canvas controls remain incomplete; the new Preferences
pages do not yet cover every shipping setting and consumer. This pass does not establish full feature parity or
change the measured revision below.

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
