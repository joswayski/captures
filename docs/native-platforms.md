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
