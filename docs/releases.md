# Captures releases

Every successful push to `main` runs `.github/workflows/release.yml`. Release workflows wait in commit order without cancelling older pending pushes, run the frontend and Rust quality gates, and then build macOS Apple Silicon, Windows x64, and Linux x64 packages.

The public version is CalVer in `YYYY.MM.DD.N` form, using the `America/New_York` date and a same-day revision from 1 through 99. A release named `Captures 2026.07.19.1` uses tag `v2026.07.19.1`. Tauri receives the SemVer-compatible internal version `2026.7.1901`; source manifests remain at the development version.

The workflow creates a draft release at the exact tested commit. Each platform uploads its installer, updater archive, and updater signature. The macOS job also builds and inspects the pinned LGPL FFmpeg sidecars, verifies their copies inside `Captures.app`, and uploads the matching FFmpeg source archive, detached signature, build configuration, LGPL license, and notice. The final job requires those files plus a DMG, NSIS installer, AppImage, Debian package, complete `latest.json`, and `SHA256SUMS` before it publishes the release and marks it latest. A failed build removes its draft and tag, leaving the prior release and updater manifest untouched. If draft creation itself is interrupted, the next run removes only stale drafts with its generated tag before retrying.

## GitHub release environment

Create a GitHub environment named `release`, restrict its deployment branches to `main`, and add these environment secrets:

| Secret | Purpose |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | Dedicated Tauri updater private key |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password for that updater key |
| `APPLE_CERTIFICATE` | Base64-encoded Developer ID Application `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | Export password for the `.p12` |
| `KEYCHAIN_PASSWORD` | Temporary CI keychain password |
| `APPLE_API_ISSUER` | App Store Connect API issuer ID |
| `APPLE_API_KEY` | App Store Connect API key ID |
| `APPLE_API_PRIVATE_KEY` | Contents of the App Store Connect `.p8` private key |

Commit only the updater public key. Keep the updater private key, its password, and the App Store Connect private key in encrypted offline backups. Losing the updater private key means existing installations cannot verify a replacement key or receive another in-app update.

The macOS build intentionally fails when any Apple credential is missing or the imported identity is not a Developer ID Application certificate. Do not merge the updater bootstrap PR until the paid Apple Developer account, certificate, and notarization credentials are ready.

## Bootstrap and acceptance

The first updater-enabled build must be downloaded and installed manually. Before relying on automatic releases, test this sequence on clean machines:

1. Install one published release on macOS, Windows, and Ubuntu.
2. Merge another PR on the same New York calendar day and confirm the revision increments.
3. Confirm the release appears only after all platform assets and all three `latest.json` entries exist.
4. Choose **Later**, then install from the tray and verify the displayed version after restart.
5. On macOS, verify notarization, stapling, and Screen Recording permission survive the update.
6. Verify Windows NSIS and Linux AppImage update in place; verify `.deb` directs the user to the release download.
7. Tamper with an updater archive in a test release and confirm signature verification rejects it.
8. Force one platform build to fail and confirm the failed draft/tag is deleted while the previous release remains latest.
9. On macOS, rapidly resize a recording region, confirm window highlights use rounded corners, and verify Window mode starts without choosing an arbitrary system window. Drag the selector controls from any non-interactive panel surface, press Escape repeatedly to cancel, and verify the selector, countdown, and HUD can be captured by another app while Captures excludes them from its own output.
10. Record a visually static display for at least 10 seconds, then repeat while continuously moving content on a secondary display. Confirm the single full-display countdown fades from 3 to 2 to 1 without flashing 3 again, Escape cancels it, the start chime plays only when recording begins and is absent from recorded audio, real motion continues past the initial frame, and both finalized durations are within 250 ms or 5% of wall-clock time, whichever is larger. On macOS 15+, enable **Show clicks** and verify clicks receive the system highlight.
11. Verify the recording HUD uses one compact control row with no border or shadow, shows unclipped tooltips and the faded **These controls won’t show in the recording** note below it, labels its active state **Recording**, responds on hover, drags from non-interactive background space, hides completely, and returns when Captures is opened from the macOS Dock. Confirm Restart requires explicit native confirmation.
12. Open a 1140×692 source in the editor and verify Fit and 100% previews, the centered video play/pause control, the 12-frame filmstrip, playhead, trim handles, crop overlay, resolution label, conditional audio controls, and millisecond labels stay synchronized.
13. Record a Retina region, window, and display with **Original** resolution and verify their masters use physical rather than logical pixels. Confirm moving content continues to update, cursor movement remains visible, **Show clicks** forces the cursor on and renders click feedback, and an untouched **Preserve quality** save updates the original without re-encoding. Verify audio-only changes preserve the video stream, visual edits use the high-quality H.264 path, a failed edit leaves the original intact, filename and folder remain editable when updating the original, optional **Keep original** saves a copy, collisions are rejected, strict maximum-size output does not exceed its selected KB, MB, or GB ceiling, and a successful save opens Finder with the measured final size and a **Show in Folder** action shown.
14. While recording, start a region screenshot and confirm the HUD fades away without flickering before the selector appears.
15. Delete a quick-access preview and confirm its native window becomes click-through immediately while the particle animation finishes.
16. Confirm the packaged FFmpeg/ffprobe sidecars run on a clean Mac and the release includes every source and compliance asset listed in `docs/media-sidecars.md`.

Windows packages are intentionally unsigned during the private alpha and may trigger SmartScreen. Add Authenticode signing before a public launch.
