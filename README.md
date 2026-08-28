# Captures

Captures is a cross-platform screen capture utility built for quick captures and a lightweight workflow.

> [!WARNING]
> Experimental and under active development. See platform status below.

## A quick look

Click and drag a region like other screenshot tools, or pick a window or full display from one compact menu.

![Captures region capture over a landscape, with a highlighted box, corner handles, and the capture menu](docs/images/capture-selection.jpg)

<p align="center">
  <img src="docs/images/capture-controls.jpg" alt="Close-up of the Captures capture menu with screenshot and recording options" width="728">
  <br>
  <sub><strong>Everything in reach</strong> with capture type and target controls together in one compact menu.</sub>
</p>

<table>
  <tr>
    <td width="50%">
      <img src="docs/images/screenshot-editor.jpg" alt="Captures screenshot editor with a landscape, a Choke point label on the fjord, a tiger sticker half off the left edge, and an Evergreen ship dragged past the right edge" width="100%">
      <br>
      <sub><strong>Edit screenshots</strong> with text, arrows, stickers, and a canvas that grows when you drag past the edge.</sub>
    </td>
    <td width="50%">
      <img src="docs/images/video-editor.jpg" alt="Captures video editor with a landscape preview, crop handles, trimming timeline, and save controls" width="100%">
      <br>
      <sub><strong>Polish recordings</strong> with preview, trimming, crop and size controls, and export options.</sub>
    </td>
  </tr>
  <tr>
    <td colspan="2" align="center">
      <img src="docs/images/preferences.jpg" alt="Captures Preferences showing the appearance, accent color, and capture settings" width="72%">
      <br>
      <sub><strong>Make it yours</strong> with a light or dark appearance, accent colors, shortcuts, and capture and recording defaults.</sub>
    </td>
  </tr>
</table>

## Download Captures Preview

These links always download the **latest** validated Preview:

