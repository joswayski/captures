# CES

CES is a small, privacy-first screenshot utility for macOS, Windows, and Linux.

The first milestone is a macOS developer alpha. It runs in the tray, captures a region, window, or full screen, copies the result to the image clipboard, and presents a preview. Choose Save to write a PNG under `~/CES`; Dismiss drops an unsaved preview without creating a file. CES does not upload screenshots or send telemetry.

## Current status

| Platform | Status |
| --- | --- |
| macOS 13+ | Primary development target |
| Windows 11 | Experimental; current GDI capture has no macOS-style permission prompt |
| Linux X11 | Experimental; region, window, and display capture |
| Linux Wayland | Experimental; region/display use the desktop screenshot portal, window capture requires X11 |

Region selection is limited to the display under the pointer. Annotations, post-capture editing, OCR, scrolling capture, video, upload, sharing, and a screenshot gallery are intentionally deferred.

## Development

Prerequisites:

- Rust 1.94 with `rustfmt` and `clippy`
- Node.js 24 and npm 11
- macOS Screen Recording permission for live capture tests
- Linux: an X11 display, or a Wayland desktop with an `xdg-desktop-portal` screenshot backend

Install dependencies and run the checks:

```sh
npm install
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm run check
```

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

Use **Open Preview** on a pending capture to inspect the full-resolution image in CES. This dedicated viewer is the future home for annotation and editing tools.

## Build and install

Build CES on the operating system where it will run:

```sh
npm install
npm run build
```

Installers and app bundles are written under `target/release/bundle`. On macOS, open the generated DMG and move CES to Applications. Launch CES once, grant Screen Recording access, then enable **Launch CES when I sign in** from Preferences if desired.

For the current Apple Silicon build, the complete install-and-run flow is:

```sh
npm run build
open target/release/bundle/dmg/CES_0.1.0_aarch64.dmg
# Drag CES to Applications, then:
open -a CES
```

CES runs as a menu-bar utility: it shows a camera at the top-right of macOS and intentionally does not keep a Dock icon. Use that camera to capture, open Preferences, or quit CES.

After macOS grants Screen Recording access, quit CES from the menu-bar camera and open it again from Applications. macOS requires this restart before capture is enabled.

`npm run build` seals local macOS bundles with a valid ad-hoc signature when no Apple identity is configured. Ad-hoc identity is still tied to the exact executable, so macOS can ask for Screen Recording permission again after the code changes. Stable permissions across upgrades require an Apple Development or Developer ID signing certificate; set its name through `APPLE_SIGNING_IDENTITY` and the build script uses it instead of the local fallback.

If CES is enabled in Screen & System Audio Recording but still asks for permission, start a capture and choose **Reset CES Permission**. CES clears only its own stale macOS permission records and immediately requests access for the running build; other applications are not affected. Enable CES in the macOS prompt, then quit and reopen CES once.

For Ubuntu development, install Tauri's native dependencies first:

```sh
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev libgbm-dev
```

Ubuntu Desktop includes the screenshot portal used for Wayland region/display capture. Ubuntu 26.04 is Wayland-only; native Wayland window capture remains a separate follow-up.

## Privacy and future uploads

CES holds pending captures locally in memory and has no network permission in this milestone. Saving is an explicit action. The future upload service will receive an explicit Upload or Share action and will keep object storage private; the desktop app will never receive bucket credentials.
