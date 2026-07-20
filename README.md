# Captures

Captures is a small, privacy-first screenshot utility for macOS, Windows, and Linux. The planned sharing service will use compact [`captur.es/<id>`](https://captur.es) links.

The first milestone is a macOS developer alpha. It runs in the tray, captures a region, window, or full screen, copies the result to the image clipboard by default, and presents a preview. Automatic clipboard copying can be disabled in Preferences. Choose Save to write a PNG under `~/Captures`; dismissing the preview leaves its private recovery copy in **Capture History** for 30 days. Captures does not upload screenshots or send telemetry.

## Current status

| Platform | Status |
| --- | --- |
| macOS 13+ | Primary development target |
| Windows 11 | Experimental; current GDI capture has no macOS-style permission prompt |
| Linux X11 | Experimental; region, window, and display capture |
| Linux Wayland + XWayland | Experimental; region/display use the desktop screenshot portal, while window capture can see X11/XWayland windows only |
| Linux without XWayland | Not supported yet; the pinned capture backend still needs X11 for monitor discovery |

Region selection is limited to the display under the pointer. Annotations, post-capture editing, OCR, scrolling capture, video, upload, and sharing are intentionally deferred.

## Development

Prerequisites:

- Rust 1.94 with `rustfmt` and `clippy`
- Node.js 24 and npm 11
- macOS Screen Recording permission for live capture tests
- Linux: an X11 display, or a Wayland desktop with XWayland and an `xdg-desktop-portal` screenshot backend

Install dependencies and run the checks:

```sh
npm install
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm run check
```

CI runs the Rust checks on macOS, Windows, and Ubuntu, then produces a macOS app bundle, Windows NSIS installer, and Ubuntu DEB package. These jobs verify compilation, tests, and packaging; live capture, desktop-portal, tray, clipboard, and launch-at-login behavior still require a manual pass on each platform.

Run the desktop app in development mode:

```sh
npm run dev
```

This runs the debug executable directly. Stop it with `Ctrl+C` in the terminal. On macOS, the debug executable and installed application have separate Screen Recording identities.

## Shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl+Shift+4` | Capture a selected region |
| `Ctrl+Shift+W` | Capture a window |
| `Ctrl+Shift+3` | Capture the full screen under the pointer |
| `Esc` | Cancel an active region or window capture |

The three global capture shortcuts can be changed from Preferences. `Esc` only applies while the capture overlay is open.

Use **View Full Size** on a pending capture to inspect the full-resolution image in Captures. Each screenshot opens in its own dedicated viewer window; opening the same screenshot again focuses its existing viewer. A subtle purple glow follows the last active open viewer without flickering when the pointer crosses the thumbnail strip. The viewer is the future home for annotation and editing tools.

The **Copied to clipboard** badge is live: it disappears and the **Copy** action returns when another app replaces the clipboard with text, an image, or any other content. With multiple capture previews open, only the preview currently owned by the clipboard hides its Copy action.

## Capture history

Every completed screenshot is backed up at full resolution in Captures' private local app-data directory. Open **Capture History…** from the tray menu to browse the last 30 days and restore a dismissed capture to the preview stack. Moving a separately saved PNG to Trash does not remove its recovery copy. History entries expire automatically after 30 days, and deleting one from the history window requires a second confirmation click.

## Build and install

Download the newest user-facing build from [GitHub Releases](https://github.com/joswayski/captures/releases/latest).

| Platform | Architecture | Package | Updates |
| --- | --- | --- | --- |
| macOS 13+ | Apple Silicon | `.dmg` | Signed in-app install and restart |
| Windows 11 | x64 | NSIS `.exe` | In-app install and restart |
| Linux | x64 | `.AppImage` | In-app install and restart |
| Debian/Ubuntu | x64 | `.deb` | Notification with a manual package download |

The first release containing the updater must be installed manually. After that, release builds check 15 seconds after startup, every four hours, and whenever **Check for Updates…** is chosen from the tray or Preferences. An available update is never installed without confirmation, and Captures will not restart during a capture or while an unsaved capture is open.

macOS releases are required to use a consistent Developer ID Application signature and Apple notarization so trust and Screen Recording permission can survive an update. Windows releases are not Authenticode-signed yet, so Windows may show a SmartScreen warning during this private alpha. Production Windows signing is required before public launch.

Build Captures on the operating system where it will run:

```sh
npm install
npm run build
```

Installers and app bundles are written under `target/release/bundle`. On macOS, open the generated DMG and move Captures to Applications. Launch Captures once, grant Screen Recording access, then enable **Launch Captures when I sign in** from Preferences if desired.

For the current Apple Silicon build, the complete install-and-run flow is:

```sh
npm run build
open target/release/bundle/dmg/Captures_0.1.0_aarch64.dmg
# Drag Captures to Applications, then:
open -a Captures
```

Captures runs as a menu-bar utility: it shows the Captures icon at the top-right of macOS and intentionally does not keep a Dock icon. Use that icon to capture, open Preferences, or quit Captures.

After macOS grants Screen Recording access, retry your shortcut and let Captures restart itself when prompted. macOS requires this restart before capture is enabled.

`npm run build` automatically uses an installed Apple Development signing identity when one is available. Otherwise it warns and seals the bundle with an ad-hoc signature. Local builds omit updater artifacts unless `TAURI_SIGNING_PRIVATE_KEY` is explicitly provided; official release builds always create and sign them. Ad-hoc identity is tied to the exact executable, so macOS asks for Screen Recording permission again after code changes. Stable permissions across upgrades require an Apple Development or Developer ID certificate; the latter can be selected explicitly through `APPLE_SIGNING_IDENTITY`.

If Captures is enabled in Screen & System Audio Recording but still asks for permission, start a capture and choose **Reset, Restart & Retry**. Captures clears only its own stale record, restarts itself, and resumes the requested capture; other applications are not affected. After you approve Captures in System Settings, retry the shortcut once and choose **Restart & Retry** so the newly granted access takes effect.

For Ubuntu development, install Tauri's native dependencies first:

```sh
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev libgbm-dev
```

Ubuntu Desktop includes the screenshot portal and XWayland support used by the current Wayland region/display path. Native Wayland monitor discovery and window capture remain separate follow-ups.

## Privacy, updates, and future uploads

Captures keeps pending previews in memory and a rolling 30-day capture history in the operating system's private local app-data directory. Saving an additional PNG to the configured output directory is an explicit action. Release builds contact only GitHub Releases to check for and download signed application updates; they do not upload screenshots, usage data, or telemetry. The future upload service will receive an explicit Upload or Share action and will keep object storage private; the desktop app will never receive bucket credentials.

GitHub Releases is the distribution host for now. Object storage such as R2 becomes useful later if Captures needs branded URLs, staged update channels, private binaries, download controls, or a broader hosted backend. Any migration should dual-publish for at least one transition release so installed clients keep a working updater endpoint.

Maintainer setup and release recovery are documented in [docs/releases.md](docs/releases.md).

## License and trademarks

The source code is licensed under the [Apache License 2.0](LICENSE).

The Captures name and logo are governed by the [Captures Trademark Policy](TRADEMARKS.md) and are not licensed under the Apache License 2.0.
