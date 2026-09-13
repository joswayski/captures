# Linux native feature parity ledger

Baseline: `origin/amp/native-platforms` at `ee8516115a8b56ce168e4b86677417d76b9a07e7`.
This ledger records exercised user actions, not release readiness.

## Verified in a disposable X11 profile

- Capture selector: screenshot/record/GIF modes, region move/resize, window and full-display capture, Escape cancellation, session lock, global shortcut without a tray watcher, and single-instance command routing.
- Screenshot editor: pan, draw, rename, layer copy/paste, undo, crop, PNG/JPEG comparison and export, draft recovery, and confirmed source replacement.
- Preferences: light/dark/theme persistence, supported recording FPS/format options, shortcut persistence/conflict validation, capture/recording defaults, and explicit updater-unavailable messaging.
- Recording: the strict GTK4 helper now passes end to end in a fresh profile on 1600×1000 Xvfb/Openbox with a private Pulse null sink/monitor and ALSA pulse configuration. It covers microphone mute/unmute, pause/restart, screenshot cancel/save, lock recovery, hide/restore, stop and draft cleanup; real preview pixel progression; keyboard/pointer trim and crop; draggable/keyboard comparison and Play dismissal; chosen-folder 320×200 MP4/GIF export and unchanged source; 380×220 Region GIF, 992×610 Window MP4, interrupted GIF recovery/history, and two independent ffplay children with per-stream mute. This is synthetic audio/process evidence, not physical microphone or AV-sync acceptance. Earlier editor-only evidence did not establish a full live pass.
- Recording comparison: bounded sample extraction chooses the last PTS at or before the requested time, including EOF. An asymmetric VFR pixel test covers 172/173/436/437/500 ms boundaries and stale-file rejection. Comparison readiness requires both decoded images. The visible divider/hit target uses native resize and one stationary-coordinate drag gesture; inspected comparison and Custom 320×200/Tiny options retain the full footer. Native folder selection retains its dialog until response.
- History and mini previews: open/edit/restore/delete/dismiss are implemented. Close dismisses without deleting the file; Delete uses a custom confirmation.
- Preview follow-up: the clean Openbox suite passes four corner placements, foreign-surface hover rejection, edge/foreign-window release nonactivation, per-card Copy/Close, external file drag, clear and confirmed delete. Captures were inspected; the parent test retains stronger lost-release/hover recovery assertions.
- History follow-up: uses authoritative X11 coordinates, centers the realized frame, and preserves exact mustard Edit/destructive hover pixels, 32×32 trash controls, toggle state, Restore, removal/clear and source files. Both idle and hover clients were inspected. Preferences centering also uses the realized outer frame rather than GTK's initial 1×1 allocation.
- Feedback: a held loopback response proves pending freeze, deliberate 500 recovery with exact retained message/contact/category/context, retry without retyping, and read-only success/Close. No request occurs at startup; no production request was submitted.
- First run: local X11 capability walkthrough is persisted without intercepting explicit `--preferences`, `--open`, or capture CLI routes.

## Implemented but evidence is incomplete

- Preview placement is verified under Openbox only. Selected-monitor math covers negative/offset origins, but runtime cross-monitor, mixed-DPI and other-WM behavior remains unverified. The pile snaps on release rather than following continuously during drag.
- The passing recording lab uses synthetic Pulse audio only; a clean orb without that configuration still has no default microphone. The helper resets its moving reference before each run so source-motion evidence is repeatable. GTK4 checks use live accessibility nodes, supported window-relative coordinates, real pointer/keyboard selection and independent media/pixel assertions.
- Microphone selection and the live meter consume the real recorder level API. Physical microphone and system-audio capture cannot be exercised in this orb.
- Launch-at-login writes the user autostart entry; installed package/Open With registration needs package-level evidence outside this directory.
- X11 topmost/taskbar/absolute placement uses EWMH. Wayland remains deliberately gated because global positioning, capture exclusion, and global shortcuts do not have equivalent generic protocols.

## External/shared limitations (not Linux-only regressions)

- WebM appears in the shipping Tauri recording UI, but the shared `captures-media` export path rejects it. Linux native exposes only MP4/GIF rather than advertising a failing export.
- Native update promotion, signing, and installer selection are external rollout work. Linux native does not point at Tauri installers.
- Crash diagnostics are local-only until explicit consent. An unclean marker alone is not presented as proof of a crash.
