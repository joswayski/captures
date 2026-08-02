# Captures

Captures is a cross-platform screen capture utility built for quick captures and a lightweight workflow.

> [!WARNING]
> Captures is functional, but still experimental and under active development. macOS is the primary development target; Windows and Linux builds are available but may be less polished.

## Download Captures Preview

[**Download the latest Captures Preview**](https://github.com/joswayski/captures/releases/tag/preview)

Choose the macOS `.dmg`, Windows `.exe`, Ubuntu/Debian `.deb`, or Linux
AppImage from that page. The Preview updates after every successful merge and
may contain bugs or incomplete features.

## Features

- Capture regions, windows, or full displays.
- Optional countdown before screenshots so you can set up menus and hover states.
- Record regions, windows, or full displays as H.264 video or GIF.
- Record desktop audio and a microphone with pause, resume, restart, and mute controls.
- Configurable countdown before recordings start.
- Show the cursor and click highlights in recordings where supported.
- Edit screenshots with text, shapes, arrows, drawing, crop, resize, zoom, and image layers that snap to edges while moving or resizing (release past a canvas edge to expand it). Dropped images can snap to a layer edge or stack on top.
- Trim, crop, resize, and adjust audio in recordings.
- Save PNGs by default, or export as JPEG or WebP. Compress and maximum-file-size modes keep the selected format (JPEG quality slider for JPEG; stronger packing for PNG).
- Use optional quick-access mini previews to copy, save, preview, or drag recent screenshots into other apps.
- Take screenshots without stopping an active recording.
- Restore recent screenshots and recordings from a 30-day Capture History.
- Customize global shortcuts and color themes.

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
| `Ctrl+Shift+3` | Capture the display under the pointer |
| `Ctrl+Shift+5` | Record the screen, then save video or GIF |
| `Esc` | Cancel an active capture, screenshot countdown, or recording countdown |

All five global capture shortcuts can be changed in Preferences.

In the screenshot editor, pinch on a trackpad or hold `Command`/`Ctrl` while
scrolling to zoom around the pointer. Use `Command`/`Ctrl` + `+` or `-` to zoom
from the keyboard, and `Command`/`Ctrl` + `0` to return to 100%.

## Development

See [DEVELOPMENT.md](DEVELOPMENT.md) for local setup, validation, and packaging.

## License and trademarks

The source code is licensed under the [Apache License 2.0](LICENSE).

The Captures name and logo are governed by the [Captures Trademark Policy](TRADEMARKS.md) and are not licensed under the Apache License 2.0.
