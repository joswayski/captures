# Developing Captures

This guide covers local setup, validation, and packaging. Maintainer release procedures live in [docs/releases.md](docs/releases.md), and bundled media details live in [docs/media-sidecars.md](docs/media-sidecars.md).

## Requirements

- Rust 1.94 with `rustfmt` and `clippy`
- Node.js 24 and npm 11
- uv 0.12.23 and an existing Python interpreter (CI uses Python 3.12;
  Linux native smokes use the distro interpreter and its bindings)
- macOS: macOS 26 SDK
- Windows: Visual Studio C++ build tools, Windows 11 SDK, and MSYS2/MinGW
- Linux: PipeWire and ALSA development packages

On Debian or Ubuntu, install `libpipewire-0.3-dev`, `libspa-0.2-dev`, and `libasound2-dev`.

The optional Rust account API also needs PostgreSQL for database integration
tests. It does not participate in local desktop capture. See
[`apps/api/README.md`](apps/api/README.md) for configuration, database isolation,
migrations and tests, and [`apps/web/README.md`](apps/web/README.md#optional-accounts)
for the account placeholder. Sign-in is unavailable. `npm run check` verifies
that account requests fail closed and the built public website still works.

Both native development hosts connect `captures-account::native::Worker` to
their sharing windows; AppKit uses an opaque, nonblocking C ABI. The worker owns the
selected original bytes, OTP challenge and account session across popup/preview
closure. Opening or signing in never uploads. At the lower-level client boundary,
construct `AccountClient::production()` on a serialized worker without
side effects; only after the user opens Share, explicitly call `load`, `request_code`,
`verify`, `me`, `retry_save`, or `logout`. The host owns email/code controls and
selected artifact state; the client owns only the bearer session. If `verify`
returns a vault error, the accepted user/token stay in memory: call `retry_save`
without reusing the one-time code. A 401 invalidates the session; if vault removal
fails, retry `clear_invalid`. Offline/503 leave a valid token alone. Logout revokes
remotely before deleting locally; if revocation fails, retry logout. There is no
refresh flow. The account API is still disabled by default; native development
controls do not activate it. Run `cargo test -p captures-account` for the disposable loopback HTTP
and injected-vault contract tests; never use staging Compose (real SES/R2).
Custom origins require an injected vault via `AccountClient::new`; the real OS
vault is bound to the canonical API and cannot be paired with a mock/staging URL.

For the underlying upload coordinator, construct `SharingCoordinator::new(&mut account,
AssociationStore::new(native_profile_root))` without network or file access.
`open(artifact_id)` explicitly checks the current account and remote list but never
uploads. Resolve a stable original History file and call `upload` only after the
user confirms; pass a cancellation flag and progress callback. The coordinator
streams bounded chunks directly to presigned object-store URLs without bearer or
cookies. It persists per-account/per-profile artifact associations, part ETags and
ready state atomically. `configure_share` is a separate explicit operation; only
its successful response supplies a usable share ID. `Patch::Keep` omits a field,
`Patch::Clear` sends null and `Patch::Set` sends a value. `trash`/`restore` preserve
the association; restore never revives an old share. A failed share configuration
can be retried without re-upload. Before create, the coordinator persists a UUID
and immutable metadata, then uses `PUT /api/asset-uploads/{UUID}` from the accounts
API (#613/#806). Retry `upload` after a lost response or repaired local storage:
the same key recovers the same asset even across process restart. Conflict (409)
and tombstone (410) retain the key rather than silently creating a replacement.
Legacy checkpoints without a key remain blocked for explicit `recover_created`
reconciliation; never clear their marker or guess an ID. The host must serialize
access to the profile and retain its accepted upload worker after the preview
closes. Disposable loopback fixtures/fake vaults, wgpu UI/accessibility tests and
software-rendered X11 fixtures have been exercised. AppKit tests cover command
selection, retained forms, capture hiding and minimum-size light/dark fixtures;
all seven passed in macOS CI, and the light/dark minimum-size renders were
inspected. Real
SES/R2, physical vaults and physical macOS/Windows/X11/Wayland acceptance remain open.

On macOS, `--scene sharing` is a disabled render fixture (not `--live` or
`--exercise`). Set `CAPTURES_NATIVE_SHARE_FIXTURE` to `otp`, `vault`, `shared`,
`uploading`, `trash` or `error` for nondefault states. `CAPTURES_TEST_ARTIFACTS`
collects XCTest light/dark, normal/minimum-size sharing renders. Neither fixture
switch changes the live API/vault or permits sign-in, uploads, Copy/Open or Cancel.

`OsVault` uses a separate `es.captur.native.account` credential in macOS Keychain,
Windows Credential Manager, or Linux Secret Service (with encrypted D-Bus transport),
not settings files or the installed Tauri identity. Missing credentials mean signed
out; locked/inaccessible and unavailable credential services are distinct errors.
Linux desktop sessions need an active Secret Service collection. Physical vault
unlock, persistence/restart and access behavior remain unverified on macOS,
Windows, X11 and Wayland; root CI compiles/tests the platform-gated adapters on
macOS, Windows and Ubuntu, but a green fake-vault test does not close acceptance.

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
  at the tray or menu bar icon, not a rotated square. Without reliable icon geometry,
  it uses a compact, left-aligned rounded rectangle instead. Its 296px width and
  54px minimum height do not stretch with the native window; long shortcuts can wrap.
- Accent is reserved for the primary capture action, selection, and focus. Status
  colors keep stable meanings: signal for recording and destructive, green for saved,
  blue for progress.

## Validation

Run the default repository gate:

```sh
npm run check
```

The JS/TS toolchain uses Vite 8 (Rolldown/Oxc), Vitest 5, stable native Go
TypeScript 7 (`tsc`), Oxlint and Oxfmt. Node 24/npm 11 remain the runtime and
package manager. `npm test` runs all isolated desktop, web and repository-script
suites; `npm run test:release-version` retains its historical name but runs all
repository-script tests. Workspace test commands remain available.

`npm run lint` covers first-party JS/TS with Oxlint correctness and React/hooks
rules. Three existing web hooks retain narrow React Compiler-rule exceptions;
this migration does not refactor their behavior. `npm run fmt` applies the
formatter baseline and `npm run fmt:check` enforces it. Generated routes, native
build output and public assets are excluded. Golden fixture generators still
run directly with `node scripts/native-*.test.mjs --write`; their test branches
load Vitest only when not generating fixtures.

The desktop retains its ES2022 build target. The website explicitly retains
Vite 7's Chrome/Edge 107, Firefox 104 and Safari 16 target instead of silently
adopting Vite 8's newer baseline. These are build targets, not new physical
browser or native-platform acceptance claims.

### Python native tooling

Install [uv](https://docs.astral.sh/uv/getting-started/installation/) for the
default gate. `npm run check:python` runs pinned Ruff 0.16.10 correctness checks
over all 36 Python scripts, then `npm run test:python` runs the native scripts' existing
unittest suite through `uv run --no-project`. Ruff checks syntax, undefined names
and invalid expressions/control flow; there was no existing Python formatter
gate, so this does not impose a large Python style baseline.

The 34 native scripts use the standard library or OS-installed D-Bus/GI/Xlib bindings;
there is no Python application package, pip dependency list or environment to
lock. `uv.toml` forbids interpreter downloads and selects existing system/CI
Python. No managed interpreter, project venv or Python runtime is added to the
website image. To run standalone packaging with the same policy, prefix the
commands below with `uv run --no-project`, for example:

```sh
uv run --no-project python apps/native/package.py --help
uv run --no-project /usr/bin/python3 -c 'import dbus, gi, Xlib'
```

The two optional font-fixture generators already used uv. They now declare
FontTools 4.60.2 in PEP 723 metadata and have per-script locks, without adding a
repository-wide Python package. This stable version retains Python 3.9/3.10
compatibility; current FontTools 4.66.1 requires Python 3.11. Regenerate with:

```sh
uv run --script --locked crates/captures-image/tests/make_test_font.py
uv run --script --locked crates/captures-image/tests/make_shaping_font.py
```

Keep explicit `/usr/bin/python3` for Linux graphical smokes: uv cannot replace
the distro bindings with a generic managed Python. Those commands and their
child-process interpreter selection remain unchanged. Native CI keeps its
existing `actions/setup-python` 3.12 selection where configured; uv wraps the
dependency-free unit/packaging commands without changing deployment behavior.

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

The [Wayland video diagnostic](apps/native/wgpu/README.md#wayland-video-acquisition-diagnostic)
checks ScreenCast consent responses, a granted PipeWire remote and changing
CPU-mapped pixels without X11. Its private software-rendered fixture uses a pinned
backend with a SHM-only format-guard backport, not an installed portal replacement.
The recording diagnostic also exercises real MP4/GIF session finalization,
pause/resume, restart, discard/cancel and recovery after transport loss. The
[native Wayland recording host smoke](apps/native/wgpu/README.md#native-wayland-recording-controls)
exercises the real History/countdown/HUD path in both appearances, cancellation
and source-loss recovery with the same disposable backend. It also checks nonzero
screenshot countdowns beside running/paused takes, cancellation before consent,
exact still pixels and pending-child Quit. The native screenshot-host smoke checks
idle countdown cancellation/retry and Quit during the countdown in both appearances.
Window recording uses
ScreenCast v3+ window grants, with fresh consent per segment and no display fallback.
Scripted window grants test protocol admission/cleanup; the live wlr fixture tests
its unsupported-window UI, not successful GNOME/KDE window capture. Physical consent,
audio/cursor and GNOME/KDE acceptance remain open.

The opt-in [stock GNOME window diagnostic](apps/native/gnome-window-verification.md)
uses disposable buses/profiles and software Mutter with the real window chooser.
It records window-only pixels, MP4/History metadata and cancellation/source-loss
controls. Its current verdict fails: GNOME window frames retain monitor-sized
padding, pending Request.Close crashes the stock GNOME 43 portal, and missing
real session state prevents successful native screenshot/History verification.
Do not substitute a display or treat this diagnostic as a completed parity gate.

The [native Wayland desktop-shortcut checks](apps/native/wgpu/README.md#native-wayland-desktop-shortcuts)
exercise private-bus adversarial grants and real native Preferences/routing on
private Sway. Build all native binaries plus `apps/native/wayland_drag_probe`, then
run `/usr/bin/python3 apps/native/wayland_shortcuts_smoke.py --binary
apps/native/wgpu/target/debug/wayland_shortcuts_probe` and
`/usr/bin/python3 apps/native/wayland_shortcuts_host_smoke.py --binary
apps/native/wgpu/target/debug/captures-wgpu-workbench --injector
apps/native/wayland_drag_probe/target/debug/captures-wayland-drag-probe --output
/tmp/native-shortcuts-new`. Use a new output directory. Grants are scripted,
not physical compositor key delivery. No installed keys/settings are modified.
Same-role remapping requires the compositor's fresh initial-configure fix;
Sway 1.7 is unsupported. On Debian 12, `.agents/setup` prepares a pinned,
headless-only Sway 1.9 fixture in the cache. Select it only for a smoke command:
`PATH="$(apps/native/build_wayland_compositor_fixture.sh):$PATH" python3
apps/native/wayland_visibility_smoke.py`. The helper's returned launch directory
also works for the native capture, recording, shortcuts and lifecycle smokes;
it never replaces the system compositor. Keep review output outside `/tmp` for
smokes that isolate `/tmp` in a private mount namespace.

### Native exploratory test archives

Native pull-request CI publishes `native-development-macos-ARM64`,
`native-development-windows-X64` and `native-development-linux-X64` artifacts
after their build/unit-test dependencies pass. Download the matching artifact
from the PR's Actions run, then extract both GitHub's artifact ZIP and the
package archive inside it. Read the included [TESTING.md](apps/native/TESTING.md)
before launching; use a new output folder and quit shipping Captures first.
Check the full run's results: an archive alone does not prove all GUI smokes passed.

These are isolated development packages, not Preview installers or updates.
They include pinned FFmpeg/FFprobe, corresponding source/licenses, and
`BUILD_INFO.json` with the CI source commit and final executable SHA-256.
macOS CI uses Apple Silicon/macOS 26 and ad-hoc signing, not notarization;
Windows is unsigned; Linux targets Ubuntu 24.04 x86_64/X11, not Wayland.
Platform runtime dependencies and physical acceptance remain open.
Windows/Linux archives omit absolute-path Open With registration files; use
local staging below if you explicitly want to register your own package.
Artifacts expire under GitHub's retention policy; none is a stable release.

Both native hosts accept `--live --open-media "/path/to/file"`; repeat
`--open-media` for PNG/JPEG/WebP/GIF/MP4/WebM paths. `--open-image` remains an alias
in the same ordered queue. Stills import owned History pixels; GIF/video entries
reference the external source and require FFmpeg/FFprobe as described below.
Neither path changes source bytes. External recordings keep **Save as new file**
locked on, so Save always writes a new copy. An already-open source keeps its edits; a closed screenshot
source with a saved draft must be restored or explicitly discarded from History
before source reload. AppKit also queues macOS file-open callbacks in live mode.
Bare development binaries do not register Open With associations. Subsequent
`--live` launches using the same canonical History root forward files to the
running host before initializing UI, settings or shortcuts; no files means
relaunch/focus. Relative paths use the sender's working directory. A different
`--settings-file` does not create a second owner of the same History root.
Use disposable `--history-root` and `--settings-file` paths for testing; fixture
scenes do not participate in single-instance election.

Forwarding uses private Unix sockets / current-user-only Windows named pipes,
not TCP. A request accepts up to 64 paths / 256 KiB; the transport retains at
most 32 queued requests. Full queues and startup failures exit nonzero. The
acknowledgement means queued, not successfully decoded or durably saved; normal
per-file errors remain in the resident host. Do not automatically retry a failed
acknowledgement, whose delivery may be unknown. Accepted quit stops the listener
before draining host workers and releases the election lock last.

### Native development login items

In a live native profile, Preferences → General → **Start Captures on login**
queries and explicitly changes a per-user, per-History-root entry. A saved
`launch_at_login` value does not enable it; fixture launches never register.
The entry runs the current executable with `--live --scene idle`, the canonical
`--history-root` and an absolute `--settings-file`. It starts hidden and preserves
tray/menu-bar and relaunch recovery. On X11 and Wayland, a missing or lost tray
host exposes History instead of leaving an unreachable process. Wayland
Preferences, Feedback and media bootstrap without mapping History. Actual desktop
sign-in and physical compositor focus/notice placement remain unverified.

The profile ID is the first 24 hexadecimal characters of the canonical History
path's SHA-256. These development entries are separate from Tauri:

- macOS: `~/Library/LaunchAgents/dev.captures.native.ID.plist` (`RunAtLoad`).
- Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value
  `CapturesNative-ID`.
- Linux/X11/Wayland: `$XDG_CONFIG_HOME/autostart/captures-native-ID.desktop` (or
  `~/.config/autostart/…` when XDG_CONFIG_HOME is not absolute/set).

Turn the control Off **before moving or deleting the binary**. Query and removal
require the exact owned contents; a moved executable, changed settings path,
symlink or conflicting entry reports an error and leaves the existing entry alone.
Inspect and remove only the named development entry manually if its old binary is
gone, then select Retry and explicitly enable again. Do not remove Tauri entries.
No admin/system-wide registration, login-agent reload, default handler change or
installed-data migration occurs. On Linux, executable paths containing `%`, `=`,
newlines or NUL are rejected because GIO validates them before Exec expansion;
profile/settings arguments may contain spaces, percent and Unicode.

Run `/usr/bin/python3 apps/native/x11_preview_smoke.py --lifecycle --login-item-only
--binary PATH --output NEW_DIRECTORY` for disposable XDG registration, real GIO
launch, hidden startup/focus, relaunch and disable in light/dark appearances.
This is private-X11/software-rendering evidence, not physical logon acceptance.
Run `/usr/bin/python3 apps/native/wayland_lifecycle_smoke.py --binary PATH
--injector apps/native/wayland_drag_probe/target/debug/captures-wayland-drag-probe`
for private headless-Sway/real-SNI startup, child windows, relaunch, explicit
disposable login registration and tray-loss recovery. See the
[wgpu setup](apps/native/wgpu/README.md#native-wayland-resident-lifecycle).

Fresh `--live` development profiles show setup before capture or external-media
import. Use a new `--settings-file` and `--history-root` to exercise it without
resetting shipping data or OS permissions. macOS checks are prompt-free until
Allow is selected; Screen Recording is required and microphone optional. Restart
is explicit and retains the development profile and queued media. macOS TCC may
require relaunch after changing the switch; a different signed build has a
different identity. Existing profiles with completed setup keep normal startup.
Run `python3 apps/native/x11_onboarding_smoke.py --binary PATH --output NEW_DIRECTORY`
for first-run, hidden-launch, capture-gating, queued-media and completion/relaunch
checks. Other live smoke fixtures explicitly represent completed profiles.
AppKit tests render permission/error states under `CAPTURES_TEST_ARTIFACTS`.
Setup copy and per-state labels come from `captures_app::onboarding`; change them
there so both native hosts stay identical to `Onboarding.tsx`.
These do not verify physical TCC, Windows input or Wayland capture.

Run `python3 apps/native/x11_update_notice_smoke.py --binary PATH --output NEW_DIRECTORY`
to screenshot every update notice state and drive notes, links, stub install,
restart and Escape on private X11. The notice uses a stub status source; no native
updater exists, and no download, install or URL open happens.

Run `python3 apps/native/instance_smoke.py --binary PATH --output NEW_DIRECTORY`
in a graphical session for real-process forwarding, sender-relative Unicode paths,
unchanged sources and normal-quit restart. Mac/Windows native CI runs this check;
Linux needs a private X11 session and window manager (as in the existing smoke
harness). The X11 `--external-image-only` suite also verifies edited-source alias
refocus, unchanged drafts and relaunch from a minimized root in both appearances.

### Native development Open With

`python3 apps/native/package.py` stages **unsigned development packages**, not
Preview installers. It never installs, registers, changes defaults, downloads
dependencies or modifies shipping data. Build the native host first, then choose
a new final output directory; staging refuses to overwrite one. Windows/Linux
registration contains absolute paths, so unregister before moving the package.
For GIF/video, run `npm run prepare:media` on the target host first and include
`--media-target` when staging. It copies the matching pinned FFmpeg/FFprobe pair
into `binaries/` beside the executable, plus corresponding source, signature,
configuration and licenses under `media-licenses/` (macOS: `Contents/Resources`).
Missing tools or source/license inputs fail before creating the package.
Omit the flag for a package that uses development tools instead. Both hosts
prefer executable `CAPTURES_FFMPEG` / `CAPTURES_FFPROBE` overrides, then bundled
tools, then checkout sidecars, then `PATH`; invalid overrides fall back.
Platform runtime dependencies remain those of the workbenches; these are not
trusted release-signed or installed-release builds.

Add `--archive /path/outside/package/native.zip` for macOS/Windows or
`--archive /path/outside/package/native.tar.gz` for Linux to create a portable
archive with `TESTING.md` and build metadata. Existing archives are never
overwritten. Local metadata leaves the source commit unset rather than claiming
the checkout produced an arbitrary input binary. On macOS, `--adhoc-sign` signs
and verifies before archiving so the recorded hash identifies the signed binary.
The staging directory retains opt-in registration files; portable Windows/Linux
archives omit them because moving the package would invalidate those paths.

macOS, after `bash apps/native/macos/build.sh`:

```sh
python3 apps/native/package.py --platform macos \
  --binary apps/native/macos/.build/release/CapturesNative \
  --resources apps/native/macos/.build/release/CapturesNative_CapturesNative.bundle \
  --media-target "$(rustc -vV | sed -n 's/^host: //p')" \
  --adhoc-sign \
  --output "$HOME/Applications/captures-native-dev"
```

The `.app` uses its own packaged Swift resources and defaults to live mode.
Open it once, then use Finder **Open With → Other…** without **Always Open With**.
Its six document types use Editor/Alternate, not default ownership. Cold Apple
events join the startup queue before single-instance election; running-app events
use the existing queue. To remove, quit this development app, unregister its exact
path with `/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -u "$HOME/Applications/captures-native-dev/Captures Native Development.app"`,
then delete that development package. This is ad-hoc signing, not notarization or
release signing; do not distribute it as a trusted download.

Windows, from PowerShell after building the wgpu host:

```powershell
python apps/native/package.py --platform windows `
  --binary apps/native/wgpu/target/release/captures-wgpu-workbench.exe `
  --media-target x86_64-pc-windows-msvc `
  --output "$env:LOCALAPPDATA\Captures Native Development"
```

Review and explicitly import `register-open-with.reg` from that directory to opt
in. It writes only HKCU development ProgID/Application entries and alternate
`OpenWithProgids` values; no elevation, extension default or `UserChoice` change.
Choose **Open with → Captures Native Development**, not **Always**. Document-mode
multi-selection invokes one process per file; the existing instance queues each
request (Explorer determines cross-process order). Import `unregister-open-with.reg`
before deleting/moving the package. It removes only this development identity and
its alternate values, retaining shared extension keys and other apps. Explorer
may need a sign-out/in to refresh its cached choices. Do not register two copies.

Linux, after building the wgpu host:

```sh
python3 apps/native/package.py --platform linux \
  --binary apps/native/wgpu/target/release/captures-wgpu-workbench \
  --media-target x86_64-unknown-linux-gnu \
  --output "$HOME/.local/opt/captures-native-dev"
# Optional per-user registration (does not select a default):
desktop-file-install --dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications" \
  "$HOME/.local/opt/captures-native-dev/es.captur.native-development.desktop"
update-desktop-database "${XDG_DATA_HOME:-$HOME/.local/share}/applications"
```

The desktop entry passes local files as `--live -- %F`, without a shell. Staging
paths cannot contain `%`, `=`, NUL or newlines; media filenames may contain `%` and
shell metacharacters. Remove only `es.captur.native-development.desktop` from the
applications directory and rerun `update-desktop-database` to unregister, then
delete the package. Wayland live capture remains gated; association metadata does
not change that limitation.

Both CLIs accept `--live -- FILE...`; all arguments after `--` are paths, never
options. `instance_smoke.py --positional` exercises that path on a disposable
profile. Python package tests check metadata/default preservation and actual GIO
argument expansion. macOS CI runs `macos_open_with_smoke.py` against a disposable
ad-hoc-signed bundle, including cold/warm/forced-secondary Apple events. Windows
CI tests the staged binary and command arguments, not Explorer registration.
Physical Finder/Explorer/file-manager, accessibility and installed-update
acceptance remain open on every platform.

### Native editor validation

Both native executables accept `--font-license` to print the bundled editor-font
copyright and full OFL notice without opening a window. Native resources and
font-bearing draft manifests also retain it. Run `apps/native/x11_editor_smoke.py
--text-only` with its usual `--binary`, `--output` and `--appearance` arguments to
exercise real Text controls; `--text-draft-only` retains the synthetic-font regression.
Use `--text-defaults-only` for pre-placement style/size/color, centered boxes,
unchanged drafts while configuring, undo/redo and default reset after reopening.
Use `--text-input-only` for transient composing, preview-error clipboard ownership,
outlined glyphs with selection/caret and rotation, middle-click PRIMARY at normal/
minimum size, unchanged drafts, undo and quit.
Use `--drawing-defaults-only` in both appearances for keyboard-entered exact custom
colors, partial input without draft writes, independently expected line pixels and
single Undo, normal/minimum color controls, and retained stroke/fill/shadow defaults.
Run `cargo +1.95.0 test --manifest-path apps/native/wgpu/Cargo.toml accessibility_`
for shared range bounds/actions, mixed keyboard/assistive event ordering and
custom-color input semantics. These checks do not replace physical screen-reader
acceptance on macOS, Windows, X11 or Wayland.

For the **public Linux accessibility provider**, use the distro interpreter:

```sh
/usr/bin/python3 apps/native/linux_crop_accessibility_smoke.py \
  --binary apps/native/wgpu/target/debug/captures-wgpu-workbench \
  --output /tmp/captures-atspi-new --appearance dark --orca
```

Use a new output directory; repeat with `--appearance light`. Dependencies are
`at-spi2-core`, `gir1.2-atspi-2.0`, `python3-dbus`, `python3-gi`, `python3-xlib`,
Xvfb, Openbox, xdotool, xclip, ImageMagick and FFmpeg. `orca` and
`speech-dispatcher`, `speech-dispatcher-espeak-ng` and `pulseaudio` are optional:
omit `--orca` for the provider-only run. Orca uses a private Speech Dispatcher
and null audio sink, never an installed desktop's speech daemon or sound output.
The smoke launches actual accessibility bus/registry services on a private
session D-Bus and private desktop, and enables only that bus's screen-reader status.
It uses completed disposable profiles, a known asymmetric PNG-derived GIF and
320×180/1600×900 MP4 fixtures. No capture permission, installed profile, save,
export, release or network-service activation is required.

X11 remains the default. For **Wayland**, reuse the existing compositor and
persistent virtual-pointer/US virtual-keyboard fixture:

```sh
cargo +1.95.0 build --manifest-path apps/native/wayland_drag_probe/Cargo.toml --locked
compositor="$(apps/native/build_wayland_compositor_fixture.sh)"
PATH="$compositor:$PATH" /usr/bin/python3 apps/native/linux_crop_accessibility_smoke.py \
  --backend wayland \
  --injector apps/native/wayland_drag_probe/target/debug/captures-wayland-drag-probe \
  --binary apps/native/wgpu/target/debug/captures-wgpu-workbench \
  --output /tmp/captures-atspi-wayland-new --appearance dark
```

Repeat with a fresh output directory and `--appearance light`. Wayland needs
Sway 1.9+ (the pinned fixture avoids system Sway 1.7), `grim` and `wl-clipboard`;
it does not need Xvfb, Openbox, xdotool, xclip or python3-xlib. `DISPLAY` is unset.
The same public Value matrix runs on both backends, including all-handle free
saturation, asymmetric aspect limits and clipped 100% scrolling. Compositor IPC
is used only to arrange the test window, inject input and crop its screenshot.
Optional `--orca` is diagnostic, not required for provider acceptance. Orca 43.1
requires `DISPLAY` and exits before its AT registry starts on this X11-free
fixture; retain its stderr/zero speech result, not a screen-reader pass. Do not
add a dummy X11 display to this Wayland proof.

Raw `*.atspi.json` snapshots contain public names, roles, attributes, numeric
values/ranges/step, bounds and actions. `*.layout.json`, screenshots,
`atspi-events.jsonl` and `result.json` retain independent graphical/state evidence.
X11 compares public `Component.GetExtents(SCREEN)` against fixture geometry
projected using an independently measured X11 client origin. Wayland compares
`GetExtents(WINDOW)` with zero projection offset. Raw `screen_bounds_unsupported`
and the result's separate `screen_diagnostic` retain Wayland SCREEN replies and
the test window position:
the current Unix adapter reports SCREEN equal to WINDOW without a global origin.
These replies are **not accepted global bounds**, never repaired with Sway IPC.
Global Wayland coordinates and wider compositor acceptance remain open.
The oracle checks all eight handles, free and aspect-limited ranges, normal/
minimum Fit layouts and clipped 100% scrolling. Standard AT-SPI
`Value.CurrentValue` writes must change staged geometry; successful D-Bus replies
alone cannot pass. The smoke checks both nudge directions, out-of-range writes
and asymmetric aspect constraints. Action enumeration remains separate. AT-SPI
`GrabFocus` plus compositor-delivered keys is a separate diagnostic, **not** a public
increment/decrement action. Clipboard field reads used when the editor provider
is missing are explicitly graphical diagnostics, not runtime AT-SPI evidence.
Sources and History are checksummed after import and must remain unchanged.

The smoke deliberately exits nonzero for absent editor nodes, descriptions or
inert Value writes, while collecting the remaining cases. The original baseline
exposes only History, not secondary editor nodes. With only the separately owned
secondary-adapter initialization fix, eight sliders expose values/bounds, but
Value writes are inert and source-pixel descriptions are absent. The separate
editor fix adds standard Value writes and Description; neither production fix is
part of this harness. Missing increment/decrement entries in AT-SPI Action are
not a Value transport failure. Orca debug speech/event output comes from the
actual reader, but does not establish audible delivery or human acceptance.
Plant the private-provider fault by adding `--fault-disable-provider` (without
`--orca`): discovery must fail with
`ScreenReaderEnabled=false`. Harness oracle tests run in `npm run test:python`.
All physical macOS/Windows/Linux, human screen-reader, Wayland global-bounds/
broader-compositor, IME and mixed-DPI gates remain open.

Run `python3 apps/native/primary_selection_smoke.py --binary
apps/native/wgpu/target/debug/captures-wgpu-workbench` for private X11/Wayland
PRIMARY transport, UTF-8/64 KiB boundaries, backend rejection, resource ceilings
and parent-death cleanup. It needs Xvfb, Sway and xclip/wl-clipboard; use the pinned
compositor PATH described above on Debian 12. This is not physical input acceptance.
Use `--properties-heading-only` for pinned image/shape/Text/Crop titles, actual
field scrolling at minimum size, and unchanged drafts/originals.
The `--canvas-interactions-only` suite also checks locked-line Curve/Straighten
Properties: held-drag pixels and draft geometry in both normal/minimum windows,
one undo step per pointer gesture, and individually ordered keyboard steps, while
locked canvas curve gestures stay blocked. Run it in both appearances.
Use `--shape-transforms-only` in both appearances for held canvas move/rotate/resize/
curve-dot pixels, byte-identical drafts while held, Escape restoring the committed
frame, single-step undo/redo, and minimum-size curve previews and draft reopen.
Unlike the Properties Curve slider, canvas gestures do not save edits while held.
Use `--output-presets-only` to exercise native compression presets and their
descriptions, the automatic before/after comparison with a dragged split handle,
Page Up/Home keys and Hide/Show, folder reveals after each Save, exact Highest
pixels, unchanged drafts/originals and minimum-size controls.
Use `--output-size-only` to exercise percentage/custom dimensions, aspect locking,
saved-file/History consistency, unchanged drafts and full-resolution clipboard copy.
Use `--polygon-only` to exercise Triangle/Diamond/Star transient and committed pixels,
concave star notches, cancelled/degenerate gestures, undo/redo and saved-draft reopening.
Use `--rotation-snap-only` to exercise the per-editor custom increment, Shift preview
and cancellation, committed angle/pixels, undo/redo and restored drafts.
Use `--overwrite-only` for explicit confirmation/Cancel/Escape, exact replaced pixels,
stable History identity/date, the saved-file reveal after every Save (a stub
FileManager1 records it), retained draft and undo/redo, and minimum-size controls.
Use `--external-image-only` for multi-file startup, canonical aliases, retained
errors, exact imported pixels, source preservation, saved-draft refusal/restoration
and same-ID source reload after explicit discard.
Use `--import-formats-only` for the real multi-select picker transport, GIF first-frame
transparency, asymmetric BMP pixels, SVG viewBox/text/local reuse/straight alpha,
external-SVG-reference conversion errors, one Undo per import and draft reopening
after removing all source files. Run it in both
appearances; it also captures minimum size. SVG resource/DTD/size/node/depth failures
are shared Rust regressions; AppKit uses the same decoder through the retained-frame ABI.
Use `apps/native/x11_recording_editor_smoke.py --external-media` with `--binary`,
`--output` and `--appearance` for a mixed still/GIF/MP4/WebM batch, live edits
through alias refocus, closed-source reopen, immutable sources and real WebM-to-MP4
export. It uses a private X11 session and disposable data, not physical acceptance.
Use `--history-shortcuts-only` for document Undo/Redo keys, exact restored layers,
duplicate offsets/selection, Delete/Backspace, locked-layer protection,
text-field and confirmation focus, and unchanged source bytes.

Native Record verifies FFmpeg and FFprobe on its worker before starting. Both hosts
resolve overrides, bundled development-package tools, prepared checkout sidecars
and `PATH`, in that order; see the staging instructions above. Recording uses separate development
History and a sibling `recording-recovery` directory. Do not point tests at real
capture data. The Linux recording acceptance owns a private Xvfb desktop and D-Bus
session; install the windowing dependencies from the wgpu README plus `ffmpeg`
and `python3-xlib`, then run:

```sh
/usr/bin/python3 apps/native/x11_recording_smoke.py \
  --binary apps/native/wgpu/target/release/captures-wgpu-workbench \
  --output /tmp/native-x11-recording
```

For the frame-based recording editor, run the following with both `dark` and
`light`. It creates a known-color recording in disposable History, opens the real
editor and checks source-relative seeks, rejected live edits, trim, asymmetric crop and
independent output sizing. Edits apply live as in shipping (there is no Apply edits
or Estimate size button); the smoke waits for the editor's Working title to settle.
Decoded MP4/GIF pixels verify crop origin and scaling, even output dimensions, MP4
encoder-capped versus explicit GIF sizes, failed-edit save gates and unchanged source
bytes. `--replace-original` checks that Save overwrites the original without a
confirmation, then changes folder through rfd's portal transport and saves a Unicode
filename with **Save as new file** off. It checks collision refusal, retained History
identity, the unchanged previous save, adopted-path repeat Save and cancellation.
The disposable portal response is not physical file-dialog acceptance. Review the
normal/minimum-size and collision/error captures as well as the applied preview:

```sh
/usr/bin/python3 apps/native/x11_recording_editor_smoke.py \
  --appearance dark \
  --binary apps/native/wgpu/target/release/captures-wgpu-workbench \
  --output /tmp/native-x11-recording-editor-dark
```

Pass `--hide-controls-only` to `x11_recording_smoke.py` for focused running/paused Hide checks through a real
Xfce SNI tray, configured New Capture shortcut restoration, tray-host-loss recovery,
finalized media decode and recovery cleanup.

Pass `--start-failure` (needs the PulseAudio tools listed below) to start a take with the
selected microphone missing. It checks the failed HUD, inline error, disabled controls,
the Retry recording tooltip, a failing and then a succeeding Retry recording with no
confirmation, and a confirmed Delete. `--device-change explicit` checks that a resume
without the microphone stays paused with the inline error and still saves.

Pass `--ready-notice-only --appearance dark` to that recording smoke (and repeat with `light`) to exercise
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
