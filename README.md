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

- Capture regions, windows, or full displays.
- Optional screenshot countdown after target selection so you can set up menus and hover states.
- Record regions, windows, or full displays as H.264 video or GIF.
- Record desktop audio and a microphone with pause, resume, restart, and mute controls.
- Configurable countdown before recordings start.
- Show the cursor and click highlights in recordings where supported.
- Edit screenshots with inline text, shapes, multi-point bendable lines and arrows, drawing, crop, resize, zoom, and image layers that snap to edges while moving or resizing. After placing a shape, subtle handles stay available so you can bend lines/arrows or resize without switching tools; drag a mid handle to curve a stroke, and double-click the path to add more curve points (hover shows a tip). Start annotations outside the canvas or release past an edge to expand it; empty chrome around the canvas deselects. Dropped images can snap to a layer edge or stack on top. Layer tools under opacity include lock, visibility, merge down, merge visible, flatten, duplicate, and delete. Trim edges shrinks the canvas to the bounds of visible layers. Remove background clears matching colors (magic wand) or lets you erase and restore alpha on image layers—including the locked capture—then export transparent PNG or WebP.
- Trim, crop, resize, and adjust audio in recordings.
- Save PNGs by default, or export as JPEG or WebP. Compress and maximum-file-size modes keep the selected format (JPEG quality slider for JPEG; stronger packing for PNG).
- Use optional quick-access mini previews to copy, save, preview, or drag recent screenshots into other apps. Mini previews and the recording control bar stay out of captures by default; Preferences can include either when you need to screenshot or record Captures itself.
- Take screenshots without stopping an active recording.
- Restore recent screenshots and recordings from a 30-day Capture History.
- Customize global shortcuts and color themes.
- Send optional product feedback from the app (tray menu, Preferences, or shortcut). Feedback includes your message, optional contact handle, category, and app/system details only — never your screenshots or recordings. Submissions go to the Captures feedback service (not stored as captures on your machine).

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
| `Ctrl+Shift+F` | Open Send Feedback |
| `Esc` | Cancel an active capture, screenshot countdown, or recording countdown |

Global capture and feedback shortcuts can be changed in Preferences.

In the screenshot editor, pinch on a trackpad or hold `Command`/`Ctrl` while
scrolling to zoom around the pointer. Use `Command`/`Ctrl` + `+` or `-` to zoom
from the keyboard, and `Command`/`Ctrl` + `0` to return to 100%. Hold `Command`
(macOS) or `Ctrl` (Windows/Linux) while dragging anywhere on the canvas or
viewport to pan; middle-click drag also pans. If you pan so far that the canvas
leaves the view, a Recenter control fades in at the top of the viewport.
Duplicate the selected layer with `Command`/`Ctrl` + `D`, or use
`Command`/`Ctrl` + `C` and `V` to copy and paste it.

## Development

See [DEVELOPMENT.md](DEVELOPMENT.md) for local setup, validation, and packaging.

## License and trademarks

The source code is licensed under the [Apache License 2.0](LICENSE).

The Captures name and logo are governed by the [Captures Trademark Policy](TRADEMARKS.md) and are not licensed under the Apache License 2.0.
