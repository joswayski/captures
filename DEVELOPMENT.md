# Developing Captures

This guide covers local setup, validation, and packaging. Maintainer release procedures live in [docs/releases.md](docs/releases.md), and bundled media details live in [docs/media-sidecars.md](docs/media-sidecars.md).

## Requirements

- Rust 1.94 with `rustfmt` and `clippy`
- Node.js 24 and npm 11
- macOS: macOS 26 SDK
- Windows: Visual Studio C++ build tools, Windows 11 SDK, and MSYS2/MinGW
- Linux: PipeWire and ALSA development packages

On Debian or Ubuntu, install `libpipewire-0.3-dev`, `libspa-0.2-dev`, and `libasound2-dev`.

The optional Rust account API also needs PostgreSQL for database integration
tests. It does not participate in local desktop capture. See
[`apps/api/README.md`](apps/api/README.md) for configuration, database isolation,
migrations and tests, and [`apps/web/README.md`](apps/web/README.md#optional-accounts)
for the account and share pages. Auth/sharing default to disabled; an enabled
environment must supply SES/auth/R2 configuration explicitly. Browser uploads go
directly to presigned R2 multipart part URLs; local captures remain local unless a
user chooses a file on the website. R2 CORS must allow `PUT` from the exact browser
origin, allow `Content-Type`, and expose `ETag`. `npm run check` covers frontend
validation and the public website. Run
`TEST_DATABASE_URL=postgres://USER@127.0.0.1:PORT/postgres cargo test -p captures-api -- --include-ignored`
against a disposable local PostgreSQL server for migration, OTP concurrency,
session, asset authorization, and revocation checks. Tests create and drop their
own databases; SES and object storage are substituted, never live services.
For local browser development, run the API on port 3001 and `npm run dev:web`;
Vite proxies `/api` while share-page SSR uses `CAPTURES_API_ORIGIN` (same local
default). HTTP-only local testing requires `AUTH_INSECURE_LOOPBACK_COOKIE=true`,
a loopback API bind, and `AUTH_ALLOWED_ORIGIN=http://localhost:5174` (or the exact
loopback origin you browse). Production always uses HTTPS and secure cookies.
The website account flow is complete, but no native auth, credential-vault, upload,
or Share-button path is connected. Native integration comes after the rewrite;
the live native hosts and Workbench files retain active ownership of that work.
The offline deployment-notification tests also require Bash and `jq` on PATH
(including on Windows); they intercept HTTP calls and send no Discord messages.

## Cloud sharing with Docker Compose and AWS SSO

This runs the website, Rust API, PostgreSQL and Worker on your machine. It sends
real SES email and uploads to the **real `staging-captures` R2 bucket**, not
production. No native app is required. Install Docker Desktop (or Docker Engine
with Compose v2) and AWS CLI v2; Rust and Node run inside the images.

1. Copy `.env.example` to `.env.local`, without overwriting an existing file,
   and restrict it with `chmod 600 .env.local`. Fill in your AWS SSO profile, SES
   settings, staging R2 credentials and Cloudflare API token. Set `LOCAL_UID` and
   `LOCAL_GID` to the outputs of `id -u` and `id -g` on your host. Generate two
   independent values with `openssl rand -hex 32` for `AUTH_SECRET` and
   `MEDIA_WORKER_SECRET`; keep them stable between restarts.
   The Cloudflare token authenticates Wrangler remote bindings; it is not the
   R2 S3 access key. Use a development token with the account's Workers/R2 access
   required by Wrangler, not a global API key. It never goes to the website.
2. Run `aws sso login --profile YOUR_PROFILE` on the host. The API reads that
   profile and its cached login from a **read-only** `~/.aws` mount. The configured
   UID/GID preserves host file permissions. This mount makes all profiles
   in that directory readable to the API container; use only trusted images.
   A profile relying on a host-only `credential_process` executable is not
   supported by this mount; use your direct SSO profile.
3. In the `staging-captures` bucket's CORS settings, add the rule below, preserving
   existing rules. This is a one-time development-bucket change, not performed
   by Compose. The bucket remains private.

   ```json
   [{
     "AllowedOrigins": ["http://localhost:5174"],
     "AllowedMethods": ["PUT"],
     "AllowedHeaders": ["Content-Type"],
     "ExposeHeaders": ["ETag"],
     "MaxAgeSeconds": 3600
   }]
   ```

4. From the repository root:

   ```sh
   docker compose --env-file .env.local up --build -d
   docker compose --env-file .env.local logs -f api worker web
   ```

   Wait for the API's `captures API listening` and Wrangler's ready message, then
   open **http://localhost:5174/dashboard** on your machine. Use `localhost`, not
   `127.0.0.1`, because the browser origin is exact. Sign in, upload a disposable
   file, open its `/s/<id>` link in an incognito window, test password/Stop sharing,
   and test Trash/Restore. If SES is sandboxed, verify the recipient first.

Compose fixes both upload and download storage to `staging-captures`. Wrangler
runs locally with a remote R2 binding; do not add `--local` (which substitutes
empty simulated storage) or `--remote` (which moves execution off your machine).
Remote binding access incurs normal Cloudflare operations charges. File Trash
retains uploaded objects; deleting the local database does not remove R2 files.

Only port 5174 is published, bound to host loopback. The containers share the
website's network namespace so API, Worker and PostgreSQL can use loopback
without weakening production URL/cookie checks. The API runs migrations against
the named local database before listening. A named volume preserves that database;
the shared development database role is deliberately local-only.

```sh
# Stop containers; retain the database.
docker compose --env-file .env.local down

# After changing source code, rebuild/recreate the local stack.
docker compose --env-file .env.local up --build -d

# If your SSO session expires, log in on the host again and restart the API.
aws sso login --profile YOUR_PROFILE
docker compose --env-file .env.local restart api
```

These are built source snapshots, not bind-mounted hot reload. The first Rust
image build can take several minutes. Use `logs` to diagnose SES/Cloudflare
permissions; do not paste `docker compose config` output because it expands
secrets. No deployment, bucket provisioning, automatic CORS changes or production
database access is performed by this setup. On Windows, use WSL2 with Docker
integration and AWS CLI/SSO configured inside WSL. No wrapper script is required.
Physical macOS/Windows Docker/SSO verification remains
separate from orb checks.

## Setup

```sh
npm install
npm run prepare:media
npm run dev
```

`npm run prepare:media` is required on the first run for each operating system and whenever the pinned media build changes. It downloads the pinned FFmpeg source from ffmpeg.org, or from a previously published Preview if that host is unreachable.

## Design harness

Every Captures window is a `?view=` route on one SPA. `npm run dev --workspace @captures/desktop`
serves that SPA on `http://127.0.0.1:1420` without Tauri, and adding `?mock` installs a
mocked backend with representative sample data so any window can be reviewed in a browser:

```sh
npm run dev --workspace @captures/desktop
open "http://127.0.0.1:1420/?view=preferences&mock=1"
open "http://127.0.0.1:1420/?view=recording-hud&mock=1&stage=1"
open "http://127.0.0.1:1420/?view=recording-hud&mock=1&stage=1&controls=1"
open "http://127.0.0.1:1420/?view=recording-region-indicator&mock=1&stage=1&target=region&x=260&y=180&width=1000&height=640"
open "http://127.0.0.1:1420/?view=thumbnail&mock=1&stage=1"
open "http://127.0.0.1:1420/?view=thumbnail&mock=1&stage=1&placement=top-right"
open "http://127.0.0.1:1420/?view=thumbnail&mock=1&stage=1&reject=1"
open "http://127.0.0.1:1420/?view=update&mock=1"
open "http://127.0.0.1:1420/?view=update&mock=1&captures=1"
open "http://127.0.0.1:1420/?view=update&mock=1&changelog=0"
open "http://127.0.0.1:1420/?view=update&mock=1&update=downloading"
open "http://127.0.0.1:1420/?view=update&mock=1&update=restarting"
open "http://127.0.0.1:1420/?view=update&mock=1&update=error"
open "http://127.0.0.1:1420/?view=startup&mock=1"
open "http://127.0.0.1:1420/?view=startup&mock=1&stage=1&caret=top&caret_x=148"
```

- `mock` installs the sample backend (`apps/desktop/ui/src/dev/previewBackend.ts`).
- `stage` paints a sample desktop behind transparent overlay windows.
- Other parameters set variants: `mode`, `target`, `state`, `update`, `platform`, `granted`, `drafts`, `captures`, `count`, `placement`, `changelog`, `reject`.
- `changelog=0` hides stacked release notes on the update notice (Preferences default is on).
- `caret=top` or `caret=bottom` plus `caret_x` places the tray-pointing triangle on the update and launch notices.
- `placement` sets the mini-preview home corner in the thumbnail and Preferences harness (`top-left`, `top-right`, `bottom-left`, `bottom-right`).
- `reject=1` loops the mini-preview self-drop “no” shake on the newest expanded card.
- `auto=1` enables automatic capture in the selector harness so its compact controls and Preferences link can be reviewed.
- `controls=1` includes recording controls in captures so the will-show copy and Preferences link can be reviewed. Combine with `platform=linux` to review the capture-menu copy that cannot open that setting.
- `live=1` or `frozen=0` shows the capture overlay and recording selector over the live desktop instead of a freeze-frame.
- `screenshot_format` and `video_format` set the Preferences defaults used by the editor harness (`png`/`jpeg`/`webp` and `mp4`/`gif`/`webm`).
- `platform` selects macOS, Windows, or Linux shortcut defaults and copy in the Preferences harness (`?view=preferences&mock=1&platform=windows`). On Linux it also disables recording-control exclusion.
- Appearance follows the `captures-appearance` value in `localStorage`.

The harness is dev-only and is dropped from production builds. Drop an optional
`apps/desktop/ui/public/dev-sample.mp4` (git-ignored) to review the recording editor
with a real clip.

## Design system

Tokens live in `shared/design.css` (neutral ramp, semantic surfaces, spacing, radii,
elevation, motion) and `shared/themes.css` (accent and signal palettes). The desktop
stylesheet is `apps/desktop/ui/src/styles.css`, which imports those tokens plus one
module per family of surfaces from `apps/desktop/ui/src/styles/`.

- Regular windows follow the light/dark/system appearance setting.
- Surfaces that float over the desktop — capture overlay, capture menu, recording
  controls, mini previews, saved/hidden recording notices, the post-update / launch
  tooltip — use the fixed `--glass-*` media palette so they stay legible on any
  wallpaper.
- The update notice is a solid `--surface-raised` card in a transparent native
  window. The launch notice is a dark glass pill with a CSS triangle caret pointing
  at the tray or menu bar icon, not a rotated square.
- Accent is reserved for the primary capture action, selection, and focus. Status
  colors keep stable meanings: signal for recording and destructive, green for saved,
  blue for progress.

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

## Native frontend migration

The browser-free macOS workbench is separate from the shipping Tauri app. On a
Mac with the macOS 26 SDK (Xcode 26+), run `bash apps/native/macos/build.sh`; this generates shared token resources,
builds the Rust settings static library, runs Swift tests and builds the AppKit
executable without installing it. Native Preferences uses a separate Captures Native
development settings file; pass `--settings-file PATH` for disposable tests.
`--exercise` isolates its settings from normal development data. See
[`apps/native/README.md`](apps/native/README.md) for representative scenes,
cold/warm effects, resource collection and known gaps. The
[`docs/native-rewrite.md`](docs/native-rewrite.md) checklist defines the staged
cutover and Windows/Linux prototype gates. Do not remove the existing frontend
or change Preview packaging before those gates pass.

The [Windows/Linux wgpu candidate](apps/native/wgpu/README.md) is an isolated
Rust 1.95 workspace; the shipping app remains on Rust 1.94. From the root, generate
its resources with `node apps/native/prepare.mjs --output apps/native/wgpu/resources`,
then run `cargo +1.95.0 build --manifest-path apps/native/wgpu/Cargo.toml --locked --release`.
Its README covers native build prerequisites, viewport smoke tests, hardware
handoff and resource collection. Root `cargo test --workspace` does not include
this experiment; run its manifest-specific checks too. It connects capture and
recording engines for development but does not select a production renderer.

Native Record requires executable FFmpeg and FFprobe commands on `PATH`; native
builds do not bundle their own media tools yet. AppKit also accepts `CAPTURES_FFMPEG`
and `CAPTURES_FFPROBE` executable paths. Recording uses separate development
History and a sibling `recording-recovery` directory. Do not point tests at real
capture data. The Linux recording acceptance owns a private Xvfb desktop and D-Bus
session; install the windowing dependencies from the wgpu README plus `ffmpeg`
and `python3-xlib`, then run:

```sh
/usr/bin/python3 apps/native/x11_recording_smoke.py \
  --binary apps/native/wgpu/target/release/captures-wgpu-workbench \
  --output /tmp/native-x11-recording
```

Pass `--hide-controls-only` for focused running/paused Hide checks through a real
Xfce SNI tray, configured New Capture shortcut restoration, tray-host-loss recovery,
finalized media decode and recovery cleanup.

Pass `--ready-notice-only --appearance dark` (and repeat with `light`) to exercise
real recording finalization followed by notice save failure/retry, byte-identical
export, missing-file reveal, expiry with a hidden root, dismissal and cleanup
before another capture. The test intercepts only the `xdg-open` launcher to check
its path without opening a file manager. It never uses installed capture data.

Add `--virtual-microphone` when PulseAudio, `pactl`, `paplay`, and the ALSA Pulse
plugin are installed. The test feeds a 730 Hz tone into a disposable null-sink
monitor, drives running mute/unmute through the real HUD, checks durable segment
boundaries and finalized microphone metadata, then decodes AAC to assert audible,
silent, and audible intervals. It waits for actual PCM delivery after Unmute,
not just stream startup. This verifies synthetic audio routing, not physical
microphone fidelity or gapless device/encoder transitions.

Pass `--restart-only` for the focused running/paused Restart, restarted-countdown
Escape, replacement-pixel decode and source-cleanup checkpoint.

The output directory must not exist. The test drives real selector/HUD input,
checks pause/resume plus running/paused restart, decodes replacement-only MP4
pixels with FFmpeg, verifies History and source cleanup, and distinguishes initial
and restarted countdown cancellation, running Escape,
explicit discard, session-loss preservation, HUD close and whole-application quit.
Session lock is simulated;
physical keyboard/display/audio, permissions and hardware compositor acceptance
remain separate gates. Inspect its selector/HUD PNGs as well as the test result.

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

Installed packages register **Open With** for PNG, JPEG, WebP, GIF, MP4, and WebM (`bundle.fileAssociations` in `apps/desktop/src-tauri/tauri.conf.json`, rank Alternate so Captures is not the default app). `npm run dev` does not. Debian and RPM packages use `apps/desktop/src-tauri/linux/captures.desktop` so the launcher receives those files (`Exec=… %U`). AppImage builds still need a desktop entry with `%U` for Open With to work.

## Platform architecture

- macOS recording uses ScreenCaptureKit and VideoToolbox.
- Windows and Linux recording use `xcap` and OpenH264.
- Bundled FFmpeg sidecars handle media synchronization, editing, and GIF conversion.

## Feedback API

Early user feedback is posted to Discord with no database. Public feedback and
Preview updater requests are handled by the Rust `captures-api` service.

Set the API process environment before starting it:

```dotenv
DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/...
```

The variable is optional at startup, but feedback returns 503 when it is absent.
Invalid or non-Discord webhook URLs are rejected without logging their contents.

Point a local desktop build at that server:

```sh
export CAPTURES_FEEDBACK_URL=http://localhost:5174/api/feedback
npm run dev
```

Packaged builds default to `https://captur.es/api/feedback`.
