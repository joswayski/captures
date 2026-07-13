# CES

CES is a small, privacy-first screenshot utility for macOS, Windows, and Linux.

The first milestone is a macOS developer alpha. It runs in the tray, captures a region, window, or display, copies the result to the image clipboard, and presents a preview. Choose Save to write a PNG under `~/CES`; Dismiss drops an unsaved preview without creating a file. CES does not upload screenshots or send telemetry.

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

The tray shortcuts default to `Ctrl+Shift+4` for region, `Ctrl+Shift+W` for window, and `Ctrl+Shift+3` for display. They can be changed from Preferences.

## Build and install

Build CES on the operating system where it will run:

```sh
npm install
npm run build
```

Installers and app bundles are written under `target/release/bundle`. On macOS, open the generated DMG and move CES to Applications. Launch CES once, grant Screen Recording access, then enable **Launch CES when I sign in** from Preferences if desired.

For Ubuntu development, install Tauri's native dependencies first:

```sh
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

Ubuntu Desktop includes the screenshot portal used for Wayland region/display capture. Ubuntu 26.04 is Wayland-only; native Wayland window capture remains a separate follow-up.

## Privacy and future uploads

CES holds pending captures locally in memory and has no network permission in this milestone. Saving is an explicit action. The future upload service will receive an explicit Upload or Share action and will keep object storage private; the desktop app will never receive bucket credentials.
