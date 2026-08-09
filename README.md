# Captures

Captures is a cross-platform screen capture utility built for quick captures and a lightweight workflow.

> [!WARNING]
> Captures is functional, but still experimental and under active development. macOS is the primary development target; Windows and Linux builds are available but may be less polished.

## Download Captures Preview

These links always download the **latest** validated Preview. Pick your platform:

| Platform | Download |
| --- | --- |
| macOS 13+ (Apple silicon) | [Captures-macOS-Apple-Silicon.dmg](https://github.com/joswayski/captures/releases/download/preview/Captures-macOS-Apple-Silicon.dmg) |
| Windows 11 (x64) | [Captures-Windows-x64-setup.exe](https://github.com/joswayski/captures/releases/download/preview/Captures-Windows-x64-setup.exe) |
| Ubuntu / Debian (x64) | [Captures-Linux-x64.deb](https://github.com/joswayski/captures/releases/download/preview/Captures-Linux-x64.deb) |
| Other Linux (x64 AppImage) | [Captures-Linux-x64.AppImage](https://github.com/joswayski/captures/releases/download/preview/Captures-Linux-x64.AppImage) |

Preview builds update after every successful merge to `main` and may contain bugs or incomplete features. You can also open the [Captures Preview — Latest](https://github.com/joswayski/captures/releases/tag/preview) release page, or browse older dated builds in the [build archive](https://github.com/joswayski/captures/releases).

## Features

- Capture regions, windows, or full displays
- Optional countdown before screenshots and recordings
- Record as H.264 video or GIF, with desktop audio and microphone
- Pause, resume, restart, and mute while recording
- Cursor and click highlights in recordings (where supported)
- Built-in screenshot editor — text, shapes, drawing, crop, layers, background removal
- Trim, crop, resize, and adjust audio in recordings
- Export PNG, JPEG, or WebP with compression options
- Mini previews for quick copy, save, and drag into other apps
- Screenshots during an active recording
- 30-day capture history
- Customizable shortcuts and color themes
- Optional in-app feedback (never includes your captures)

## Wishlist

- Scrolling capture for content larger than the screen.
- On-device text recognition (OCR).
- An easy way to repeat the previous capture area.
- Pinned captures that stay visible above other windows.
- Editable click highlights and keystroke overlays after recording.
- Optional hosted sharing for images and recordings with shareable `captur.es/<id>` links.
- Faster recording on Windows and Linux.
- Keep recording controls out of Linux recordings.
- Window, cursor, and click-highlight support on Wayland.

## Platform status

| Platform | Status |
| --- | --- |
| macOS 13+ | Supported; primary development target |
| Windows 11 | Supported; experimental |
| Linux X11 | Supported; recording controls must be hidden manually when needed |
| Linux Wayland | Experimental; no window targeting, cursor capture, or click highlights |

## Build archive

Every successful push to `main` publishes a validated **Preview** build
using [CalVer](https://calver.org/) `YYYY.MM.DD.N`. Each dated version remains
available in the [build archive](https://github.com/joswayski/captures/releases)
for historical testing. Stable releases will use a separate channel when
Captures is ready for launch.

## Shortcuts

| Default shortcut | Action |
| --- | --- |
| `Ctrl+Shift+Space` | Open New Capture |
| `Ctrl+Shift+4` | Capture a region |
| `Ctrl+Shift+W` | Capture a window |
| `Ctrl+Shift+3` | Choose a display for a full-screen screenshot |
| `Ctrl+Shift+5` | Record the screen, then save video or GIF |
| `Ctrl+Shift+F` | Open Send Feedback |
| `Esc` | Cancel an active capture, screenshot countdown, or recording countdown |

Global capture and feedback shortcuts can be changed in Preferences. In the
screenshot editor, zoom with pinch or `Ctrl`/`Cmd`+scroll, pan with
`Ctrl`/`Cmd`-drag or middle-click, and duplicate layers with `Ctrl`/`Cmd`+`D`.

## Development

See [DEVELOPMENT.md](DEVELOPMENT.md) for local setup, validation, and packaging.

## License and trademarks

The source code is licensed under the [Apache License 2.0](LICENSE).

The Captures name and logo are governed by the [Captures Trademark Policy](TRADEMARKS.md) and are not licensed under the Apache License 2.0.
