# Captures

> [!WARNING]
> Captures is a work in progress.

Captures is a cross-platform screen capture utility built for quick captures and a lightweight workflow.

[Visit captur.es](https://captur.es) for the project website and latest development updates.

macOS is the primary development target today. Windows and Linux builds are available, but some behavior is still experimental.

## Features

- Capture a region, window, or full display.
- Record a region, window area, or display on macOS 13+, Windows 11, and Linux, then trim, crop, resize, and save it as H.264 video or an optimized GIF.
- For video, record desktop audio and a selected microphone independently, with pause, resume, restart, mute, interruption recovery, and hideable recording controls.
- Include the pointer and animated click highlights on macOS 15+, Windows, and Linux X11.
- Take a lossless region, window, or display screenshot while a recording continues.
- Capture user-facing Captures windows such as Preferences, Capture History, previews, editors, and the open New Capture controls with a direct screenshot shortcut; transient capture chrome stays out of window-target lists.
- Review recordings on an aspect-correct filmstrip timeline, adjust crop and audio, then save changes to the original or make a named copy, with preserved quality, optional compression, or an exact maximum file size.
- Edit screenshots locally on macOS, Windows, and Linux with formatted text, shapes, arrows, and smoothed freehand drawing; crop, resize, and combine dropped images as selectable, drag-reorderable layers. Copy the result or save a non-destructive PNG, JPEG, or lossless WebP copy with notched quality, a live estimated output size, and optional maximum-file-size controls. Lossless PNG remains the default.
- Open one desktop Capture and Record overlay from the menu bar, Spotlight, or the macOS Dock, switch between screenshot and recording targets in place, or start immediately with customizable global shortcuts.
- Choose from nine color presets—including Mustard and a black-and-white Mono option—or build a custom RGB accent and recording-signal palette in Preferences; changes update every Captures window.
- Copy captures to the clipboard automatically, save them as full-resolution lossless PNGs, or inspect them in a full-size viewer.
- Keep multiple recent captures in a quick-access preview stack, and drag a preview directly into file-upload targets.
- Restore captures from a rolling 30-day local history.
- Launch at login and receive in-app update notifications.

## Roadmap

The roadmap is still taking shape. Likely additions include:

- Hardware-accelerated encoding for the Windows and Linux recording backends.
- Native Wayland window targeting, pointer and click-highlight support, and reliable exclusion of Captures controls from Linux recordings.
- Editable click highlights and keystroke overlays after recording.
- Optional cloud hosting for images and recordings with shareable `captur.es/<id>` links.
- Scrolling capture for content larger than the screen.
- On-device text recognition (OCR).
- Timed captures and an easy way to repeat the previous capture area.
- Pinned captures that stay visible above other windows.
- A **Send Feedback to Developer…** action that lets the user review and submit a short description with a redacted diagnostic summary, including the Captures version, operating system, device architecture, recent in-app action breadcrumbs, and any related crash identifier.
- AI-assisted issue triage that groups incoming reports, proposes root causes and fixes, and can open draft pull requests for human review. Automated reports will never be merged or released without maintainer approval.

These are directions, not promised release dates or a fixed order.

## Platform status

| Platform | Status |
| --- | --- |
| macOS 13+ | Screenshots and H.264/AAC or GIF recording; primary development target |
| Windows 11 | Experimental screenshots and recording; video supports desktop/microphone audio, and video/GIF output supports cursor capture and click highlights |
| Linux X11 | Experimental screenshots and recording; video supports desktop/microphone audio, and video/GIF output supports cursor capture and click highlights |
| Linux Wayland | Partial screenshots and experimental display/region recording; video supports desktop/microphone audio, while window targeting and pointer features remain unavailable |

The Windows and Linux recorders currently use a portable CPU H.264 encoder and
cap output at 4K; they do not yet provide ShadowPlay-style GPU encoding.
Windows captures desktop audio through WASAPI and Linux through PipeWire, while
both platforms expose available microphones in the recording selector. Cursor
capture and click highlights work on Windows and Linux X11. Windows excludes
the recording controls from captured output; on Linux, hide the controls before
recording content underneath them. Wayland asks for a screen again through the
system portal, so choose the same display selected in Captures. Wayland
compositors do not expose the global pointer and button state this implementation
needs, so cursor control and click highlights are disabled there.

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

You will need Rust 1.94 with `rustfmt` and `clippy`, Node.js 24, and npm 11.
macOS builds currently require the macOS 26 SDK for the pinned ScreenCaptureKit bindings; the app's deployment target remains macOS 13.
Windows development also requires the Visual Studio C++ build tools, a Windows
11 SDK, and an MSYS2/MinGW build environment for the bundled media sidecars.
Linux development requires PipeWire and ALSA development packages. On
Debian/Ubuntu, install `libpipewire-0.3-dev`, `libspa-0.2-dev`, and
`libasound2-dev`.

```sh
npm install
npm run prepare:media # first run on each OS, unless the pinned build changes
npm run dev
```

Run the main checks with:

```sh
npm run check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Build an installer or app bundle on the operating system where it will run.
`npm run build` builds and validates the platform's pinned LGPL-only FFmpeg and
ffprobe sidecars before packaging. On macOS it also:

1. Unmounts stale disk images left by earlier builds in this checkout
2. Builds the release bundle
3. Quits any running Captures instance only after the build succeeds
4. Installs `Captures.app` into `/Applications` (replacing any previous copy)
5. Launches Captures

Installers and app bundles are still written under `target/release/bundle` (including the DMG).

```sh
# Build + install + launch (default)
npm run build

# Build only (no /Applications install)
CAPTURES_SKIP_INSTALL=1 npm run build

# Also wipe this app’s Screen Recording TCC record (opt-in; usually not needed)
CAPTURES_RESET_PERMISSIONS=1 npm run build

# Install without auto-launching
CAPTURES_OPEN_AFTER_INSTALL=0 npm run build
```

`npm run build` automatically uses an installed Apple Development signing identity when one is available. Otherwise it warns and seals the bundle with an ad-hoc signature. Local builds omit updater artifacts unless `TAURI_SIGNING_PRIVATE_KEY` is explicitly provided; official release builds always create and sign them. Ad-hoc identity is tied to the exact executable, so macOS asks for Screen Recording permission again after code changes. Stable permissions across upgrades require an Apple Development or Developer ID certificate; the latter can be selected explicitly through `APPLE_SIGNING_IDENTITY`.

Build output is written under `target/release/bundle`. Maintainer release setup and recovery steps live in [docs/releases.md](docs/releases.md).

On Windows, use `npm run dev` to run Captures from the checkout. `npm run build` creates
an NSIS installer `.exe` under `target/release/bundle/nsis`, an `.msi` under
`target/release/bundle/msi`, and the unpackaged executable at `target/release/captures.exe`.
Before compiling, the build stops a running copy at that exact unpackaged path so Windows
can replace it; an installed copy elsewhere is not stopped.

On macOS, recording uses ScreenCaptureKit and Apple's hardware-aware
VideoToolbox H.264 path. Windows and Linux use the operating system capture APIs
exposed by `xcap`, followed by Captures' in-process OpenH264 CPU encoder.
Windows desktop audio uses WASAPI loopback, Linux desktop audio uses PipeWire,
and microphone input uses the platform audio device exposed through CPAL.
FFmpeg is not the live recorder: separately bundled FFmpeg command-line
sidecars synchronize and mux the Windows/Linux video and audio segments, then
handle editing and GIF conversion on every platform. The sidecars are built
without GPL, nonfree, or libx264 components. Build, source-distribution, and
license details live in
[docs/media-sidecars.md](docs/media-sidecars.md).

## License and trademarks

The source code is licensed under the [Apache License 2.0](LICENSE).

The Captures name and logo are governed by the [Captures Trademark Policy](TRADEMARKS.md) and are not licensed under the Apache License 2.0.
