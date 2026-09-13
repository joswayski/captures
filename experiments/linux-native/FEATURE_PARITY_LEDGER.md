# Linux native feature parity ledger

Baseline: `origin/amp/native-platforms` at `ee8516115a8b56ce168e4b86677417d76b9a07e7`.
This ledger records exercised user actions, not release readiness.

## Verified in a disposable X11 profile

- Capture selector: screenshot/record/GIF modes, region move/resize, window and full-display capture, Escape cancellation, session lock, global shortcut without a tray watcher, and single-instance command routing.
- Screenshot editor: pan, draw, rename, layer copy/paste, undo, crop, PNG/JPEG comparison and export, draft recovery, and confirmed source replacement.
- Preferences: light/dark/theme persistence, supported recording FPS/format options, shortcut persistence/conflict validation, capture/recording defaults, and explicit updater-unavailable messaging.
- Recording: real X11 recording, pause/resume, recovery, trim, quality comparison, export, safe same-format source replacement, and live microphone-meter widget. The orb has no physical microphone, so only the meter's zero/unavailable state is exercised here.
- History and mini previews: open/edit/restore/delete/dismiss are implemented. Close dismisses without deleting the file; Delete uses a custom confirmation.
- Feedback: explicit form and parent-owned bounded transport are wired. No request occurs at startup; loopback is the only acceptance-test endpoint.
- First run: local X11 capability walkthrough is persisted without intercepting explicit `--preferences`, `--open`, or capture CLI routes.

## Implemented but evidence is incomplete

- Mini-preview stack renders and pointer-drag corner placement works. The full preview suite currently blocks on GTK AT-SPI coordinates becoming stale for card hover after moves; this is not counted as passing Close/hover evidence yet.
- Microphone selection and the live meter consume the real recorder level API. Physical microphone and system-audio capture cannot be exercised in this orb.
- Launch-at-login writes the user autostart entry; installed package/Open With registration needs package-level evidence outside this directory.
- X11 topmost/taskbar/absolute placement uses EWMH. Wayland remains deliberately gated because global positioning, capture exclusion, and global shortcuts do not have equivalent generic protocols.

## External/shared limitations (not Linux-only regressions)

- WebM appears in the shipping Tauri recording UI, but the shared `captures-media` export path rejects it. Linux native exposes only MP4/GIF rather than advertising a failing export.
- Native update promotion, signing, and installer selection are external rollout work. Linux native does not point at Tauri installers.
- Crash diagnostics are local-only until explicit consent. An unclean marker alone is not presented as proof of a crash.
