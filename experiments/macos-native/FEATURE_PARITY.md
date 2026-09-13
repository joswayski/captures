# Native macOS parity ledger

Baseline compared: shipping Tauri sources at `ee8516115a8b56ce168e4b86677417d76b9a07e7`.
This ledger distinguishes implemented behavior from native-runtime and rollout gates.

## Implemented in the native app

- Capture: display/window/region selection, frozen screenshot selection, cursor,
  countdown, explicit confirmation/auto-start, aspect presets and dimensions.
- Recording: display/window/region/GIF starts, cursor/click/key startup options,
  desktop audio and microphone selection, pause/resume/restart countdown, runtime
  microphone mute, crash-draft recovery, custom HUD and region guide.
- Recording editor: one 12-frame filmstrip timeline, trim/crop/resolution/audio,
  MP4/GIF output, Preserve/compression/maximum-size modes, automatic before/after,
  real save-pipeline size estimates, create-new and atomic source replacement.
- Image editor: persisted drafts, crop/resize/trim, text/arrow/line/shape/image
  layers, ordering/visibility/locking, opacity/fill/color, layer shadows, blur,
  free rotation with 15-degree snap, contiguous/global color wand with tolerance,
  live layer movement with edge snap guides, explicit canvas overflow expansion,
  interpolated hard/soft erase and restore brushes, PNG/JPEG/WebP export and
  transparency-preserving PNG/WebP output, fit/manual 5–800% viewport zoom,
  pointer-anchored modified-wheel/pinch zoom, Command/Control or middle-drag pan,
  six shipping blend modes, merge down, merge visible, and flatten with matching
  hidden-layer and canvas-background behavior; text style presets, sans/serif/
  monospace/rounded families, bold/italic/alignment, outlined and box labels,
  and shared color/opacity/fill/stroke/shadow defaults for new annotations.
- App surfaces: persisted preferences with runtime consumers, history restore/edit/
  trash, mini-preview stacks and disintegration, custom confirmations/popovers/
  toggles, global shortcut registration failures, duplicate shortcut rejection,
  installed Open With declarations and URL import, first-run permission onboarding,
  explicit user-initiated feedback on a separate worker lane, and profile-scoped
  local crash evidence with redacted preview and explicit Send/Dismiss consent.

## Implemented; native CI or hardware evidence still required

- Onboarding Screen Recording prompt/settings/restart and optional microphone flow.
- Open With cold-launch deferral (avoids flashing setup/Preferences before an editor).
- Feedback SwiftUI interaction and its bridge call. Shared transport is loopback-tested;
  CI must never submit a production fixture.
- Crash marker lifecycle, exact-identity macOS report selection, redacted consent UI,
  and clean normal quit/restart behavior. The shared collector is subprocess-tested,
  but Swift/AppKit lifecycle behavior still requires native CI and physical-Mac checks.
- Exact 12-cell filmstrip layout and live estimate pending/exact/approximate states.
  Successful native estimate renders exist, but one rerender timed out intermittently
  without backend diagnostics. The confirmed initial-subscription race is fixed and
  failures now report waiting/pending/backend state; it is not established as that timeout's cause.
- Layer blend rendering and merge/flatten rasterization have passed native pixel tests.
  The [lifecycle run](https://github.com/joswayski/captures/actions/runs/34729382697)
  passed 29 AppKit tests, 27 normal references and the video/dust compositor checks.
  Inspected references show no Recenter on the fitted canvas and all three Combine
  controls fully visible after scrolling. Physical menu interaction remains unverified.
- Rich text family fallback, glyph metrics, outlined glyphs, rounded plates and
  default-style controls have new model and pixel tests awaiting native execution.
  Installed-font behavior and physical text entry still require interaction evidence.
- Native modifier/middle-drag pan, pointer-anchored wheel/pinch zoom, and the
  mostly-offscreen Recenter cue. Model math is asymmetric-tested; physical input
  routing and trackpad magnification still require native interaction evidence.
- Main runtime focus, nonactivation, click-through, capture exclusion and multi-DPI
  behavior. Static references are layout evidence only; video/dust require compositor capture.

## Confirmed gaps

- Text placement edits through the properties TextEditor rather than shipping's
  inline canvas composer; inline focus/caret behavior and custom shadow colors
  remain missing even though multiline auto-sizing and shadow geometry are implemented.
- Recording responsiveness: stale estimate results are generation-guarded, but queued
  and running estimates/exports are not cancelled or coalesced. They can still delay Save
  on the serial media queue; this needs an owned cancel protocol matching shipping.
- Repeated screenshot-hotkey behavior while capture UI is visible is not yet equivalent
  to shipping's intentional recapture/switch-target state machine.
- The post-launch tray pill is not yet implemented. Update checking/notices/install are
  intentionally unavailable because no signed native update channel exists.

## Shared or rollout limitations (not native-only feature gaps)

- WebM is offered by shipping UI but rejected by the shared media backend for exports,
  previews and estimates. Native UI does not pretend it can save WebM.
- Runtime system-audio mute and recording split are not shipping features. Export-time
  system gain/mute is implemented.
- Native updater promotion, signing/notarization, package installation and production
  data migration are rollout gates. A native updater must never consume Tauri installers.
