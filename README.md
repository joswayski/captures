# CES

CES is a small, privacy-first screenshot utility for macOS, Windows, and Linux.

The first milestone is a macOS developer alpha. It runs in the tray, captures a region, window, or display, copies the result to the image clipboard, and saves a PNG under `Pictures/CES`. CES does not upload screenshots or send telemetry.

## Current status

| Platform | Status |
| --- | --- |
| macOS 13+ | Primary development target |
| Windows 11 | Experimental build target |
| Linux X11 | Experimental build target |
| Linux Wayland | Planned portal-backed target |

Region selection is limited to the display under the pointer. Annotations, post-capture editing, OCR, scrolling capture, video, upload, sharing, and a screenshot gallery are intentionally deferred.

## Development

Prerequisites:

- Rust 1.94 with `rustfmt` and `clippy`
- Node.js 24 and npm 11
- macOS Screen Recording permission for live capture tests

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

## Privacy and future uploads

CES saves locally by default and has no network permission in this milestone. The future upload service will receive an explicit Upload or Share action and will keep object storage private; the desktop app will never receive bucket credentials.

