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

Packages are written under `target/release/bundle`. On macOS, the default build also replaces `/Applications/Captures.app` and launches it. Use that Applications copy from Spotlight or Raycast. Checkout bundles in `target/` and Git worktrees are the same app name, so packaging excludes `target/` from Spotlight and moves the leftover `.app` to `target/release/bundle/macos.noindex` after install.

Useful macOS options:

```sh
# Build without installing
CAPTURES_SKIP_INSTALL=1 npm run build

# Install without launching
CAPTURES_OPEN_AFTER_INSTALL=0 npm run build

# Also reset Screen Recording permission
CAPTURES_RESET_PERMISSIONS=1 npm run build
```

macOS `npm run build` uses an installed Apple Development signing identity when available and otherwise uses an ad-hoc signature. Those builds omit updater artifacts unless `TAURI_SIGNING_PRIVATE_KEY` is provided. They also skip Apple notarization and strip quarantine on install, so they are a different Screen Recording identity and a different Gatekeeper path than a downloaded Preview. The bundle includes the Hardened Runtime `audio-input` entitlement so macOS can list Captures in Microphone settings after the app asks.

To iterate on first-run setup against the same Developer ID signature, notarized DMG, and Gatekeeper quarantine a user gets, use the local signed build:

```sh
# One-time: store the App Store Connect API key you already backed up for CI
npm run build:signed -- --setup --key ~/AuthKey_XXXXXXXXXX.p8 --key-id XXXXXXXXXX --issuer <issuer-id>

# Each iteration: sign, notarize, staple, install from the DMG, reset setup
npm run build:signed
```

The Developer ID Application identity name (`Developer ID Application: Your Name (TEAMID)`) is not a secret. `codesign` prints it on every shipped app. The `.p12` private key, its password, and the App Store Connect `.p8` are secrets; `--setup` stores the API key in `~/.captures` and a `captures-notary` keychain profile. GitHub will not give the `release` environment secrets back.

`npm run build:signed` requires macOS and that Developer ID identity in the login keychain. It resets the onboarding flag and Screen Recording / Microphone grants unless you pass `--keep-onboarding` or `--keep-permissions`. Notarization talks to Apple and usually takes a few minutes. It still does not publish a Preview, produce Windows or Linux installers, or exercise the in-app updater.

Useful options:

```sh
npm run build:signed -- --dry-run
npm run build:signed -- --no-launch
npm run build:signed -- --fresh-settings
npm run build:signed -- --skip-notarize
```

Windows builds produce an NSIS installer, MSI package, and unpackaged executable under `target/release`. Linux builds produce AppImage and Debian packages.

## Platform architecture

- macOS recording uses ScreenCaptureKit and VideoToolbox.
- Windows and Linux recording use `xcap` and OpenH264.
- Bundled FFmpeg sidecars handle media synchronization, editing, and GIF conversion.

## Feedback API

Early user feedback is posted to Discord with no database. The same-origin
`/api/*` routes live in the Node server under `apps/web/src/server`.

To run the website and its API locally, create `apps/web/.env` with:

```dotenv
DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/...
```

Then run:

```sh
npm run dev:web
```

The local health endpoint is `http://localhost:5174/api/health`. See
[`apps/web/README.md`](apps/web/README.md) for Railway and Cloudflare setup.

Point a local desktop build at that server:

```sh
export CAPTURES_FEEDBACK_URL=http://localhost:5174/api/feedback
npm run dev
```

Packaged builds default to `https://captur.es/api/feedback`.
