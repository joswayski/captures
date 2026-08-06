# Developing Captures

This guide covers local setup, validation, and packaging. Maintainer release procedures live in [docs/releases.md](docs/releases.md), and bundled media details live in [docs/media-sidecars.md](docs/media-sidecars.md).

## Requirements

- Rust 1.94 with `rustfmt` and `clippy`
- Node.js 24 and npm 11
- macOS: macOS 26 SDK
- Windows: Visual Studio C++ build tools, Windows 11 SDK, and MSYS2/MinGW
- Linux: PipeWire and ALSA development packages

On Debian or Ubuntu, install `libpipewire-0.3-dev`, `libspa-0.2-dev`, and `libasound2-dev`.

## Setup

```sh
npm install
npm run prepare:media
npm run dev
```

`npm run prepare:media` is required on the first run for each operating system and whenever the pinned media build changes.

## Validation

Run the default repository gate:

```sh
npm run check
```

For Rust changes, also run:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Packaging

Build Captures on the operating system where the package will run:

```sh
npm run build
```

Packages are written under `target/release/bundle`. On macOS, the default build also replaces `/Applications/Captures.app` and launches it.

Useful macOS options:

```sh
# Build without installing
CAPTURES_SKIP_INSTALL=1 npm run build

# Install without launching
CAPTURES_OPEN_AFTER_INSTALL=0 npm run build

# Also reset Screen Recording permission
CAPTURES_RESET_PERMISSIONS=1 npm run build
```

macOS builds use an installed Apple Development signing identity when available and otherwise use an ad-hoc signature. Local builds omit updater artifacts unless `TAURI_SIGNING_PRIVATE_KEY` is provided.

Windows builds produce an NSIS installer, MSI package, and unpackaged executable under `target/release`. Linux builds produce AppImage and Debian packages.

## Platform architecture

- macOS recording uses ScreenCaptureKit and VideoToolbox.
- Windows and Linux recording use `xcap` and OpenH264.
- Bundled FFmpeg sidecars handle media synchronization, editing, and GIF conversion.

## Feedback API

Early user feedback is posted to Discord by a small Rust service under `apps/api`
(no database). Create a channel webhook in Discord, then:

```sh
export DISCORD_WEBHOOK_URL='https://discord.com/api/webhooks/...'
cargo run -p captures-api
```

Point a local desktop build at that service:

```sh
export CAPTURES_FEEDBACK_URL=http://127.0.0.1:8080/api/feedback
npm run dev
```

Packaged builds default to `https://api.captur.es/api/feedback`. Docker notes live in [apps/api/README.md](apps/api/README.md).
