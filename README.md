# Captures

Captures is a work-in-progress, cross-platform screen capture utility. It is built for quick captures, a lightweight workflow, and privacy by default.

macOS is the primary development target today. Windows and Linux builds are available, but some behavior is still experimental.

## Features

- Capture a region, window, or full display.
- Start captures from the tray or customizable global shortcuts.
- Copy captures to the clipboard automatically, save them as PNGs, or inspect them in a full-size viewer.
- Keep multiple recent captures in a quick-access preview stack.
- Restore captures from a private, rolling 30-day local history.
- Launch at login and receive in-app update notifications.
- Keep captures local today, with no uploads or analytics.

## Roadmap

The roadmap is still taking shape. Likely additions include:

- GIF and video recording, with microphone/system audio and cursor controls.
- Screenshot markup and editing, plus trimming for recordings.
- Full feature parity across macOS, Windows, and Linux.
- Optional cloud hosting for images and recordings with shareable `captur.es/<id>` links.
- Scrolling capture for content larger than the screen.
- On-device text recognition (OCR).
- Timed captures and an easy way to repeat the previous capture area.
- Pinned captures that stay visible above other windows.

These are directions, not promised release dates or a fixed order.

## Platform status

| Platform | Status |
| --- | --- |
| macOS 13+ | Primary development target |
| Windows 11 | Experimental |
| Linux X11 | Experimental |
| Linux Wayland | Partial support through the desktop screenshot portal and XWayland |

## Download

Download the latest builds from [GitHub Releases](https://github.com/joswayski/captures/releases/latest). Captures is still early software, so expect rough edges.

## Shortcuts

| Default shortcut | Action |
| --- | --- |
| `Ctrl+Shift+4` | Capture a region |
| `Ctrl+Shift+W` | Capture a window |
| `Ctrl+Shift+3` | Capture the display under the pointer |
| `Esc` | Cancel an active region or window capture |

The three global capture shortcuts can be changed in Preferences.

## Development

You will need Rust 1.94 with `rustfmt` and `clippy`, Node.js 24, and npm 11.

```sh
npm install
npm run dev
```

Run the main checks with:

```sh
npm run check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Build an installer or app bundle on the operating system where it will run:

```sh
npm run build
```

Build output is written under `target/release/bundle`. Maintainer release setup and recovery steps live in [docs/releases.md](docs/releases.md).

## Privacy

Captures stores pending previews and its 30-day recovery history locally. It does not upload captures or send telemetry. Release builds contact GitHub Releases only to check for and download application updates.

Future cloud sharing will be optional and explicit.

## License and trademarks

The source code is licensed under the [Apache License 2.0](LICENSE).

The Captures name and logo are governed by the [Captures Trademark Policy](TRADEMARKS.md) and are not licensed under the Apache License 2.0.
