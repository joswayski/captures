# Captures

Captures is a cross-platform screen capture utility built for quick captures and a lightweight workflow.

> [!WARNING]
> Captures is functional, but still experimental and under active development. macOS is the primary development target; Windows and Linux builds are available but may be less polished.

## Features

- Capture regions, windows, or full displays.
- Record regions, windows, or full displays as H.264 video or GIF.
- Record desktop audio and a microphone with pause, resume, restart, and mute controls.
- Show the cursor and click highlights in recordings where supported.
- Edit screenshots with text, shapes, arrows, drawing, crop, resize, and image layers.
- Trim, crop, resize, and adjust audio in recordings.
- Save lossless PNGs by default, with JPEG and WebP export options.
- Copy, save, preview, or drag recent captures into other apps.
- Take screenshots without stopping an active recording.
- Restore recent screenshots from a 30-day history.
- Customize global shortcuts and color themes.

## Roadmap

The roadmap is still taking shape. Likely additions include:

- Scrolling capture for content larger than the screen.
- On-device text recognition (OCR).
- Timed screenshots and an easy way to repeat the previous capture area.
- Pinned captures that stay visible above other windows.
- Editable click highlights and keystroke overlays after recording.
- Optional hosted sharing for images and recordings.
- Faster recording on Windows and Linux.
- Automatic exclusion of Captures controls on Linux recordings.
- Window, cursor, and click-highlight support on Wayland.

These are directions, not promised release dates or a fixed order.

## Platform status

| Platform | Status |
| --- | --- |
| macOS 13+ | Supported; primary development target |
| Windows 11 | Supported; experimental |
| Linux X11 | Supported; recording controls must be hidden manually when needed |
| Linux Wayland | Experimental; no window targeting, cursor capture, or click highlights |

## Releases

Every successful push to `main` publishes a GitHub Release after its macOS,
Windows, and Linux packages pass the full release validation. The first release
of a New York calendar day ends in `.1`; later releases that day use `.2`, `.3`,
and so on.

Anyone signed in with the GitHub CLI can replace the installed app with the
newest fully validated release for their current system:

```sh
npm run install:latest
```

The command verifies the published checksum, quits and replaces Captures while
preserving local app data, then launches the installed build.

## Shortcuts

| Default shortcut | Action |
| --- | --- |
| `Ctrl+Shift+Space` | Open New Capture |
| `Ctrl+Shift+4` | Capture a region |
| `Ctrl+Shift+W` | Capture a window |
| `Ctrl+Shift+3` | Capture the display under the pointer |
| `Ctrl+Shift+5` | Record the screen, then save video or GIF |
| `Esc` | Cancel an active capture or recording countdown |

All five global capture shortcuts can be changed in Preferences.

## Development

See [DEVELOPMENT.md](DEVELOPMENT.md) for local setup, validation, and packaging.

## License and trademarks

The source code is licensed under the [Apache License 2.0](LICENSE).

The Captures name and logo are governed by the [Captures Trademark Policy](TRADEMARKS.md) and are not licensed under the Apache License 2.0.
