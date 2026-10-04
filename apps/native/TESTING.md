# Captures Native Development: exploratory testing

This is an experimental native build, **not the shipping Captures Preview**.
Do not replace the installed app or point this build at its settings or History.
Signed updates, installed-data migration/rollback and full platform acceptance
are not ready. Wayland capture/recording remains unavailable.

## Start safely

1. Quit the shipping Captures app to avoid competing global shortcuts.
2. Extract the entire package to a new directory. Keep its resources, `binaries`
   and `media-licenses` in their packaged locations. CI archives include FFmpeg;
   locally staged packages include it only when built with `--media-target`.
3. macOS: open **Captures Native Development.app**. CI packages are ad-hoc signed, not
   notarized; Gatekeeper may require an explicit approval in Privacy & Security.
   Do not disable Gatekeeper. This CI package is Apple Silicon, built on macOS 26;
   other macOS versions and Intel Macs are not verified by this artifact.
4. Windows: run `.\CapturesNative.exe --live` from PowerShell in this directory.
   It is unsigned and may show an OS security warning.
5. Linux: run `./captures-native --live` in an X11 session. CI builds on Ubuntu
   24.04 x86_64; other distributions may need a local build and runtime libraries.

Portable Windows/Linux archives omit Open With registration files because their
paths must be generated on your own computer. Do not register a locally staged
package during the initial test.

Complete native setup and allow screen access when requested. Preferences and
History use a separate native development profile. Choose a **new output folder**
in Preferences; exported files are ordinary files and are not isolated by the
profile. Leave Start Captures on login off during this first test.

## Useful first pass

- Screenshot a region, a window and a display. Try Escape, countdown cancellation
  and repeated captures. Check that Captures' own controls are excluded.
- Open a screenshot in the editor. Add text/shapes, crop, use Undo/Redo, Copy and
  Save. Close/reopen an unsaved draft and verify its recovery.
- Record a short disposable MP4/GIF. Try pause/resume, mute, Stop, editing/export
  and Hide/restore through the tray/menu bar. Test real audio only if desired.
- Drag History and mini-preview files to another app. Check copied bytes and
  cancelled drags. Delete only test captures; ordinary exported files remain.
- Use capture shortcuts while Preferences is focused, then actively record a new
  shortcut and check that its keystrokes do not trigger a capture.
- Quit normally, reopen, and check saved preferences and drafts. Feedback is
  optional: nothing sends without pressing Send. Diagnostic Copy stays local.

Do not treat a successful first pass as hardware, multi-display/mixed-DPI,
accessibility/IME, physical audio or crash/shutdown acceptance. Native updates and
account sharing remain unavailable; no release date is promised.

## Report what happened

CI archives include `BUILD_INFO.json` with the source commit and packaged binary
SHA-256. Include it, your OS/version and display scales, the exact action sequence,
expected/actual result and whether it repeats. Screenshots are optional; avoid
private captures, contact information and credentials. No test result uploads
automatically.

Quit this development app before returning to shipping Captures. If you enabled
native login or Open With registration, disable/unregister that development
entry before deleting or moving the package; see DEVELOPMENT.md in the repository.