| Platform | Download |
| --- | --- |
| macOS 13+ (Apple silicon) | [Captures-macOS-Apple-Silicon.dmg](https://github.com/joswayski/captures/releases/download/preview/Captures-macOS-Apple-Silicon.dmg) |
| Windows 11 (x64) | [Captures-Windows-x64-setup.exe](https://github.com/joswayski/captures/releases/download/preview/Captures-Windows-x64-setup.exe) |
| Ubuntu / Debian (x64) | [Captures-Linux-x64.deb](https://github.com/joswayski/captures/releases/download/preview/Captures-Linux-x64.deb) |
| Other Linux (x64 AppImage) | [Captures-Linux-x64.AppImage](https://github.com/joswayski/captures/releases/download/preview/Captures-Linux-x64.AppImage) |

Preview builds update after every successful merge to `main` and may contain bugs or incomplete features. Installed copies can update in the app; the notice lists every Preview published since the version you have, then installs the latest. Older dated builds stay in the [build archive](https://github.com/joswayski/captures/releases).

## Features

- Capture regions, windows, or full displays
- Draw a region from an empty screen (no pre-sized outline); lock to common aspect ratios, or hold Shift for a square
- Optional auto-start after selecting a region or window (Preferences)
- Optional countdown before screenshots and recordings
- Record as H.264 video or GIF, with desktop audio and microphone
- Pause, resume, restart, and mute while recording
- Cursor and click highlights in recordings (where supported)
- Built-in screenshot editor — text, shapes, drawing with optional drop shadows, crop (drag from outside the canvas to reach an edge; hold Shift to lock aspect), layers, erase to transparent; unsaved edits restore when you reopen
- Trim, crop, resize, and adjust audio in recordings, with an estimated saved size and an in-editor before/after compression comparison
- Export PNG, JPEG, or WebP with Tiny through High quality presets and an in-editor before/after comparison
- Mini previews for quick copy, save, and drag into other apps
- Screenshots during an active recording
- 30-day capture history, filtered by screenshots, video, or GIF
- Light, dark, or system appearance across every Captures window
- Customizable shortcuts and accent colors
- Capture UI and capture actions stay disabled while the desktop session is locked or inactive
- Optional in-app feedback (never includes your captures)

## Wishlist

- Scrolling capture for content larger than the screen
- On-device text recognition (OCR)
- Repeat the previous capture area
- Pinned captures that stay above other windows
- Editable click highlights and keystroke overlays after recording
- Hosted sharing with shareable `captur.es/<id>` links
- Faster recording on Windows and Linux

## Platform status

| Platform | Status |
| --- | --- |
| macOS 13+ | Supported; primary development target |
| Windows 11 | Supported; experimental |
| Linux X11 | Supported; hide recording controls manually when needed |
| Linux Wayland | Experimental; no window targeting, cursor capture, or click highlights |

## Shortcuts

Defaults follow each platform’s built-in screenshot keys. Captures-only actions keep extra shortcuts.

### macOS

| Default shortcut | Action |
| --- | --- |
| `Cmd`+`Shift`+`Space` | Open New Capture |
| `Cmd`+`Shift`+`4` | Capture a region |
| `Cmd`+`Shift`+`W` | Capture a window |
| `Cmd`+`Shift`+`3` | Choose a display for a full-screen screenshot |
| `Cmd`+`Shift`+`5` | Record the screen, then save video or GIF |
| `Cmd`+`Shift`+`F` | Open Send Feedback |
| `Esc` | Cancel an active capture, screenshot countdown, or recording countdown |

### Windows

| Default shortcut | Action |
| --- | --- |
| `Ctrl`+`Shift`+`Space` | Open New Capture |
| `Win`+`Shift`+`S` | Capture a region |
| `Alt`+`PrtScn` | Capture a window |
| `PrtScn` | Choose a display for a full-screen screenshot |
| `Win`+`Alt`+`R` | Record the screen, then save video or GIF |
| `Ctrl`+`Shift`+`F` | Open Send Feedback |
| `Esc` | Cancel an active capture, screenshot countdown, or recording countdown |

### Linux (GNOME / Ubuntu)

| Default shortcut | Action |
| --- | --- |
| `PrtScn` | Open New Capture |
| `Super`+`Shift`+`S` | Capture a region |
| `Alt`+`PrtScn` | Capture a window |
| `Shift`+`PrtScn` | Choose a display for a full-screen screenshot |
| `Ctrl`+`Shift`+`Alt`+`R` | Record the screen, then save video or GIF |
| `Ctrl`+`Shift`+`F` | Open Send Feedback |
| `Esc` | Cancel an active capture, screenshot countdown, or recording countdown |

Global capture and feedback shortcuts can be changed in Preferences. Installations still on earlier factory defaults (`Ctrl`+`Shift` or the shared macOS-style number keys) are updated automatically; custom shortcuts stay as they are.

On macOS, overlapping Screenshot app shortcuts (`Cmd`+`Shift`+`3` / `4` / `5`) are turned off so those keys reach Captures; restore them in System Settings → Keyboard → Keyboard Shortcuts → Screenshots if you want both. On GNOME, overlapping screenshot keybindings are cleared when `gsettings` is available. On Windows, Print Screen is turned off for Snipping Tool when Captures uses that key; `Win`+`Shift`+`S` may still open Snipping Tool until you change it in Windows keyboard settings.

While selecting a capture region, pick an aspect ratio in the capture menu or hold
`Shift` for a square. In the screenshot editor, zoom with the header slider and
`+`/`-` controls, pinch or `Ctrl`/`Cmd`+scroll, pan with `Ctrl`/`Cmd`-drag or
middle-click, hold `Shift` while dragging a corner handle to scale
proportionally, and duplicate layers with `Ctrl`/`Cmd`+`D`. Header W×H resizes
the canvas. Locked layers keep size and position until unlocked; layer width
and height stay proportional.

## Development

See [DEVELOPMENT.md](DEVELOPMENT.md) for local setup, validation, and packaging.

## License and trademarks

The source code is licensed under the [Apache License 2.0](LICENSE).

The Captures name and logo are governed by the [Captures Trademark Policy](TRADEMARKS.md) and are not licensed under the Apache License 2.0.
