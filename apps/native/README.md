# Native desktop workbenches

These are implementation stages of the [native rewrite](../../docs/native-rewrite.md),
**not replacement downloads**. The existing Tauri Preview is unchanged.
macOS is Swift/AppKit + Core Animation with Core Image explicitly backed by Metal.
No WebView, React, JavaScript runtime or Rust sidecar is used. Local capture
does not require a network service; optional sharing uses the account API.
Fixture launches do not request screen access. The opt-in capture workspace below
connects the existing Rust capture engine in process. The
[shared wgpu candidate](wgpu/README.md) adds Windows/Linux fixture windows and
cross-platform resource diagnostics; it is not a production renderer selection.
Its fade/settle probe is not equivalent to AppKit dust. DirectComposition/GTK
comparators and full parity gates remain open. The instructions below cover
AppKit; the candidate README has Windows/Linux build and test commands.

For exploratory testing without a local build, native PR workflows publish
development archives with pinned media tools, source/licenses and build identity.
See [download instructions and platform limits](../../DEVELOPMENT.md#native-exploratory-test-archives)
and the included [TESTING.md](TESTING.md). Use a new export folder and quit the
shipping app first. macOS CI archives are ad-hoc signed, Windows unsigned and
Linux requires the documented X11 or experimental Wayland prerequisites. These
artifacts do not close signing, updater, migration or physical-platform acceptance
gates.

## Native update acquisition diagnostic

`captures_app::updater` implements shared signed-manifest checks and bounded,
streamed downloads and temporary package staging without Tauri. It is **not an
enabled updater**: Preferences, tray actions and the update-notice fixture remain
unchanged. There is no bundled native signing key, release endpoint or installed-app
updater. The explicit development-package helper below can replace and launch only
a selected stopped development package with a new disposable test profile.
`native_update_probe` creates no window, changes no installed/profile data and
removes its download and staged files on normal exit. It requires an explicit
endpoint, standard two-line Minisign public key file, renderer and current version:

```sh
cargo +1.95.0 run --manifest-path apps/native/wgpu/Cargo.toml --bin native_update_probe -- \
  --manifest-url "$NATIVE_TEST_MANIFEST_URL" --public-key-file "$NATIVE_TEST_PUBLIC_KEY_FILE" \
  --renderer wgpu --current-version 0.1.0
# Add --download-directory EXISTING_DIRECTORY to acquire and verify bytes only.
# Or --stage-directory EXISTING_DIRECTORY to also extract and validate a package.
# These modes are mutually exclusive. Both remove their temporary files on exit.
```

The manifest is UTF-8 JSON, at most 256 KiB. `<manifest path>.minisig` contains a
standard detached **prehashed** Minisign signature, not Tauri's outer base64.
Verify/sign the exact bytes; whitespace or reserialization changes the signature.
The signed v1 shape is:

```json
{
  "schema": 1,
  "identity": "es.captur.native-development",
  "renderer": "wgpu",
  "version": "2026.10.50",
  "notes": "Development fixture only.",
  "artifacts": {
    "x86_64-unknown-linux-gnu": {
      "url": "https://example.invalid/native-development.tar.gz",
      "size": 23,
      "sha256": "d2820340a902904952ed3ce50313a4867f8717eadfbbc24378a863d52d2009cf"
    }
  }
}
```

This example is illustrative, not a download or signed release. Replace every
artifact value with the actual native package's URL, byte count and SHA-256 before
signing. `appkit` accepts macOS ARM64/x64 targets; `wgpu` additionally accepts
Windows x64 and Linux x64. Renderer and current host target must match. Equal,
older and build-metadata-only versions do not offer a download. HTTPS is required;
HTTP is allowed only for loopback diagnostics. Metadata redirects are rejected;
artifact redirects are bounded and may not downgrade to non-loopback HTTP.
The signature is bounded to 8 KiB and artifacts to 1 GiB. Cancellation checks occur
at I/O boundaries; a blocked HTTP request can take up to its 60-second timeout.
Tampering, truncated/oversized bytes, wrong signatures/identities/targets and
cancelled downloads never produce a verified file. Tests use disposable in-memory
keys and private loopback data, not release keys or public services. Signature and
version checks do not establish channel freshness, installed-data migration,
OS signing or physical-platform acceptance.

Staging consumes a verified download, rehashes a private copy, and extracts only
inside an owned temporary directory. It accepts `package.py`'s current single-root
development ZIP32 (macOS/Windows) and tar.gz (Linux) layouts with media sidecars,
source/licenses, matching `BUILD_INFO.json`, executable hash and permissions,
and the macOS development bundle identity/resources. It never executes binaries,
imports registration files or changes a profile. `StagedUpdate` owns cleanup;
callers must keep it alive while reading its paths. Exposed files are not immutable:
replacement rehashes and re-extracts the retained verified archive instead of
trusting those files. Forced termination can leave private staging scratch behind.

The staging format deliberately rejects symlinks/hardlinks/special entries,
traversal/absolute paths, Windows device/stream names, trailing dots/spaces,
duplicates, case conflicts and file/directory collisions. Resource names inside
the outer folder must be ASCII; the outer folder may contain Unicode. Limits are
10,000 archive records and 10,000 path nodes (including implicit directories),
512 MiB per file, 2 GiB expanded file bytes, 256 KiB metadata and 8 MiB ZIP central
metadata. ZIP64, comments, extra fields, encryption and data descriptors are not
supported. Local PAX path metadata is bounded; global PAX and GNU extensions are
rejected. Tests use the real Python packager with inert bodies for all four target
layouts and adversarial archives. These are extraction checks, not signed OS
distribution or real-machine acceptance.

### Development-package replacement and interruption recovery

`StagedUpdate::replace` explicitly replaces an existing **development package
root**, not an `.app` bundle, Preview installation or profile. It validates the
old package, re-extracts signed bytes on the destination filesystem, and retains
the entire old package until `PendingInstallation::confirm`. `rollback` or
`recover_installation` restores unconfirmed replacements, including the gap
between renames. Confirmed replacements finish cleanup without restoring old bytes.
Transaction IDs reject stale handles; an OS file lock serializes operations and
releases after termination. Cleanup commits an atomic directory rename before
recursive deletion, so partial deletion does not destroy recovery's decision.

This API requires the exact absolute destination path, the current host target,
a trusted parent directory, all app processes stopped, the caller running outside
the package, and all profile/export data outside it. Replacement itself never
launches either version; `PendingInstallation::launch` adds the opt-in health
handoff described below. Changed files, invalid receipts,
links/junctions and unfamiliar packages are preserved for manual recovery.
Interruption before the preparation receipt is published can leave scratch that
requires manual cleanup; it has not moved the old app. A persistent empty sibling
lock file is intentional. Process-interruption recovery is **not a cross-platform
power-loss guarantee**; Windows directory syncing is not supplied by Rust `std`.

Tests use real Python-packaged, signed **inert** binaries/sidecars and disposable
profiles. They cover each rename/cleanup boundary, exact old/new content,
tampering, cancellation, locks, stale handles and conflicting files. The existing
macOS/Windows updater CI jobs include these tests; physical installed-app,
permission-identity and crash/relaunch acceptance remain open. No GUI update action,
channel, registration or installed-data migration is enabled by this backend.

### Opt-in development helper and startup health

Build `native_update_helper` in the shared workspace and run it **outside** the
package. Quit every native app process first and prevent other launches until the
handoff finishes. Supply your own signed test endpoint/key and the package **root**
(not the executable or macOS `.app`). Create a new empty profile directory outside
the package, transaction and cleanup trees; existing profiles are rejected.

```sh
cargo build -p captures-app --bin native_update_helper
# Every path below must be explicit; no installed location or profile is inferred.
target/debug/native_update_helper \
  --manifest-url "$NATIVE_TEST_MANIFEST_URL" --public-key-file "$NATIVE_TEST_PUBLIC_KEY_FILE" \
  --renderer wgpu --current-version 2026.10.51 \
  --stopped-development-package "$ABSOLUTE_NATIVE_PACKAGE_ROOT" \
  --empty-test-profile "$ABSOLUTE_NEW_EMPTY_PROFILE" \
  --health-timeout-seconds 60
# On Windows: target/debug/native_update_helper.exe. On macOS: --renderer appkit.
```

The helper acquires/stages signed bytes, activates the replacement and launches
the packaged executable directly. It disables system-shortcut takeover and passes
only the new history/settings paths. AppKit and wgpu acknowledge only as the elected
primary, after workspace initialization/render submission, a successful settings
load and verification of the exact packaged `binaries/ffmpeg-<target>` and FFprobe.
Readiness never substitutes PATH, environment overrides or checkout tools, and
requests no capture permission. The private, attempt-specific file contains exactly
one canonical lowercase UUIDv4 followed by a newline; replacing its path, partial
bytes, another token or a late response cannot confirm the update. This checks
startup, not physical frame presentation, accessibility or capture acceptance.

On success, JSON reports `confirmed` and the process ID; the new GUI stays running
with diagnostics in the new profile's `startup.log`. The operation lock remains
held through acknowledgement and confirmation, with root liveness/cancellation
rechecked after rehashing immediately before committing confirmation. **Startup
failure retains the transaction and old package; no automatic post-launch rollback.**
An exited/killed root process, even with successful exit status, does not prove
all descendants stopped. Failure JSON reports `handoff_failed`, the root process
ID if started, and manual recovery. Stop **every** app process and exclude other
launches before explicit `recover_installation`; restarting the helper refuses a
pending transaction rather than silently recovering it. Confirmation/cleanup
failure can leave an acknowledged host running and confirmation already committed:
recovery then finishes cleanup rather than restoring the old package. Inspect the
receipt, cleanup directory and log; a post-commit old backup may be incomplete.
This helper does not import installed history/settings, register an app, preserve
OS permission identity, publish a channel or enable the Update now button.

After stopping **all** app processes and excluding new launches, recover the same
explicit development-package root (it may be missing after an interrupted rename):

```sh
target/debug/native_update_helper \
  --recover-stopped-development-package "$ABSOLUTE_NATIVE_PACKAGE_ROOT" \
  --all-app-processes-stopped
```

`--all-app-processes-stopped` is the operator's assertion, not process detection.
Recovery never kills/launches an app, contacts a release endpoint or imports a
profile. It validates receipts and full package hashes, then restores an
unconfirmed backup or finishes already-committed cleanup. Links, conflicts and
changed files remain untouched for manual repair. JSON reports `recovery_complete`
or `no_pending_replacement`; success does not always mean rollback. Do not combine
this mode with acquisition flags. The persistent empty sibling lock file remains.

Runnable signed-package regressions cover partial/wrong/oversized/replaced-file
acknowledgements, late health, a clean root exit leaving a live child, timeouts,
prelaunch cancellation/profile rejection, exclusion of recovery during handoff,
and exact backup retention. Host tests cover settings/render/tool readiness and
termination. Linux X11/software-rendered handoff is tested in the orb; macOS,
Windows and live Wayland acceptance remain unverified.

## Explicit offline development-profile import

`native_profile_import` copies explicitly selected shipping settings and local
data into a **new** native development profile. It does not discover installed
locations, change the source, launch an app or connect profile import to updating.
Stop shipping/native app processes and other data writers, exclude new launches,
and choose a private destination parent you own:

```sh
cargo run -p captures-app --bin native_profile_import -- \
  --source-settings-file "$ABSOLUTE_SHIPPING_SETTINGS_JSON" \
  --source-data-directory "$ABSOLUTE_SHIPPING_LOCAL_DATA_DIRECTORY" \
  --new-development-profile "$ABSOLUTE_NEW_NATIVE_PROFILE" \
  --all-app-processes-stopped
```

The stopped-process flag is an operator assertion, not detection. The destination
must not exist or overlap source data. Import copies only settings, `capture-history`,
`screenshot-editor-drafts` and `recording-recovery`; account/credential stores,
registration and external exports are not imported. `source-snapshot` retains the
selected original file bytes and empty directories with SHA-256 receipts. A second
copy supplies native `settings.json`, `history`, `editor-drafts` and recovery.
Expect roughly twice the selected source size in additional storage. Limits are
100,000 entries, 64 GiB selected source bytes and 8 MiB JSON documents.

Schema migrations run only on the working copy. New exports use the profile's
`exports` folder; copied History cannot overwrite/trash shipping exports. Login,
onboarding and permission/restart bookkeeping reset for the new app identity.
Invalid/newer schemas, missing media, reference-only recordings without retained
History media, unfinished publication intents and links/special files reject the
whole import. Nothing outside the selected trees is followed to repair them.
Source changes, conflicts and cancellation leave no partially published profile.
An OS lock serializes imports; its empty sibling file remains. Forced termination
may leave private sibling scratch. This is not a power-loss durability claim.

Launch either development host manually with `--live --history-root
"$ABSOLUTE_NEW_NATIVE_PROFILE/history" --settings-file
"$ABSOLUTE_NEW_NATIVE_PROFILE/settings.json"`. Setup must run for the new identity.
Rollback means quitting native and returning to the unchanged shipping profile;
do not copy a snapshot over installed data. The signed update helper still accepts
only a new empty test profile. Automatic handoff, reference-only-media reconciliation,
OS permission identity and physical macOS/Windows/X11/Wayland acceptance remain open.

## Native sharing controls

Both hosts connect mini-preview Share and selected History to the shared Rust
account/upload worker. AppKit uses an opaque C handle and safe presentation
events; bearer tokens, challenge IDs and snapshot paths do not cross into Swift.
Opening/sign-in never upload. Password/expiry, progress/cancel/retry, link
management and confirmed cloud Trash/Restore are explicit actions. Closing the
window retains accepted work; Quit and permission recovery drain it before
releasing the isolated profile. Capture hides Share regardless of preview inclusion.

AppKit `--scene sharing` renders disabled controls without a worker. Set
`CAPTURES_NATIVE_SHARE_FIXTURE` to `otp`, `vault`, `shared`, `uploading`, `trash`
or `error`; live transport ignores fixture state. Tests collect light/dark and
minimum-size renders through `CAPTURES_TEST_ARTIFACTS`. All seven AppKit sharing
regressions passed in macOS CI; the light/dark minimum-size states were inspected.
Physical macOS/Windows/X11/Wayland vault/object-store acceptance remains open.
The account service is still disabled and undeployed; these controls
do not activate it or change the shipping Preview.

## Persisted native Preferences

Both native hosts use the same `captures-settings` Rust types, migrations,
validation, atomic persistence, and custom-theme derivation as the shipping
adapter. AppKit calls a versioned in-process C ABI; Windows/Linux calls Rust
directly. Preferences stores appearance and capture/media defaults in a separate
**Captures Native** development identity. `--settings-file PATH` selects an
explicit test file. Malformed/newer files report an error instead of resetting
them. `--exercise` uses disposable data. No installed Preview settings are imported.

Without `--live`, capture, recording, history, and editor scenes use fixtures. Saving a
default is not an engine integration: fixture login items, feedback submission and
update actions remain visibly unavailable, with copy that says why. Both hosts draw
the shipping Preferences layout, controls and copy, shared through
`captures-app::preferences`; physical input, screen-reader and Windows/Wayland
acceptance and the other checklist gates remain open.

Preferences records all seven stored shortcut fields using the shared Rust
key/modifier, display, cancellation and validation policy. Escape (including
modified Escape), focus loss or leaving the recorder cancels without saving.
Modifier-only input previews the chord; invalid keys show an inline error.
Only an active focused recorder releases all seven capture OS registrations so
it can receive an existing global chord. Completion, Escape, blur, hiding or
replacing its controls restores the latest saved bindings. Focused Preferences
otherwise permits screenshot, recording and New Capture shortcuts; a consumed
recorded press does not arm a capture on its later release. Registration failures
are reported rather than treated as success.
New Capture and all six screenshot/recording target actions are connected in live mode.
Fixture Preferences never registers global keys. The shipping TypeScript policy
supplies 390 recording and 195 platform-display differential test vectors.

On X11, `x11_preview_smoke.py --lifecycle --shortcut-editing` exercises all seven
storage paths with native input, registered-key delivery, invalid/cancel/blur,
duplicate rejection, held-key completion, focused-Preferences screenshot/recording/
New Capture launches, restart persistence and saved Region/New Capture chords.
The existing `CAPTURES_NATIVE_LAYOUT_PROBE` waits for settled navigation and
recorder focus before sending each chord once; it reports named rectangles and
focus, never typed keys or binding values. wgpu invalidates cached Preferences
focus on every native focus event so transferring to History cannot briefly
re-suspend restored shortcuts. AppKit is unchanged.
AppKit XCTest renders both appearances and checks controller/bridge input. Synthetic
virtual-key mapping does not prove physical Mac media/external-keyboard input;
physical Windows/macOS, Wayland and screen-reader acceptance remain open.

## Shared recording runtime

`captures-recording-platform::RecordingSession` owns a durable recovery bundle
and the existing platform engine across start, pause/resume, restart, stop, discard and
MP4/GIF finalization into private History. Run its blocking methods on a worker.
Hosts still own permissions, countdown presentation, window exclusion and the
capture-generation cancellation gate passed to `start`; `prepare` never records.
Linux portal-selected display sessions use `RecordingTarget::PortalDisplay` and
`prepare(..., None)`, without invented monitor IDs or geometry. The new target is
explicitly unsupported on other OSes and rejected by shipping's selection adapter.
Cancellation reaches portal consent and first-frame waits. Pause/resume requests
a fresh grant; stream loss stops encoding and retains recoverable media instead
of publishing a successful take. The wgpu Wayland host connects History's
**Record display…** to native countdown/HUD controls and MP4 publication; its
worker refreshes source failures into History recovery. Portal consent cancellation
discards only an empty initial take and retains accepted paused media on resume.
Failed assembly/publication keeps source segments. Successful video publication
removes the draft only after Ready metadata is saved; GIFs keep editable sources.
Post-publication housekeeping failures return the saved artifact with a warning.
Restart discards only that session's active and completed segments, retains its
target/options, resets elapsed time, and returns to the stored countdown. The
same capture generation rearms global Escape for direct recording, or focused
countdown cancellation on Wayland, before opening the replacement engine.
Both hosts connect Video-only Record controls and region/window/display recording
shortcuts. From idle the keys open Record on that target; in an open selector,
screenshot and recording keys switch mode/target in place. Busy recording phases
and active shortcut recorders suppress capture shortcuts. Hidden recording controls are the exception:
only New Capture is routed, and it restores the same generation without launching another capture.
The HUD Screenshot action opens the existing region selector under a temporary child
generation while the accepted recording keeps running or paused. Escape cancels only
the selector/countdown, and a completed still follows normal History, mini-preview
and auto-copy behavior. AppKit/Windows apply capture exclusion; X11 hides the HUD and
guide from the still but cannot exclude the selector from ongoing recording pixels.
Both hosts open recordings in the native recording editor, which saves MP4 and
GIF. New editor windows honor the saved export format, GIF frame rate and maximum
width, including custom valid values. GIF sources stay GIF; the MP4 preference
preserves an opened WebM's format. WebM remains explicitly unavailable in bundled
tools; choose MP4 or GIF to export it. Refocusing an existing editor retains its
edits. Capture still records a video master, and GIF palettes follow editor quality,
not the recording palette preference.
Unsigned development packages can include the existing pinned media tools,
corresponding source and licenses; see [staging](../../DEVELOPMENT.md#native-development-open-with).
History **Save file**
copies original video/GIF bytes to the configured output folder without encoding
or overwriting another file. Repeated Save reuses the export; deleting it allows
another copy from private History. **Show in Folder** reveals the exported copy,
which survives deleting or clearing History. The poster remains the native preview.

Closing a recording editor opens a nonactivating fixed-glass **Recording ready**
notice at the display work area's top right. It reuses History's Save file operation
and changes to **Recording saved** / Show in Folder after export. Save failure and
missing-file reveal errors remain retryable; Dismiss and the 15.2-second expiry
never delete media. Expiry pauses during saving and restarts on completion/error.
A new capture clears the notice, and stale callbacks cannot reopen a dismissed
or replaced notice. Physical input,
compositor, multi-display and accessibility acceptance remain open.

Linux CI records an asymmetric 310×170 region on a private Xvfb display, pauses,
resumes and restarts from running/paused, independently decodes replacement pixels
with FFmpeg, publishes video
and GIF History entries, and exercises cancellation and persistence failures:

```sh
env -u WAYLAND_DISPLAY XDG_SESSION_TYPE=x11 CAPTURES_TEST_PRIVATE_X11=1 \
  xvfb-run -a -s '-screen 0 640x480x24 -nolisten tcp -noreset' \
  cargo test -p captures-recording-platform \
  private_x11_records_pause_resume_pixels_and_cancelled_start -- --ignored
```

This needs `xvfb`, `xauth`, `hsetroot`, FFmpeg and FFprobe. It is not physical
Windows/macOS, audio-device, multi-display or Wayland acceptance.

Failures follow the shipping HUD. If the engine cannot start a take (for example
the selected microphone is missing), the HUD stays up as **Failed** with the error
on one line below the controls, and offers **Retry recording** (no confirmation) and
Delete. If resume or a microphone change cannot reopen the engine, the take stays
paused with that error, ready to resume or save. Stop shows **Saving…** until the
take is published. A failed save still closes the HUD and keeps the recovery bundle.
Engine warnings share the error line, which clears when the next HUD action starts.
HUD buttons use the shipping fixed-glass tooltips, which appear immediately on hover
or focus. `x11_recording_smoke.py --start-failure` exercises these states on X11.

The native recording Hide slice removes the AppKit or wgpu HUD without changing
the accepted session, timer, pause/microphone state, capture generation or media.
A 6.2-second click-through fixed-glass notice explains restoration; no collapsed
replacement strip remains. Menu bar/tray actions, app reactivation and the configured
New Capture shortcut restore controls under the persisted capture-exclusion policy.
Linux enables Hide only while a real SNI host supplies a restoration path, and tray-host
loss restores the HUD and workspace. AppKit and Windows still require physical-host
compositor and accessibility acceptance; Wayland remains gated.

## Resident lifecycle and screenshot shortcuts

Only `--live` creates the macOS menu-bar item or Windows/Linux tray and registers
the persisted New Capture and region/window/display shortcuts. The menu uses the
shipping labels, order and separators: New Capture…, Screenshot
Region/Window/Display, Record Region/Window/Display, Capture History…, Open Save
Location, Preferences, Send Feedback…, a disabled Check for Updates… (signed
updates are not connected) and Quit Captures. Capture items show the saved
shortcuts as accelerators where the platform menu displays them (Linux SNI hosts
may not), and any tray capture action brings hidden recording controls back. Closing the root hides it when a usable tray is available; previews and
accepted work stay alive.
Quit cancels pending capture, drains accepted file work and removes shortcuts/tray.
Timed and framebuffer-screenshot completion also explicitly quit, not hide.
Captures launched from a hidden root leave it hidden on success or cancellation.
macOS Dock reopen shows an existing visible window or Preferences.

### Local crash review

Only a live primary starts local diagnostics, under the History profile's
`.crash-diagnostics` directory (excluded from History pruning). Retained markers
and bounded, home-path-redacted Rust panics are available in local review before
any sharing. Copy is local; Add to message/Feedback appends visible editable text;
only explicit Send uses the existing feedback channel. Fixtures/secondaries do
not collect evidence or start uploads. Dismiss removes prior evidence, not the
current session. Hidden login startup does not raise a diagnostic window.

Normal accepted Quit cleans after draining and before releasing instance election.
AppKit restart disarms before handoff and rearms only after failed spawn wins
election again. Unix TERM/HUP/INT and Windows session-end messages classify normal
OS stops; KILL or an unclean marker alone is not proof of a crash. OS exception
report discovery and physical-platform shutdown/restart acceptance stay open.

```sh
cargo test -p captures-app --test crash_lifecycle --locked
cargo test -p captures-feedback -p captures-settings-ffi crash --locked
cargo test -p captures-session shutdown --locked
node apps/native/prepare.mjs --output apps/native/wgpu/resources
cargo +1.95.0 build --manifest-path apps/native/wgpu/Cargo.toml --locked
/usr/bin/python3 apps/native/x11_crash_smoke.py \
  --binary "$(pwd)/apps/native/wgpu/target/debug/captures-wgpu-workbench" \
  --output /tmp/captures-native-crash-review
```

The output directory must not exist. The X11 smoke seeds retained panic text,
exercises light/dark/default/minimum UI and real input, verifies exact local
clipboard/message text and marker ownership, and uses a non-forwarding loopback
proxy to assert no review action attempts networking. Isolated Rust subprocess
tests exercise real panic and signal handlers. These are orb/software-GL checks,
not physical macOS/Windows/Wayland acceptance; AppKit XCTests require macOS CI.

The shared Rust dispatcher queues release-triggered screenshot actions and
temporary Escape cancellation without competing process-wide handlers. Native
event loops drain actions on their UI thread. Active capture/preparation and
active Preferences recorders suppress screenshot shortcuts; Preferences alone
does not. Invalid/colliding shortcuts report errors. This does not complete
physical platform/input or lifecycle parity.

General is the first Preferences card and owns the opt-in **Start Captures on
login** control. Existing per-profile development login entries remain separate
from the shipping app; Wayland now supports tray-resident hidden startup and
independent Preferences/media/Feedback windows. Physical sign-in acceptance stays
open. Updates keep the actual crate version and **Native development** identity
on the left, with a
160-point disabled Check Now action and unavailable status on the right at both
normal and compact widths. There is no native check or last-checked timestamp.
Feedback places Included automatically app/system details directly below its
header, before Category, Message and Contact; submission and consent are unchanged.

Like shipping, the live shortcut owner unbinds overlapping system screenshot keys
when it starts and whenever a binding changes, with no prompt and no automatic
restore (`captures-app::system_shortcuts`). macOS writes the disabled ⌘⇧3 / ⌘⇧4 /
⌘⇧5 Screenshot hotkeys through `defaults` and AppKit also disables them live in
WindowServer; the wgpu host has no live WindowServer call, so on macOS it relies on
the persisted setting. Linux clears the overlapping GNOME `gsettings` keys and, for
Super+Shift+S, KDE Spectacle's region key. Windows turns off Print Screen for
Snipping Tool and intercepts Win+Shift+S with a keyboard hook instead of
registering it. Only the seven native bindings count (the GIF shortcut is not a
native binding). Preferences shows the shipping copy and opens keyboard settings,
where users restore the keys. Smokes set
`CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER=1` so they never change the real OS
configuration; unit tests use a fake command runner. Verified by unit tests only:
no physical macOS, Windows, GNOME or KDE session was checked.

On macOS, a capture denied Screen Recording access offers the shipping
**Restart & Retry** dialog (access was requested this launch) or **Reset, Restart
& Retry** (`tccutil reset ScreenCapture` for this bundle only, then the saved
prompt identity is cleared). The capture mode is saved as
`pending_capture_after_restart`, the host relaunches, and the next launch takes
the retry once and runs that capture instead of the startup notice. New Capture
and recordings retry as Region, like shipping. The wgpu host does not offer the
dialog: shipping shows it only on macOS, where AppKit is the native host.

Live hosts now elect one process per canonical History root. Subsequent launches
forward media paths or request native reactivation without creating UI or capture
workers. Fixtures remain independent. See [DEVELOPMENT.md](../../DEVELOPMENT.md#native-frontend-migration)
for limits, acknowledgement semantics and cross-platform process tests.
Optional [development Open With packages](../../DEVELOPMENT.md#native-development-open-with)
stage a separate AppKit `.app`, Windows per-user alternate registration files or a
Linux desktop entry. Nothing is installed or registered by staging; opt-in steps
and removal are documented separately. Both CLIs support `--live -- FILE...`.
The bundle defaults to live mode and collects cold Apple events before election.
These packages do not close physical lifecycle or installed-release acceptance.

Linux uses SNI/KSNI over session D-Bus, not XEmbed or GTK/AppIndicator. Building
needs pkg-config and libdbus-1-dev; runtime needs a registered StatusNotifier host
and `xdg-open` for folder fallbacks; Show in Folder first asks a
`org.freedesktop.FileManager1` implementer to select the saved file. No watcher/host means an explicit error and
normal close-to-quit. Losing the tray host restores the root instead of stranding
the process. XEmbed-only trays require an SNI bridge. Wayland History offers
desktop-portal screenshots even without a tray. Quiet startup stays hidden with a
tray and exposes History without one; tray loss also exposes History, and closing
it quits normally. Native selectors and recording remain gated. Physical macOS,
Windows, mixed-DPI and accessibility acceptance remain open.

## New Capture controls

New Capture starts a screenshot selector in Region mode. Its fixed-glass toolbar
switches between Region, Window and Full screen without replacing the prepared
desktop snapshot or discarding settled selections. An actual display change
prepares a replacement session and clears display-local selections while retaining
target mode and aspect ratio; stale replies cannot reopen a cancelled selector.
Capture is disabled until the selected target
is valid. Enter confirms, Escape cancels, and aspect/auto-start/freeze/countdown
preferences use the existing Rust geometry and capture policies. The root returns
to its prior visibility; a background capture does not reopen Preferences.

One Rust `WindowSession` also accepts a region target, reusing region crop/cursor
validation without another full-screen copy. A nonzero countdown refreshes pixels
for all three targets. Existing direct screenshot actions remain available.

Menu copy and small policies come from `captures-app::capture_menu`, so both hosts
share the shipping labels. The footer note reports whether "these controls" show
in screenshots or recordings from the recording capabilities; where the platform
can exclude them (macOS/Windows) it and the "Auto-capture is on" notice link to
Preferences, which scrolls to and briefly highlights that row. Linux shows the note
as plain "will show" text. Full screen shows the display name and size (plus FPS in
Record). Record mode shows labelled FPS / Max resolution selects, Show cursor /
Show clicks / Desktop audio switches (On/Off/Unavailable with a reason tooltip;
clicks imply the cursor) and the microphone select. The primary button hides under
auto-start unless a start failed. Guidance stays until a window is selected, hides
while dragging and fades when the pointer comes within 28 points. Segmented
indicators slide and the Record row arrives as shipped;
Wayland remains open; physical
displays, platform input and accessibility acceptance remain open.

Configured Region/Window/Full screen global shortcuts switch the open selector's
target without replacing its session. Like the shipping keyboard path, every
target key clears hover; Region/Full screen clear the selected window, while
Window retains it. Settled region/aspect remain. Keyboard Full screen does not
auto-start; pointer selection still follows that preference. New Capture cannot
re-enter an open selector. Shared generation checks reject queued/held keys when
selection ends or display preparation starts; countdown and capture stay blocked.

The toolbar drags from blank/footer space, clamps inside the display, and fits a
768-point viewport without hiding the picker or Capture action. Region guidance
hides during a selection drag. AppKit XCTest renders empty, selected, auto-start
and narrow controls; inspect those native pixels alongside the assertions.
`x11_capture_smoke.py --controls` reuses the exact-pixel capture oracle through
New Capture, including target retention, toolbar drag, frozen/live/countdown
sources, occluded windows, full-screen capture and cancellation. Its seven
scenarios persist 15 captures using real X11 input and simulated session state;
add `--target-shortcuts` to exercise target keys, window clearing/reselection,
same-selector identity, keyboard auto-start suppression and countdown isolation.
This is not hardware or physical multi-display acceptance.

## Capture History

The live workspace renders History like the shipping `CaptureHistory` window:
an "On this device" / **Capture History** header with the 30-day lede, counted
All/Screenshots/Video/GIF filter pills (hidden while History is empty), the
**Interrupted recordings** card, and an auto-fill grid of cards (minimum 252 pt,
16 pt gaps, 168 pt thumbnail). Each card shows the thumbnail (`contain` fit),
date ("Sep 26, 2026, 3:04 PM" in local time), "W × H · size" plus duration for
recordings, and dropped-frame warnings. Screenshots offer **Edit** and **Restore**,
recordings **Edit** and **Save file**; after a recording is exported its second
action becomes **Show in Folder**. Clicking the thumbnail opens the editor. A recording
whose media is gone shows **File missing**, no actions, and is removed with one
click. Otherwise the trash control arms **Delete forever** for four seconds and
deletes on the second click. Secondary click lists the card's commands, including
**Copy image**, **Save image** and (once exported) **Show in Folder** for screenshots. Loading, empty ("No captures yet") and load/delete
error states use the shipping copy.

Copy, card details, actions, the missing-media rule and grid metrics come from
`captures_app::history_view`; AppKit reads them through the `history_copy`,
`history_cards` and `history_grid` settings operations. Both hosts virtualize
cards by row and decode thumbnails off the UI thread with bounded residency.
Unlike shipping, History never selects a card on load; an explicit selection
(click, arrow keys, a new capture or import) shows the accent ring, arrow keys
move it, Return opens it and Escape backs out of an armed deletion. The workspace
also keeps its native capture controls above the grid; the window is not resizable
on macOS.

**Restore** brings a screenshot back as a floating mini preview, like shipping: the
button reads "Restoring…" and then "✓ Restored" for 2.5 seconds, with the tooltip
"Bring this screenshot back as a floating preview". The card joins the front of the
existing pile (or opens one on the selected display) without copying to the
clipboard or changing History. If that screenshot's preview is already showing it
stays where it is and the button still confirms. Failures appear in the workspace
status line. History **Edit** on a screenshot restores its preview the same way
before opening the editor, like shipping; a failed restore opens nothing.

## Live display-capture slice

Launch with `--live [--history-root PATH]` on either native host. This is an
explicit opt-in to real desktop capture, not a synthetic benchmark. The default
history is beside the separate Captures Native settings file, never installed
Preview history. Capture from the tray, the shortcuts or New Capture, on the display
under the pointer; History reloads itself and has no capture controls, as in shipping.
The host hides its History and Preferences windows before capture and restores
their prior visibility afterward. Capture History, Captures Preferences and first-run
setup are separate, resizable windows, as in the shipping app; `--live
--open-preferences` also opens Preferences at launch on Windows/Linux.
Permission and locked/inactive session checks remain in force.

Both hosts use `captures-app` for display enumeration, PNG/thumbnail persistence,
history recovery, save and delete. Image files cross the ABI as paths, not base64.
Capture, file work and preview decode run off the UI thread. **Save image** uses
the output folder and PNG/JPEG/WebP format selected in Preferences, without
overwriting an unrelated file. Repeat Save reuses the existing export; a missing
export can be recreated from history. History always retains the lossless PNG.
JPEG composites alpha onto white; WebP saves losslessly, using the same encoders
as the shipping application. Deleting history preserves all exported formats.
**Delete all** arms **Delete all forever** (with Cancel) for four seconds; the
second click deletes the workspace's screenshot, video and GIF history copies,
including entries outside the selected filter. It leaves exported files, recording
recovery drafts and other history roots untouched. Cancel, Escape or the timeout
leave history unchanged. Both hosts reload after a failure, including partial
deletion, and keep the error visible. This also works with existing
local history on Wayland; the live capture restriction is separate.
Captures not deleted remain available on reopening the workspace.

Automatic copy follows Preferences (enabled by default, like shipping Captures).
Turn it off to leave the clipboard untouched by a new capture; explicit Copy
still works. A clipboard failure does not discard the captured image. Only an
explicit capture action can trigger automatic copy, never loading history or a
fixture screenshot. Save/capture report settings errors rather than silently
using different output or clipboard defaults.

Screenshot countdown follows the stored 0–10-second preference. The selected
display shows a native, fixed-media-palette countdown. Escape is registered only
for an active capture and cancels even when another app has focus. If registration
fails, capture is refused rather than losing cancellation. A desktop-session
watcher invalidates the pending capture on lock/inactivity; unlocking does not
resume it. Monotonic deadlines and capture generations are shared Rust logic.
The overlay closes before the host hides/settles and captures. Late worker replies
cannot restart a cancelled capture. Once the captured pixels commit to saving,
Escape no longer claims cancellation; accepted history/file work drains at quit.
Timers stop and Escape is released after completion/cancellation (the Windows
low-level capture hook stays installed but disarmed). Interactive permission,
focus, mixed-DPI, compositor, and screen-reader acceptance still needs real OS tests.

Cursor inclusion follows `show_cursor_in_screenshots`: the shared Rust engine
samples the pointer after countdown/window hiding and composites it before
history, copy, and export. This preserves shipping behavior: macOS system pixels
and hotspot, a synthetic arrow on Windows/X11, and no overlay when the pointer
is outside the selected display or unavailable. Wayland-only cursor acquisition
remains unsupported. Exact cursor shapes on Windows/Linux are not claimed.

Both native hosts offer **Capture region** on the selected display.
Draw from an empty selection, move it or resize its corners, choose Free/1:1/4:3/
3:2/16:9/9:16, and hold Shift for a square while dragging. Release Shift to restore
the chosen aspect. Confirm with Enter/Capture, or enable automatic start on
selection in Preferences. Escape covers preparation, selection and countdown.
Freeze follows Preferences; zero countdown uses the retained frame/cursor, while
any countdown captures fresh pixels. The AppKit panel borrows Rust-owned pixels
through an image provider and releases them after closing/worker completion.
This adds no full-desktop temporary file. `--scene region` provides a synthetic
selector using the same view without screen access; `--exercise` covers draw,
move, aspect, resize and Shift release. The Windows/X11 candidate uses the same
Rust `RegionSession`; its [private-X11 integration test](wgpu/README.md#validate-and-collect-evidence)
checks repeated captures, exact saved pixels and cancellation with simulated
session state. Real OS capture, mixed-DPI and accessibility acceptance is still
required. Selector glass panels approximate the shipping backdrop blur with the
near-opaque fixed glass fills.

Both hosts also offer **Capture window**. Hover a window or desktop, click to
select and confirm with Enter/Capture; automatic start confirms on click. Shared
Rust hit testing handles front-to-back targets, shell strips, half-open bounds
and platform coordinates. The retained `WindowSession` owns target descriptors,
frozen pixels/cursor and safe composited-crop/native-surface selection. If another
window covers the target, capture uses its native surface rather than saving the
covering pixels; this fallback reads current pixels even in frozen mode. A
countdown refreshes window geometry and pixels. A target that disappears or moves
to another display fails rather than saving stale bounds. Desktop/shell selection
saves display-mode history. AppKit calls the allocation-free Rust hit-test and
corner-radius ABI, not a Swift copy of the targeting or masking policy.

The private-X11 test checks two captures in each region/window scenario, exact
asymmetric PNG pixels and metadata, frozen versus fresh countdown and live
capture, an occluded window's native surface, a disappearing countdown target,
desktop fallback, clicked-target confirmation after moving the pointer, automatic
start without Enter, cross-application Escape and simulated lock/unlock. This is
software-rendered X11 integration evidence, not Mac/Windows hardware, Wayland, accessibility or
real login-manager acceptance. `--scene window` is a permission-free fixture on
both hosts. The slice remains experimental and all platform acceptance gates
stay open until real desktop/input tests pass.

Full UI parity remains open. The wgpu Wayland host now unmaps Captures' windows,
waits for compositor-processing acknowledgements, and requests a Screenshot
portal still from History. It restores the windows on success, cancellation or
failure. Captures return to History without guessed display geometry or preview
placement. **Record display…** uses the same unmapping acknowledgement before
portal consent, then the normal MP4 recording controls with compositor placement.
Region/window selectors and screenshots during a recording remain unavailable;
recording controls are included in output and Hide needs a working tray.
Consent and cursor inclusion are portal-controlled. Remapping may change
compositor-assigned window positions. Linux X11 needs an
active, unlocked desktop session; bare Xvfb normally has no session service and
must refuse capture. Verify real permission, clipboard ownership, multi-display
behavior and exported pixels on each OS before accepting the slice.

## Screenshot mini-preview stack

Both live workbenches retain recent screenshots in fixed-glass native cards.
Copy uses full-resolution pixels, Save uses current screenshot preferences and
becomes Reveal after export. Trash moves only that export to the OS trash,
then dissolves the card; an unsaved card's Delete only dissolves the preview.
Errors keep the card available for retry. Private History files and metadata remain untouched.
macOS uses Finder (which may request automation permission), Windows uses the
Recycle Bin, and Linux uses its desktop trash specification. Actual Finder and
Recycle Bin behavior still needs physical-host acceptance.
Edit opens the exact screenshot in its native editor without showing a hidden
workspace, switching Preferences, or changing the selected History row. Repeated
Edit focuses the existing editor without resetting its pending edits.
While that editor is open (or minimized) the card shows the shipping "In
editor" pill and accent ring; the pill offers "Show in editor" on hover or
focus and focuses the editor. Closing it eases the ring out and leaves the
Edit icon visible for 3 seconds. Hover blurs, darkens and slightly enlarges the
card media, but not while the pointer still rests where an expand or a new
capture left it: chrome waits until it moves. Card icons and Clear all show
instant glass tooltips instead of system hover text.
Dismiss closes only the targeted card. Clear all dismisses a snapshot of the stack, preserving history,
exports and any later capture. There is no automatic dismissal timer or count cap.
A failed copy shows the shipping "Clipboard unavailable" warning beside the
metadata until a copy of that capture works.

Stack motion follows shipping and the shared `captures-app::preview_motion`
data. Close streaks the card toward the pile's screen edge; Delete dissolves it
into dust from the trash control (AppKit filters chips with Core Image, wgpu
paints them as a textured egui mesh; AppKit falls back to the shipping
scale-and-fade when Metal is unavailable). The exiting card keeps its slot while
older cards slide into it after the shipping delay, then the window resizes.
An overlapping deletion freezes the exiting card and affected survivors at their
current presentation (zero is valid), holds until the new exit is ready, then
eases the accumulated distance. Removing a held slot rebases the Rust trajectory
without moving its survivor. AppKit retargets its Core Animation presentation
rather than scheduling independent model-frame shifts.
Clear all streaks every card out, bottom first. Show less and expand fly the
cards between the list and the compact pile, and the stack toolbar enters, leaves
and clears with its shipping keyframes; the Show less pill morphs over 240 ms.
New cards fade an accent capture highlight, main-action glyphs pop when they
change, the clipboard chip arrives with its bounce and a hovered pile sparkles.
Reduce Motion skips every exit, flight, highlight and sparkle. The Close streak
steps through shipping's horizontal Gaussian filters on wgpu (AppKit uses Core
Image motion blur); dust chips carry only the pre-blurred hover media, not their own
dissolve blur.

Run `/usr/bin/python3 apps/native/x11_preview_smoke.py --retarget-only --binary
apps/native/wgpu/target/debug/captures-wgpu-workbench --output /tmp/native-retarget`
with a fresh output folder to exercise three-card dust holds in all four corners.
It compares the survivor's media-edge pixels past the first settle deadline,
checks the final actionable card and preserves original History bytes. Shared
clock-driven tests cover zero, just-started settle and slot-pruning boundaries;
AppKit tests read presentation-layer positions, not model targets. Software-X11
and macOS CI do not establish physical Windows/macOS or Wayland preview acceptance.

Stacks start expanded, with newest cards nearest the configured top/bottom edge.
Overflow scrolls without dropping captures; chevron cues at the stack edges
scroll one card at a time. Show less parks a compact pile with
the newest card in front; clicking it expands the stack. Incoming captures and
capture cancellation preserve the parked state. Collapsed piles drag within
their capture display and fan on hover, respecting reduced motion. Rear cards take
the shipping pile pose from `captures-app::preview::collapsed_card_pose`: the
per-capture 2.7–3° spin (faded in as a dragged pile nears the vertical middle),
depth recession through the 900 px perspective, depth scale and peek jitter.
The whole 3D pose, `rotateX` keystone included, reaches both hosts as a shared
projective map; rear media blur with depth (less while fanned), and a hovered pile
takes the shipping accent ring and glow. Cards and their controls draw the shipping
box shadows (Core Animation shadow layers on AppKit, cached Gaussian masks on
wgpu). Glass surfaces do not blur what is behind them yet (`backdrop-filter`).

Show mini previews, all four placement corners and Include mini previews in
captures use the shared settings. Turning previews off hides retained cards and
resets collapse; enabling them restores an expanded stack. By default the host
hides the stack before preparing/capturing pixels, restoring it on cancellation/
error. Out-of-order decodes preserve capture order; late results cannot resurrect
dismissed cards. AppKit keeps actions alive
when Preferences replaces the workspace; closing the app closes the panel too.

The implementation reuses `captures-app::preview` for corner placement,
work-area/DPI math, membership, card poses, scroll content height and capture/
decode visibility generations. AppKit calls this policy through the versioned
`captures_preview_*_v1` ABI, `NativePreviewStack` and `NativePreviewPolicy`;
Windows/Linux calls Rust directly. Work areas come from `NSScreen.visibleFrame`,
the audited Windows `rcWork` query, or X11 EWMH properties clipped to the monitor.
If the candidate cannot query usable bounds, the screenshot stays in history and
the host reports that it cannot position a preview rather than guessing.

macOS uses a nonactivating panel; X11 uses an unmanaged notification window to
avoid activation by the window manager. Keyboard focus on a card reveals its
controls like shipping `:focus-within`. The macOS panel takes keyboard focus only
while Captures is active (Ctrl-F6, Cmd-`), and clicks never make it key. The X11
window never receives keyboard focus, so its controls are pointer-only there. Private-X11 tests cover real pixels,
placement, focus and actions; `x11_preview_smoke.py --stack` additionally checks
per-card routing, compact arrivals/cancellation and nondestructive Clear all in
all four corners, plus eight-card overflow at bottom-left. CI runs this stack
mode. macOS CI covers AppKit/ABI lifecycles and renders.
Physical macOS/Windows desktops, mixed-DPI monitors, compositor behavior,
transparent hit-region parity and screen-reader/keyboard access still need
acceptance. Wayland mini-preview positioning remains unsupported; portal
screenshots return to History instead.

## Build and try on macOS

Requires macOS 13+, Xcode command-line tools with Swift 5.9+, Rust 1.94, and Node
24 at build time. Node compiles design tokens and test fixtures; it is not bundled.

```sh
bash apps/native/macos/build.sh
apps/native/macos/.build/release/CapturesNative --scene preferences
apps/native/macos/.build/release/CapturesNative --live
apps/native/macos/.build/release/CapturesNative --scene history --history-count 1000
apps/native/macos/.build/release/CapturesNative --scene history --history-count 0
apps/native/macos/.build/release/CapturesNative --scene hud --appearance light
apps/native/macos/.build/release/CapturesNative --scene preview
# Compare identical workbench content using the original per-chip filter strategy:
apps/native/macos/.build/release/CapturesNative --scene preview --reference-chips
# Update notice fixture (stub status source; no updater is connected):
apps/native/macos/.build/release/CapturesNative --scene update --update-state error
```

Close each instance before starting another. Cmd+Q quits. The menu bar matches
shipping's default macOS menu: Cmd+W closes the front window through its normal
close path, Cmd+M minimizes, Cmd+H hides and text fields support Undo/Redo.
Nothing installs into Applications or changes the installed app's data, shortcuts,
or updater. `--live` does unbind overlapping system screenshot keys like shipping (see above); set
`CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER=1` to keep them.
Only the explicit live screen-access action requests capture permission.
The executable needs its SwiftPM resource bundle; run it from the build directory.

Preferences has section navigation and a Capture History fixture action.
Appearance/theme changes save automatically; history uses reusable native table
rows, with an empty-state switch. HUD Pause /
Resume changes fixture state; the timer is deliberately static. Preview has cold
and warm dissolve buttons plus Reset. Reduce Motion uses an immediate change.
`--exercise` runs six scripted actions (appearance changes, history end-to-end
scroll, HUD pause/resume, or alternating cold/warm dust); `--quit-after 30` exits
automatically. `--scene idle` creates no visible window.
`--scene update` opens the update notice in a transparent panel below a
simulated menu-bar icon. `--update-state` accepts `available`, `single`, `closing`,
`manual`, `downloading`, `restarting`, `error`, `checking` or `up-to-date`.
Buttons in the window switch states. Update now and Try again run the shared stub
through download progress and the restart countdown. Nothing is downloaded,
installed or relaunched, and links are logged rather than opened. Hide / What’s
new persists only to an explicit `--settings-file`.

These screens are **not full pixel or functional parity**. Native Preferences
includes Find, custom colors, persisted defaults and live system appearance.
The fixture scenes use synthetic history, recording and preview state; image-backed
History, real recording, the preview pile and both editors run under `--live`.
Do not claim whole-app savings from these development scenes.

## Checks and measurements

```sh
node --test scripts/native-tokens.test.mjs
python3 -m unittest discover -s apps/native -p 'test_*.py'
# Included by build.sh; generated resources must exist first:
swift test --package-path apps/native/macos -c release

# About 40 minutes: 10 workloads × (warmup + 3 trials) × 60 seconds.
# Output must name a new directory. No screenshots or capture access requested.
python3 apps/native/profile.py \
  --binary apps/native/macos/.build/release/CapturesNative \
  --output /tmp/captures-native-results
```

Development/test profiles use wrapping arithmetic only for `tiny-skia 0.11.4`
in both Cargo workspaces. Its scalar Overlay pipeline evaluates an unused
expression that can underflow; wrapping matches its SIMD and release arithmetic.
Application overflow checks stay enabled. Release settings and rendered pixels
are unchanged; re-evaluate the override when updating that dependency.

The runner records raw CPU counters, RSS samples, binary hash, readiness, and
separate effect/scene-construction timings. It fails on missing actions, premature
exit and effect errors. **RSS is not physical footprint**. CPU is process-only;
there is no WindowServer/helper accounting, presentation timing, wakeup/energy
measurement, or original-Tauri comparison in this runner. Use the tested #529
coalition/resource and frame-observer protocol for renderer comparisons, then
Instruments Time Profiler / Animation Hitches / Energy Log for the broader matrix
in `docs/native-rewrite.md`. Readiness and scripted-action timings measure CPU
submission, not first displayed pixels or hardware input latency.

The shared screenshot compositor's focused release benchmark compares a borrowed
canvas with consuming the fresh canvas allocated by the native editor:

```sh
cargo test -p captures-image --release --locked benchmark_native_canvas_4k -- --ignored --nocapture
```

It checks identical pixels, includes allocation/fill and painting, discards one
warmup, then reports five alternating-order trials of three renders each. Retain
the raw trials, compiler and source/binary identity; repeat on the same idle host.
This fixture excludes decoding, UI, encoding, presentation and process footprint.
The `opaque-bitmap` fixture covers direct composition of one fully opaque,
full-canvas identity image, which avoids the second full-size raster plane.
Partial alpha, transforms, shadows, non-normal blends and later layers retain
the raster path. The phase diagnostic below also measures the extra opacity
scan when only the final image pixel is non-opaque; report this cost as well.

The compositor retains a 64 KiB channel lookup built from tiny-skia's exact
demultiplication. Measure its one-time initialization alone in a fresh process
(the normal render benchmark warms it before timing):

```sh
cargo test -p captures-image --release --locked benchmark_demultiply_cache_initialization -- --ignored --nocapture
```

Large normal renders also build a temporary 256 KiB lookup for composition over
the first source pixel, which usually matches the native canvas fill. It caches
the exact, separately rounded dependency operations, uses the old path for other
background pixels and is freed after each render. Renders of at most 65,536
pixels and non-normal blend modes do not allocate it. Profile premultiplication,
composition over transparent/partial/opaque fills, the mixed-background fallback
and table construction with:

```sh
cargo test -p captures-image --release --locked benchmark_native_compositing_phases_4k -- --ignored --nocapture
```

This phase diagnostic excludes source preparation/fill and warms the process-wide
demultiply cache. It discards one warmup, reports five single-operation trials
and compares fallback pixels with the reference loop in alternating order.
Use `benchmark_native_canvas_4k` for full-render costs including allocation/fill
and table construction, and compare frozen old/new binaries in alternating order.
Record the unfavorable fallback cost as well as the repeated-background gain;
neither diagnostic is whole-app latency, memory/energy or physical-host acceptance.

The shared editor worker reuses its existing canvas frame for layer Lock and
Rename commands. Rename also keeps the layer thumbnail; labels, lock state,
history and drafts still update. Visual edits and Undo/Redo retain their normal
render path. Measure command execution, thumbnail refresh and snapshot JSON with:

```sh
cargo test -p captures-app --test editor_session --release --locked benchmark_native_metadata_4k -- --ignored --nocapture
```

This uses a 3840×2160 mixed-alpha source on the default solid canvas background,
checks unchanged pixels for metadata edits, discards one warmup and reports five
three-command trial means. Opacity is a repainting control. Compare old/new
binaries in alternating order on the same idle host. Timings exclude fixture
setup, request parsing/queueing, draft I/O, encoding, host pixel transfer and
presentation; they are not input-to-screen or physical-platform acceptance.

For visual review, capture the specific workbench window using macOS Screenshot
(not your whole desktop), in light/dark, history empty/populated, HUD running/paused,
and preview before/during/after deletion. Test both 1× and 2× screens, keyboard
focus/Space activation, Reduce Motion, reset during an effect, moving between
screens and closing the preview. Send the captures and raw diagnostics together.

## Effect provenance and unresolved gates

`DustMath.swift` is imported unchanged from the
[tested AppKit reference](https://github.com/joswayski/captures/commit/9fa5698528ffafa59fc4a71df9a7c50c8cc6e42e).
Build-generated particles and expected poses come from shipping
`thumbnailExit.ts`; the large pose oracle is bundled only into tests, not the app.
The Swift differential test covers uneven times and each delay boundary.

The atlas candidate filters isolated, padded chips once instead of issuing one
Core Image render per chip. A 1×/2× static chip pixel test compares it to per-chip
filtering under both normal and flipped preview parents, and rejects blank output.
CI uploads representative rendered chip pairs as `native-pixels` for inspection;
set `CAPTURES_TEST_ARTIFACTS` to an output directory to retain them locally.
A missing Metal device explicitly skips these tests; a skip is **not** acceptance.
The full original WindowServer visual gate is
still required: this workbench's synthetic image and sampled Core Animation
keyframes differ from #529, and its source fade does not yet reproduce the original
blur/brightness/scale treatment. No performance or visual-parity result is claimed.

All cold texture preparation and animation construction are timed on the main
thread in this experiment so setup cost cannot disappear from reports. This is
not the production scheduling policy: if batching still blocks input, prepare
bounded resources off-main before use and supply a responsive fallback for a
cache miss. Warm reuse holds only one surface's textures, invalidated on backing
scale changes; switching scenes releases them. Core Animation plays submitted
keyframes without a recurring application timer/display link; completion has one
cleanup callback. Profile keyframe construction and residency too, not just blur.
