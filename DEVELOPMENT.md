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
[`apps/api/README.md`](apps/api/README.md) for configuration, schema isolation,
migrations and tests, and [`apps/web/README.md`](apps/web/README.md#optional-accounts)
for the account placeholder. Sign-in is unavailable. `npm run check` verifies
that account requests fail closed and the built public website still works.
The offline deployment-notification tests also require Bash and `jq` on PATH
(including on Windows); they intercept HTTP calls and send no Discord messages.

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

### Optional native UI experiment

The Windows port shares the full GTK3/Cairo frontend with the Linux experiment,
not the minimal probe. See the [Windows report](docs/windows-native-implementation.md)
and [Windows build/benchmark instructions](experiments/native-ui/windows/README.md).
Use MSYS2 MINGW64, Rust 1.94's `x86_64-pc-windows-gnu` target, and
`bash experiments/native-ui/windows/build.sh`. Its native-only feature set does
not link Tauri/WebView2. The shipping app and its packages remain unchanged.

The [native UI evaluation](docs/native-ui-evaluation.md) explains the scope and
limitations. The standalone `experiments/native-ui` workspace is not included in
the normal gates, Preview packaging, or default Rust workspace. It contains two
minimal controls and the expanded `captures-linux-native` app. Build with the usual
Linux Tauri dependencies and FFmpeg/ffprobe/ffplay on PATH (the native app currently uses
system media tools rather than the shipping app's packaged sidecars):

```sh
CARGO_BUILD_JOBS=1 cargo build --release --locked --manifest-path experiments/native-ui/Cargo.toml
cargo fmt --manifest-path experiments/native-ui/Cargo.toml -- --check
CARGO_BUILD_JOBS=1 cargo test --release --locked --manifest-path experiments/native-ui/Cargo.toml
CARGO_BUILD_JOBS=1 cargo clippy --release --locked --manifest-path experiments/native-ui/Cargo.toml --all-targets -- -D warnings
experiments/native-ui/target/release/captures-linux-native
# Original minimal controls, run separately:
experiments/native-ui/target/release/captures-gtk-probe
# Or, in a separate run:
experiments/native-ui/target/release/captures-tauri-probe
```

The expanded app requires X11 and offers a display picker. See the
[implementation and parity report](docs/linux-native-implementation.md) before
using it for anything important. Tray, X11 global shortcuts, single-instance
routing and opt-in login startup are implemented; installer and updater are not.
Launch without arguments opens Preferences, as in the shipping app; there is no
separate native launcher screen. Closing a window leaves the app running. Use
**Quit Captures** in the tray menu to exit; finish/save recording first.
Its independent data directory is `$XDG_DATA_HOME/captures-linux-native` (normally
`~/.local/share/captures-linux-native`). Override with `CAPTURES_NATIVE_DATA` for
disposable runs. Settings and captures do not migrate from the Tauri application.
Capture/Export creates new files. The image editor's explicit **Save** overwrites
the imported source; drafts automatically restore on reopening the same file.
Recording crash drafts are offered for recovery at startup and from History.
Use `--capture`, `--record`, or `--gif` for the shared target selector;
`--open /absolute/file.png`, `--open /absolute/file.mp4`, `--preferences`,
`--history`, or `--previews /absolute/a.png /absolute/b.png` for a specific window.
`--canvas` opens the experiment's additional blank-canvas entry point.

The expanded app's automated checks use a disposable X11 desktop, compositor,
AT-SPI, and a test-only ScreenSaver DBus authority; the app itself still executes
the real fail-closed session checks. Never run that fixture in your personal DBus
session. `openbox`, `xcompmgr`, `python3-dbus`, `python3-gi`, `python3-pyatspi`,
`xdotool`, `xvfb`, and `xauth` are included in orb setup; ImageMagick and FFmpeg are
also required. Use `/usr/bin/python3` so it finds Debian's GI/AT-SPI modules.
The lab extracts the neutral SVG from the React harness's `sampleCapture` and
renders it through GdkPixbuf's SVG loader. Keep `apps/desktop/ui/src/dev/previewBackend.ts`
with the experiment when transferring a worktree; no screenshot of existing app
controls is used as the test image.

```sh
# In an orb, keep the isolated desktop supervised:
amp orb service start captures-native-lab --command 'dbus-run-session -- xvfb-run -a -s "-screen 0 1280x800x24" /usr/bin/python3 experiments/native-ui/native_desktop.py /tmp/captures-native-lab'
/usr/bin/python3 experiments/native-ui/native_check.py \
  --lab /tmp/captures-native-lab --artifacts .amp/in/artifacts/linux-native-parity

# The suite runs capture/preferences, image editor, previews, history and
# recording checks sequentially. Audio tests need a disposable Pulse server
# with a monitor source (export PULSE_SERVER to that server before running).
# Individual checks also accept --lab/--artifacts; history_check needs only --lab.
# preview_check additionally requires --binary /absolute/path/to/captures-linux-native.

# Build the unchanged Tauri source app with its production assets first:
npm run build --workspace @captures/desktop
CARGO_BUILD_JOBS=1 cargo build --release --locked -p captures-desktop --bin captures \
  --features tauri/custom-protocol --target-dir experiments/native-ui/target
ffmpeg -nostdin -y -loop 1 -framerate 25 -i /tmp/captures-native-lab/fixture.png \
  -frames:v 100 -threads 2 -c:v libx264 -pix_fmt yuv420p /tmp/captures-native-lab/fixture.mp4

# No builds, browser automation, or UI smoke tests concurrently with measurement.
/usr/bin/python3 experiments/native-ui/native_comparison.py \
  --lab /tmp/captures-native-lab --artifacts .amp/in/artifacts/linux-native-parity \
  > /tmp/linux-native-features.json
```

Inspect the measured Preferences/image-editor images before accepting the numbers.
The timing is **first mapped window, not UI/content readiness**. Raw samples cover
unequal feature completeness; they do not establish recording or energy gains.
Memory/CPU samples use matched 980×720 Preferences and 1280×760 editor client areas,
resized after the first-mapped-window timestamp and before the settling interval.
The lab removes window-manager decoration constraints for both apps after mapping
and checks capture dimensions; Openbox otherwise clips a full-width GTK client.
`--inspect` also captures the video editor, which is excluded from the measured
states because the actual Tauri player could not decode the fixture in this orb.
The isolated compositor uses `xcompmgr -n`: alpha compositing without synthetic
whole-window shadows. `xcompmgr -c` produced oversized rectangular shadows around
otherwise transparent native overlays; GNOME/KDE compositor behavior needs its
own verification. This setting applies equally to both measured applications.

To reproduce the labelled review gallery, capture `before-*.png` from the dev
harness: `recording-selector` for menu/region/window (set `target`, drag/select as
appropriate), `thumbnail` for expanded/collapsed previews (340×760 CSS viewport at
DPR 2; wait for the transition and capture both idle and hovered cards),
`recording-hud` for controls, `history`, and
`recording-editor&artifact_id=recording-1` with the local sample clip decoded.
Use `mock=1&stage=1&platform=linux`. Preferences and image-editor comparisons use
the actual-app captures from `native_comparison.py`, not the browser harness.

```sh
/usr/bin/python3 experiments/native-ui/native_gallery.py \
  --artifacts .amp/in/artifacts/linux-native-parity \
  --before-artifacts .amp/in/artifacts/linux-native \
  --output .amp/in/artifacts/linux-native-review
amp orb service start captures-native-review --command 'python3 -m http.server "$PORT" --bind 0.0.0.0 --directory .amp/in/artifacts/linux-native-review' --portal
```

Capture includes the probe window. **Save PNG** creates numbered files in
`$TMPDIR/captures-ui-probe` (normally `/tmp/captures-ui-probe` on Linux); set
`CAPTURES_PROBE_OUTPUT` to use another directory. These are disposable experimental
captures, separate from Captures history. The minimal GTK probe is Linux-only;
the full native frontend also has an experimental Windows port. A macOS native
frontend is not implemented.

For the reproducible Linux warm-launch/idle benchmark, install `xvfb`, `xauth`,
and `xdotool` (included in orb setup) and use a disposable display/DBus session:

```sh
LIBGL_ALWAYS_SOFTWARE=1 xvfb-run -a -s '-screen 0 1280x800x24' \
  dbus-run-session -- sh -c 'python3 experiments/native-ui/benchmark.py > /tmp/native-ui-results.json'
```

Redirect inside the DBus session as shown: accessibility-service diagnostics can
otherwise mix with the JSON on the wrapper's stdout.

Do not run builds or other CPU-heavy work during measurement. Results cover the
two minimal shells, not the full React application. Keep the JSON's individual
samples alongside any reported summary. See the evaluation for readiness
milestones, memory accounting, and excluded costs.

The real-window smoke test also requires ImageMagick (`import`, `identify`, and
`convert`). It clicks Capture and Save, validates the PNG dimensions/content and
repeat-save bytes, and captures empty, preview, saved, and save-error states:

```sh
LIBGL_ALWAYS_SOFTWARE=1 xvfb-run -a -s '-screen 0 1280x800x24' \
  dbus-run-session -- python3 experiments/native-ui/smoke.py \
  --artifacts .amp/in/artifacts/native-ui
```

Inspect the resulting images; saving screenshots alone does not verify rendering.
The preview's black right/bottom areas are the empty Xvfb desktop, not corruption.

For a separate current-app Preferences reference, build its embedded production
UI, then run with disposable settings/history and an unavailable outbound HTTP
proxy. **The `tauri/custom-protocol` feature is required when bypassing the Tauri
CLI**; without it the app can point at its development URL even in a release build.

```sh
npm run build --workspace @captures/desktop
CARGO_BUILD_JOBS=1 cargo build --release --locked -p captures-desktop --bin captures --features tauri/custom-protocol
LIBGL_ALWAYS_SOFTWARE=1 xvfb-run -a -s '-screen 0 1280x800x24' \
  dbus-run-session -- sh -c 'python3 experiments/native-ui/production_reference.py \
  target/release/captures --artifacts .amp/in/artifacts/native-ui \
  > /tmp/current-app-reference.json'
```

Inspect `production-preferences.png` before accepting its numbers: a native window
title can appear even when the frontend failed to load. This is three separate
empty-profile runs, each settled for ten seconds after Preferences becomes
visible, followed by five seconds of idle CPU sampling. It is not a matched
workload or a measurement of the packaged Preview, recording, or hidden-tray idle.

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
