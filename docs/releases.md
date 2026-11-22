# Captures releases

## Public channel automatically tracks native

Native (AppKit, Win32, GTK4) is the public application since November 2026. The
maintainer approved the switch after staging over-the-air updates passed on his
Mac and Windows PC, both native to native (1501 → 1502) and Tauri to native
(Tauri 1505 → native 1506). Main pushes with unpublished native/shared desktop
changes build and promote a release after all platform, signing, notarization and
complete-package checks pass. The egui/wgpu workbench never ships.

`release.yml` builds each target with `native-preview-build.mjs build`, signs the
version- and target-bound updater artifacts with `collect`, and writes
`latest.json` with `manifest` (including the `linux-x86_64-deb` entry that
existing Tauri `.deb` installs select). Installed Tauri apps update to native
once through their normal updater, using the same identity and signing key; the
native app then imports the Tauri profile read-only into its own profile.

Public downloads, `npm run install:latest`, and both updater endpoints use the
highest published CalVer that `scripts/preview-release.mjs` accepts: releases
carrying the `captures-native.json` runtime marker, uploaded only after the native
packaging job passes validation, plus Tauri releases (`captures-tauri.json`, and
history through `v2026.10.11.4`). Markers are included in `SHA256SUMS`. Unmarked
releases, including the withdrawn October native `v2026.10.18.1`, are excluded.
The release baseline, public changelog and website publishing status use the same
filter. Old October native installs read the public feed and can now update to
the current native release.

Updating cannot return to Tauri. To go back, quit Captures and install a Tauri
build from the archive manually; the original Tauri profile remains separate from
native-only captures and edits. Do not delete either profile or assume a reverse
migration exists. Rebuilding (`target_sha`) a commit from before the switch is
unsupported, because the workflow now packages native.

To refresh the public installers and updater metadata without rebuilding apps:

```sh
gh workflow run release.yml --repo joswayski/captures --ref main -f channel_only=true
```

All four installer aliases and `latest.json` are copied from the newest public
release. Verify the public feed and installer digests after this job, not merely
the workflow dispatch. For a broken release, fix or revert on main and publish a
higher version; an older version will not downgrade installed apps. Prove
updater changes on the native staging feed (below) before they reach main.

## Native packaging and recovery

The native public rollout has been withdrawn pending parity. Earlier native
archives used AppKit (macOS Apple Silicon) and Rust/wgpu (Windows/Linux x64)
and remain experimental; physical acceptance is incomplete. Native builds are no
longer published by `release.yml`. `scripts/native-preview-build.mjs`
builds the native executable and updater helper, verifies their build identity,
then uses the existing pinned Tauri CLI only to bundle installers. It does not
build or embed the Tauri app, frontend or WebView2 installer. `CI` can be manually
dispatched on a branch to build unsigned installers without release secrets, and
builds them nightly; nothing is uploaded to a release.

The builder now packages the replacement hosts, not the maintenance-only wgpu
workbench: AppKit for `aarch64-apple-darwin` (`.app` and `.dmg`, unchanged),
the Win32 host (`apps/native/windows`) for `x86_64-pc-windows-msvc` (NSIS) and
the GTK4 host (`apps/native/gtk`, default `x11` feature, no `layer-shell`) for
`x86_64-unknown-linux-gnu` (AppImage and `.deb`). Each Win32/GTK host is built
from its own Rust 1.95 workspace into `target/native-<renderer>-preview` after
`apps/native/prepare.mjs` writes its git-ignored `resources/` (tokens, dust and
icons are compiled in). The host must answer `--preview-build-info` with
renderer `win32` or `gtk` and the requested version before it is copied to the
bundler's main binary, so installed executables keep the Tauri names
(`captures.exe`, `/usr/bin/captures`, `Captures.app/Contents/MacOS/captures`)
that the installed app's updater requires. `Target::package_executable`
(`CapturesNative.exe`, `captures-native`) names development archives, not these
installers. Packages also carry the target-suffixed FFmpeg/FFprobe sidecars,
their notices and `native_preview_update_helper`.

A host built with `CAPTURES_NATIVE_PREVIEW_VERSION` starts as the resident app
(`--live`) on the imported native profile and holds the package-use lock, as the
wgpu host did. The Win32 host now also starts the startup recovery companion
described below, acknowledges readiness (settings, Direct2D device, bundled
FFmpeg/FFprobe) for both the companion and the NSIS `/ARGS` relaunch, checks the
release channel at launch and every five minutes, and installs from its
Preferences Updates card (Update now downloads and verifies, then starts the
copied helper and quits), and shows the floating update notice for new versions
and failed installs. Win32 opens
plain path and `file:` URI arguments, so Open With and file associations reach
the editors, and forwards them to a running instance. None of this has had a
physical Windows, real NSIS install or real update run. The GTK host does the
same: it starts the companion, acknowledges readiness (settings, a GDK display,
the packaged FFmpeg/FFprobe) for the companion and the helper's relaunch, checks
at launch and every five minutes, and installs an AppImage from Preferences
(Update now downloads and verifies, starts the copied helper with the open
previews, editors and Preferences, and quits; the helper replaces the AppImage
and relaunches it), and shows the floating update notice for new versions and
failed installs. A `.deb` install cannot replace itself: its card offers View
release and the captur.es fallback, the manual package update path. No GTK
AppImage or `.deb` has had a real update run. GTK opens the desktop entry's `%U`
files. The `.deb` depends on `libgtk-4-1` (GTK 4.6 or
newer) in addition to the WebKitGTK/GTK 3 entries the CLI always adds. The
AppImage carries `libgtk-4.so.1` and its libraries (linuxdeploy follows the
binary's dependencies), but the CLI's GTK plugin is forced to GTK 3: it also
bundles WebKitGTK/GTK 3 (about 130 MB in all), points `GTK_PATH` at `gtk-3.0`
and exports `GDK_BACKEND=x11`, so the AppImage runs the GTK host through X11 or
XWayland, never its native Wayland path. Neither host is accepted for cutover.

Previously published native releases required all three platforms, Apple signing/notarization,
FFmpeg notices, checksums and a draft before publication. Native artifacts use
version/target-bound filenames in their signed Minisign trusted comments; the
compiled helper authenticates the collected bytes before upload. `latest.json`
keeps the existing Tauri-compatible platform keys and public signing key, so
installed Tauri apps keep updating to native even though the Tauri source is
gone from the repository. The manifest retains a separately signed
`linux-x86_64-deb` artifact for existing Tauri Debian installs: their updater
cannot install an AppImage fallback. Once on native, Debian installs use the
manual package update path.

Native per-file update patches (`capturespatch1`) have release tooling that no
workflow runs; they serve only signed schema-2 development manifests. The public
feed uses [patch updates](#patch-updates) instead. For each release it would:

1. `scripts/native-update-retain.sh download "$BASES"` fetches the retained full
   archives (`<version>--<target>.<tar.gz|zip>`) from the draft
   `native-update-bases` release. A missing release is a warning.
2. Build `native_update_patch` (`cargo build -p captures-app --bin
   native_update_patch --locked`), then run
   `node scripts/native-update-patches.mjs generate --manifest MANIFEST.json
   --tool target/debug/native_update_patch --bases "$BASES" --archives
   NEW_ARCHIVES --output PATCHES --patch-base-url HTTPS_RELEASE_DOWNLOAD_URL`
   on the unsigned schema-2 manifest. It patches from each target's six newest
   earlier bases, lists only patches strictly smaller than half the full
   archive, keeps the manifest within 128 KiB by dropping the oldest offers,
   and reports failures as `::warning::` without failing the release. It never
   signs: upload `PATCHES/*.capturespatch` with the archives and sign the
   manifest it wrote with the existing signer.
3. Once the signed manifest is live,
   `RELEASE_TAG=<tag> scripts/native-update-retain.sh retain MANIFEST.json
   NEW_ARCHIVES` deletes patches the manifest no longer lists, uploads this
   build's archives as bases and keeps six builds per target. Run it with
   `continue-on-error` and a `contents: write` token; `--dry-run` prints each
   create, upload and delete instead.

Without bases or patches, clients download the full archive.

The installed native profile lives under the platform-local `Captures Native Preview`
data directory, in `profile`. First launch copies previous settings, retained History
and screenshot drafts, preserving the old source and exports; subsequent launches
validate the marker and reuse that copy. If the previous app's settings or data
changed after the import (someone returned to Tauri and later updated to native
again), the next launch renames the stale copy to `profile-superseded-<unix time>`
beside it, kept rather than deleted, and imports again. Do not launch another old or native app
during this first import. The process check is a best-effort guard, not a lock
honored by old Tauri versions. Development workbenches remain separate.

Native migration retains History's saved-file references, including reference-only
recordings created by Open With. It neither reads nor copies external media; missing
files remain missing History entries. Development imports still reject reference-only
recordings and clear saved-file links so test workbenches cannot modify installed exports.
If an earlier native release exits with `Reference-only recordings must be reconciled
before import`, install a fixed release over the app and retry. That failed import
publishes no native profile; do not delete or edit the original History to work
around it.

macOS/AppImage native updates keep `.Captures-previous-<uuid>.app` / `.AppImage`
next to the installed app until the replacement acknowledges renderer, settings
and bundled FFmpeg readiness. Failure retains that backup and the private temporary
`captures-preview-*` directory (`install.log`, `install.json`, `backup.json`). Quit
every Captures process before restoring the backup to the `installed` location in
`backup.json`, or reinstall from the download page. Do not delete the native profile
to repair an executable. There is no automatic timeout rollback or profile rollback.
Windows delegates replacement/relaunch to NSIS; Debian remains a manual update.

### Native staging feed (update testing)

`.github/workflows/native-staging.yml` (manual dispatch on main, input
`version` such as `2026.11.1401`) builds signed, notarized
`native-preview-build.mjs build TARGET VERSION --staging` packages for all three
targets. Staging builds use the side-by-side "Captures Native Test" identity
(`captures-native-test`, `io.github.joswayski.captures.nativetest`, no file
associations, a separate `CapturesNativeTest` login item), so they install next
to the public app. Unlike plain `--side-by-side` test builds, their updater is
on, but it reads only the compiled-in
`https://github.com/joswayski/captures/releases/download/native-staging/latest.json`
and replaces only the test identity. Each run publishes a
`native-staging-<version>` prerelease and repoints the `native-staging`
release's `latest.json` at it. These tags never match the public
`vYYYY.MM.DD.N` selector, and `/api/updates/preview` proxies only the `preview`
release, so public clients never see them. To prove over-the-air updates,
install one staging build, dispatch a higher version, and update from the app.
Delete old `native-staging-*` releases when no longer needed. Staging builds
still import the installed profile on first launch into the shared `Captures
Native Preview` profile directory.

### Patch updates

An update from a recent release can download a patch instead of the whole
updater artifact (`crates/captures-app/src/updater/preview/delta.rs`), as dbm
does. Each platform in `latest.json` may list `patches`: `{from, url, size,
signature}`. A patch is a bzip2 bsdiff that rebuilds a signed release file
from the installed copy, so the result faces the same version- and
target-bound signature check as a full download:

- **AppImage:** from the running `$APPIMAGE` to the new AppImage. Its squashfs
  stays compressed, so the patch carries the changed files' compressed blocks
  (17 MB of 113 MB from 2026.11.21.9 to .10).
- **macOS:** the `.app.tar.gz` is compressed, so patches rebuild
  `Captures-<version>-<target>.app.tar`, a deterministic tar of the bundle
  (sorted entries, regular files and folders only, modes 755/644, no owners or
  times). The client tars the installed app the same way; the helper installs
  the rebuilt tar like the `.tar.gz`. Only that tar's `.sig` is published (1 MB
  of 32 MB from .9 to .10). DMG and updated installs hold identical bundles.
- **Windows and Debian:** always the full download. The NSIS installer is
  solid-compressed (a bsdiff is as large as the installer) and leaves no copy
  to patch from; patching Windows would need an install path that replaces
  files without NSIS.

A missing or changed base, a failed download or a signature mismatch falls
back to the full artifact in the same attempt, so a patch only makes an update
smaller. Clients from before patch support ignore the field.

`scripts/native-preview-patches.mjs generate` runs after `collect` in each
package job. It downloads the target's artifact from the five newest earlier
native releases of the same feed (public releases with `captures-native.json`,
or `native-staging-*`), writes `Captures-<version>-<target>.from-<base>.patch`
with `native_preview_update_helper --make-patch` (which checks that the patch
rebuilds the new file exactly) and, for macOS, signs the bundle tar from
`--bundle-tar`. Patches not smaller than half the full artifact are dropped.
Failures are warnings; the release continues with full downloads. `manifest`
lists whatever patches the release carries. Patches stay on their releases.

The staging feed always makes patches. Public releases make them only when the
repository variable `CAPTURES_UPDATE_PATCHES` is `true`. Prove a patch update on
staging first: install one staging build, dispatch a higher version, and check
that Preferences offers the patch size and the update installs. Then:

```sh
gh variable set CAPTURES_UPDATE_PATCHES --repo joswayski/captures --body true
```

### Startup recovery companion

Installed hosts start a copied `native_preview_update_helper --watch-startup`
before profile import. The GUI remains the original app executable, preserving
LaunchServices/Open With routing; this is not a wrapper app, login service, or
second tray. A per-launch file lock tracks actual host lifetime without PID reuse.
The existing renderer/settings/bundled-media readiness gate ends monitoring;
successful secondary forwarding and normal Quit explicitly cancel it. Healthy
launches perform no recovery HTTP requests. Ordinary in-app updates still require
user confirmation; failed-startup recovery automatically installs a signed newer
release using the same pinned key and version/target binding.

The helper reads only its private request/health files and installed executable,
never settings, History, migration markers or exports. Its scratch directories are
`startup-recovery/watch-*` alongside (not inside) the native `profile` directory.
They contain `recovery.log`, `status.json` on failure, and the retained install
request/backup journal if replacement fails. Completed sessions are pruned on the
next launch. Repaired launches run first-launch import normally rather than
passing explicit empty profile paths that would bypass migration.

After 120 seconds without readiness, an OS dialog asks the user to quit a hung
app. Recovery waits up to ten more minutes for actual exit and never kills an
importer or assumes timeout means it is safe to replace files. A repair lock
serializes failed launches; the installer still requires every native host's
package-use lock to release. It rechecks the captured package fingerprint under
that lock, so an older helper cannot overwrite a manual replacement or another
update. Recovery makes at most 30 checks with one minute between attempts (plus
bounded HTTP timeouts). Reopening retries later if no fix was published. A
persisted attempted-version marker prevents repeatedly installing the same or
an older repair; failed downloads do not consume that attempt. There is no blind
application downgrade or user-data rollback. OS permissions may still require
manual intervention. Debian directs the user to its installer.

Failure dialogs use the OS facilities (AppleScript / Windows MessageBox;
zenity, kdialog or xmessage on Linux when available). If no dialog program/display
is available, the recovery log still records the manual-installer fallback.
No capture permissions, GPU, FFmpeg execution, or migrated profile are needed by
the helper. Corrupt loaders, failures before the host arms it, missing/broken
helpers, power loss during package replacement, and builds released before this
companion still require manual recovery. This is a startup safeguard, not a
claim that every possible broken build can update itself.

Acceptance must include a real packaged failed migration, pre-readiness crash,
hang, normal Quit and secondary Open With; the signed repair must restart with
unchanged original captures and no duplicate UI. Automated component tests cover
the readiness/lifetime contract, retry/signature rejection, package-use exclusion,
newer-version loop prevention and retained backup on failed readiness. Physical
macOS, Windows, X11 and Wayland acceptance remains separate.

Known cutover limits: capture windows do not reopen automatically; reopen saved
captures and screenshot drafts from History. macOS/AppImage preserve Preferences
visibility, while Windows follows the installer launch. Physical signing/permission
continuity, login items, mixed DPI, accessibility and GNOME/KDE acceptance remain
unverified until run on those systems. Wayland region capture and floating previews
remain unavailable. Automated checks must not be reported as physical acceptance.

## Release queue

`.github/workflows/release.yml` runs on every push to `main`, including merges.
Its inexpensive scope check compares with the last published dated release and
skips app builds when there are no unpublished desktop changes. Native-app-only and website/API-only
pushes do not rebuild an already-published desktop; a later push can still collect
desktop changes left unpublished by a failed or superseded request.

Automatic builds use the triggering commit, not whatever `main` becomes while
the request waits. Manual dispatch without `channel_only` forces a rebuild of
the dispatch commit, or an explicitly supplied `target_sha` on main's history.
One request runs at a time and a newer pending request replaces the older pending
request. Channel-only dispatches share this lock and skip all app builds.

Staging waits for the frontend checks (about a minute), so a failure there
stops the release before any packaging. The Rust quality gates then run while
the macOS Apple Silicon, Windows x64, and Linux x64 packages build in parallel,
so they add no time to a release. The Rust checks run on GitHub-hosted runners
and restore the same cache as CI's Rust checks. A release is published only when
every job succeeds: **Publish release** needs the checks and the validated draft. Notes compare with the nearest published
dated release in the requested commit's history, ignoring unmarked archives,
drafts and orphan tags. Successful publication advances the public channel.

PR and release packaging reuse an exact-key cache of unsigned FFmpeg/ffprobe
binaries, source archives, and compliance files. The key includes the platform,
architecture, runner image, compiler, Rust host/toolchain, SDK or zlib version,
and hashes of the pinned source/build script, validator, configuration, and
notice. There are no fallback restore keys. Cache hits still verify the source
checksum, recreate compliance files, and validate both binaries. Cache saves
happen immediately after validation, before packaging can sign binaries; app
signing, notarization, updater signatures, and complete-release validation still
run for every release. GitHub evicts the least recently used caches once the
repository holds 10 GB, which removed this cache between releases, so release
packaging also keeps each platform's newest build as `<key>.tar.gz` on the draft
release **Build cache (not a release)** (`scripts/media-sidecar-store.mjs`). A
cache miss restores from it before compiling, and a restored build is validated
like a cached one. Store failures are warnings and fall back to compiling.
Delete the relevant GitHub Actions `ffmpeg-v1-*` cache and that draft's asset to
force a cold rebuild; the first build after a key change is also cold. Cold
cache misses download the pinned FFmpeg source. GitHub-hosted runners try a
previously published copy of that tarball from this repository first, then
ffmpeg.org, so a timeout talking to ffmpeg.org cannot stall a release.
Checksum verification is unchanged.

Dated releases are full GitHub releases (not pre-releases) with CalVer versions in
`YYYY.MM.DD.N` form, using the
`America/New_York` date of the main-branch commit and a same-day revision from 1
through 99. A release named `Captures 2026.07.19.1` uses tag
`v2026.07.19.1`. Tauri receives the SemVer-compatible internal version
`2026.7.1901`; source manifests remain at the development version. The updater
channel and fixed Git tag are both named `preview`; refreshing them always uses
the newest published Tauri version, ignoring native archives. The
**Update public downloads** job also retitles older dated releases to the same
`Captures <version>` form, strips their former experimental warning and clears
their pre-release flag.

The workflow stages a draft release at the exact commit under test. Each platform
builds and validates its pinned LGPL FFmpeg sidecars, then uploads its installer,
updater archive, and updater signature. The macOS job also verifies the sidecars
inside `Captures.app` and uploads the shared FFmpeg source archive, detached
signature, build configuration, LGPL license, and notice. When packaging an
older `target_sha`, that job overlays `scripts/github-release-assets.mjs` and
`scripts/release-assets.mjs` from the workflow commit so the stapled DMG upload
uses the current helper, including its local import. The final job requires
those files plus a DMG, NSIS installer, AppImage, Debian package, complete
`latest.json`, and `SHA256SUMS`, confirms the release is still staged, and then
publishes it as a full release. A failed build removes its draft and tag, leaving
published releases untouched. If draft creation itself is interrupted, the next run
removes only stale drafts with its generated tag before retrying.

Release notes are generated from only the commits in the release range that
match the same installed-app scope check. This keeps skipped website and API
changes from appearing later in the next real desktop update message.

During draft preparation, `scripts/ai-release-notes.mjs` asks Gemini for up to
five short, user-facing highlights. It follows Caper's casual, lightly witty editorial
prompt and Gemini request configuration, but writes the existing release body,
not a separate highlights feed. Packaging and the updater reuse that body.

Set the repository Actions secret `GEMINI_API_KEY`: a Gemini API key from
Google AI Studio whose project uses the paid tier. Optional repository
variables `CAPTURES_RELEASE_NOTES_MODEL` and
`CAPTURES_RELEASE_NOTES_THINKING_LEVEL` (`low`, `medium`, `high` or `xhigh`)
override `gemini-4-hydrogen` and `medium`.

The optional repository secret `OPENROUTER_API_KEY`
(`gh secret set OPENROUTER_API_KEY --repo joswayski/captures`) adds a fallback:
when Gemini fails, such as when its credits run out, or its answer fails
validation, the same prompt goes to `meta/muse-spark-1.3` on OpenRouter with the
same schema and checks. The variable `CAPTURES_RELEASE_NOTES_FALLBACK_MODEL`
overrides that model with any OpenRouter model whose provider supports
structured outputs. Generation uses structured JSON output,
32,768 output tokens shared with thinking, and a five-minute timeout.
It sends filtered source diffs and commit messages to Google, never capture
files or installed-app data. There is no AI request from the desktop app.

Evidence uses the exact previous-release-to-target first-parent range. Only
native-host (`apps/native`), `crates` and `shared` source paths supply evidence;
the wgpu workbench, website, tests, dependency bots and secret-looking paths are
excluded. The prompt requires behavior reachable in the AppKit, Win32 and GTK
hosts; shared-code changes alone do not establish that a feature is exposed.
Path filtering is not
a secret-content scanner. Requests are capped at 400,000 serialized characters
by shortening diffs and marking truncation, without dropping commit metadata.
Citations, output shape, plain text and length limits are checked locally;
these checks cannot prove every sentence is accurate. Review generated prose
when inspecting a release.

Missing credentials/baseline, provider errors, oversized metadata, or rejected
output retain the original commit-based notes. A successful empty result stays
quiet instead of restoring technical commit bullets. Generation happens once
per draft preparation, so all installers in that build share the same notes;
a fresh release-workflow run may generate different wording. Existing published
releases are not backfilled, and `channel_only` does not call Gemini.

If an in-app update download fails, the notice keeps the error on screen and
offers **download from captur.es**, which opens the website installer section.
Preferences → Updates always shows the same installer link, not only after a
failure. Users can also download the `.dmg`, Windows setup, or `.deb` from
captur.es or the README and install over the current copy. The AppImage does not
replace an existing launch by itself: copy it over `~/.local/bin/Captures.AppImage`
and `chmod +x`, matching `npm run install:latest`. Running it from Downloads
starts a second copy. That recovery path does not depend on the in-app updater
working; settings and capture history stay.
`scripts/updater-recovery.test.mjs` fails `npm run check` if updater artifacts,
endpoints, the signing public key, stable installer filenames, or that download
page URL are removed.

Creating the draft early is only a private staging step, and packaging starts
before the Rust checks finish. The **Publish release** job runs
only after those checks and **Validate staged release** succeed and `SHA256SUMS` is
present; that marker means every required macOS, Windows, and Linux artifact was
downloaded and validated together. Publishing does not resend the release's
commit: `GITHUB_TOKEN` cannot hold the workflows permission, and GitHub refuses
a release update naming a commit whose workflow files differ from `main`'s, so a
workflow change pushed during a release used to fail **Publish release** with a
403 and discard the build. The existence of a draft page by itself does
not mean the release is complete.

The fixed `preview` release is the permanent **Captures — Latest** download page,
marked as GitHub's Latest release, not a historical build. Its tag stays `preview`
because installed apps fetch `releases/download/preview/latest.json`. It holds the
macOS, Windows, Debian, and AppImage installers plus the `latest.json` updater
manifest for the newest
public Tauri CalVer version. That manifest includes a Tauri-only `changelog`
through that version, never notes from experimental native archives.
Installed apps ask `https://captur.es/api/updates/preview` for that
manifest about every five minutes (and again shortly after launch). The website
caches GitHub's `latest.json` for one minute and serves it unchanged; Tauri
still falls back to the GitHub download URL if captur.es is unreachable.
Platform download URLs in `latest.json` are rewritten from GitHub API asset
endpoints to public `releases/download/<CalVer-tag>/…` links so the archive
download itself does not consume the unauthenticated API rate limit (HTTP 403)
and does not keep draft `untagged-*` paths that 404 after the release is
published. Validation passes the workflow’s
CalVer tag into that rewrite because GitHub draft releases keep reporting
`tag_name` as `untagged-*` until they are published, even when the git tag
already exists. Installers on this channel use **stable filenames**
(`Captures-macOS-Apple-Silicon.dmg`, `Captures-Windows-x64-setup.exe`,
`Captures-Linux-x64.deb`, `Captures-Linux-x64.AppImage`) so the root README can
link directly without updating URLs on every merge. Dated immutable releases keep
versioned package names. The channel page links to the corresponding immutable
release, while the Releases page remains the dated build archive. Selection uses
the explicit public pin, not the highest version or publication time.

The first successful publication removes the obsolete `nightly` rolling
release and tag after the new `preview` page and its installer set are verified.
No compatibility redirect is retained.

For a historical backfill, dispatch the workflow from `main` with `target_sha`
set to a commit that is already on `main`. The workflow checks out and rebuilds
that commit, derives its New York date from the commit timestamp, and publishes
the next revision for that date. Dispatch historical commits from oldest to
newest so their revisions preserve merge order. Manual requests share the same
single pending slot and can be superseded by a subsequent dispatch
before starting; retry a superseded backfill when the queue is quiet. A dispatch
without `target_sha` snapshots the latest `main` when it starts.

When switching from the old preserve-every-push workflow, pending runs created
with the old workflow may still exist. Inspect Actions after merging; if needed,
cancel only obsolete pending release runs, keep the active run, and dispatch
the new workflow from `main`. Do not rerun old workflow runs to enable batching:
they retain their old workflow definition. For an urgent fix, a maintainer can
explicitly cancel an obsolete active build and request the latest `main`; check
that any staged draft was cleaned up before retrying. Normal pushes never
cancel the active release.

## Install the latest release

Use the CI-produced release when you need the published multi-platform
installer set, updater channel, or `SHA256SUMS` completion marker:

```sh
npm run install:latest
```

The command requires Node.js 24 and an authenticated GitHub CLI. It always
selects the greatest published release CalVer and requires both the current
system's installer and `SHA256SUMS`. The checksum file is the completion marker
uploaded only after the entire staged release passes validation.

After downloading and verifying the installer, the command quits Captures,
removes the installed app package, installs the macOS Apple Silicon DMG, Windows
x64 NSIS installer, Debian/Ubuntu x64 package, or Linux x64 AppImage, and launches
Captures again. User data, capture history, settings, and operating-system
permissions are not removed.

Changes to the installer command run this same download, uninstall, native
install, and post-install verification flow on ephemeral macOS Apple Silicon,
Windows x64, and Ubuntu x64 GitHub-hosted runners.

Useful options:

```sh
# Show whether the newest release is ready without changing the installed app
npm run install:latest -- --dry-run

# Fail instead of waiting when the newest release is incomplete
npm run install:latest -- --no-wait

# Install without launching Captures afterward
npm run install:latest -- --no-launch
```

## Stable-release gates

Public releases ship while the gates below remain open. Creating an installer and
signing a Tauri updater archive do not by themselves close them:

| Platform or concern | Required before production publishing                                                                                                               | Current implementation                                                                                                |
| ------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| macOS               | Developer ID Application signature, Apple notarization, stapled ticket, and clean-machine Gatekeeper validation                                     | CI signs and notarizes the app, notarizes and staples the DMG, and verifies both; clean-Mac validation remains manual |
| Windows             | Publicly trusted Authenticode signatures and RFC 3161 timestamps on both `captures.exe` and the NSIS installer                                      | Updater archives are signed, but Authenticode is not configured                                                       |
| Linux               | `SHA256SUMS` plus GitHub build-provenance attestations for the `.deb` and AppImage                                                                  | Checksums and updater signatures exist; attestations are not configured                                               |
| All platforms       | Every downloadable artifact must come from the tested commit, pass clean-machine installation, and remain unpublished if any platform is incomplete | The workflow validates the complete staged release before publishing; Windows and provenance gates remain             |

These signatures have separate trust boundaries:

- Apple and Authenticode signatures establish the operating-system publisher.
- `TAURI_SIGNING_PRIVATE_KEY` lets installed copies authenticate automatic updates; it does not identify the Windows publisher or replace Apple notarization.
- GitHub artifact attestations establish which repository, commit, and workflow produced a downloaded artifact.
- `SHA256SUMS` detects file changes after publication.

## GitHub release environment

The `release` environment protects signing and notarization credentials; it is
not a release channel. Public and staging builds can share this environment
while using different release metadata and updater endpoints.

Create a GitHub environment named `release`, restrict its deployment branches to `main`, and add these environment secrets:

| Secret                               | Purpose                             |
| ------------------------------------ | ----------------------------------- |
| `TAURI_SIGNING_PRIVATE_KEY`          | Dedicated Tauri updater private key |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password for that updater key       |

The Apple Developer ID certificate and App Store Connect key are not GitHub
secrets. Like caper and dbm, the macOS package job assumes the
`production-captures-release-signer` AWS role (trusted for this repository's
`release` environment) and reads the `apple` section of the
`production/signing/release` secret in AWS Secrets Manager:
`developer_id_p12_base64`, `developer_id_p12_password`,
`app_store_connect_key_id`, `app_store_connect_issuer_id` and
`app_store_connect_private_key`. The keychain password is generated per run.
joswayski/infrastructure's `docs/release-signing.md` covers storing and
rotating them.

Commit only the updater public key. Keep the updater private key, its password, and the App Store Connect private key in encrypted offline backups. Losing the updater private key means existing installations cannot verify a replacement key or receive another in-app update.

The macOS build intentionally fails when any Apple credential is missing, the
imported identity is not a Developer ID Application certificate, or the app or
DMG fails signing, notarization, stapling, or Gatekeeper validation. Do not
merge a release-workflow change unless every required credential is configured
on the protected environment.

## macOS signing and notarization

Direct distribution outside the Mac App Store requires a Developer ID Application signature and Apple notarization. A Developer ID Installer certificate is not needed for the DMG; it is used for signed `.pkg` installers. The same Developer ID Application identity can sign Captures and DBM, although each repository must independently protect its release environment and validate its output.

1. Create a certificate signing request in Keychain Access.
2. Create a **Developer ID Application** certificate in the Apple Developer portal, install it in the login keychain, and export the identity and private key as a password-protected `.p12`.
3. Create an App Store Connect **Team API key** with Developer access. Save its issuer ID, key ID, and downloaded `.p8`; Apple permits the private key to be downloaded only once.
4. Back up the `.p12`, `.p8`, Tauri updater private key, passwords, and recovery information in encrypted offline storage.
5. Store the Apple values in AWS Secrets Manager as described in joswayski/infrastructure `docs/release-signing.md`.
6. Confirm the workflow signs, notarizes, and staples the app and DMG, then validate them on a clean supported Mac without using a Gatekeeper bypass.

Before publishing, verify the signature, Gatekeeper assessment, and stapled ticket:

```sh
codesign --verify --deep --strict --verbose=2 Captures.app
spctl --assess --type execute --verbose=2 Captures.app
xcrun stapler validate Captures.app
codesign --verify --strict --verbose=2 Captures.dmg
spctl --assess --type open --context context:primary-signature --verbose=2 Captures.dmg
xcrun stapler validate Captures.dmg
```

## Windows Authenticode signing

Use a publicly trusted code-signing service before production publishing. The preferred CI route is **Microsoft Artifact Signing Public Trust** because its private signing keys remain in Microsoft's managed service instead of being exported into GitHub.

The same Artifact Signing account, validated identity, and Public Trust certificate profile can serve Captures and DBM.

### Account setup

1. Create an Azure subscription and Microsoft Entra tenant, then confirm the legal name and address on the Azure billing profile are correct.
2. Register the `Microsoft.CodeSigning` resource provider.
3. Create an Artifact Signing account.
4. Complete **Individual Public Trust** identity validation. Microsoft notes that validation can take from 1 to 20 business days, so start it before a planned production release.
5. Create a **Public Trust** certificate profile. Do not use a Public Trust Test or Private Trust profile for public downloads.
6. Create an Entra application or workload identity for GitHub Actions and grant it the **Artifact Signing Certificate Profile Signer** role scoped to the certificate profile.
7. Add a GitHub OIDC federated credential restricted to:

   ```text
   repo:joswayski/captures:environment:release
   ```

   OIDC avoids storing a long-lived Azure client secret in GitHub.

### Environment variables

Add these non-secret values as variables on the existing `release` environment:

| Variable                          | Value                                            |
| --------------------------------- | ------------------------------------------------ |
| `AZURE_CLIENT_ID`                 | Entra application or workload identity client ID |
| `AZURE_TENANT_ID`                 | Entra tenant ID                                  |
| `AZURE_SUBSCRIPTION_ID`           | Azure subscription containing Artifact Signing   |
| `AZURE_ARTIFACT_SIGNING_ENDPOINT` | Regional Artifact Signing endpoint               |
| `AZURE_ARTIFACT_SIGNING_ACCOUNT`  | Artifact Signing account name                    |
| `AZURE_ARTIFACT_SIGNING_PROFILE`  | Public Trust certificate profile name            |

The Windows package job must request `id-token: write`, authenticate to Azure with OIDC, and integrate Artifact Signing with Tauri so that both `captures.exe` and the final NSIS installer are signed. Sign with SHA-256 and use Microsoft's RFC 3161 timestamp service. Timestamping is required because Artifact Signing certificates are intentionally short-lived.

Validate both files before upload:

```powershell
Get-AuthenticodeSignature .\captures.exe |
  Format-List Status, StatusMessage, SignerCertificate, TimeStamperCertificate

Get-AuthenticodeSignature .\Captures_*_x64-setup.exe |
  Format-List Status, StatusMessage, SignerCertificate, TimeStamperCertificate
```

Both results must report `Valid`, show the expected publisher, and include a timestamp. Test the installer on a clean Windows 11 system and confirm the UAC dialog displays that verified publisher.

If Microsoft Artifact Signing is unavailable, use a publicly trusted OV/EV code-signing certificate from a certificate authority. Follow that provider's current hardware-token or cloud-HSM instructions; do not assume an exportable `.pfx` is permitted.

## Linux publication integrity

Linux has no single platform-wide publisher certificate comparable to Apple Developer ID or Windows Authenticode. For direct GitHub Release downloads, require verifiable integrity and provenance. GitHub attestations apply to every platform, so generate them for the macOS and Windows artifacts as well:

1. Continue building every release artifact only in the release workflow for the tested commit.
2. Generate `SHA256SUMS` over the final downloadable artifacts.
3. Generate a GitHub artifact attestation for every DMG, NSIS installer, `.deb`, AppImage, updater archive, and checksum manifest.
4. Confirm each artifact has a retrievable attestation before treating an automated release as production-ready.
5. Verify the release from a clean Ubuntu system:

   ```sh
   sha256sum --check SHA256SUMS
   gh attestation verify ./Captures_VERSION_amd64.deb --repo joswayski/captures
   gh attestation verify ./Captures_VERSION_amd64.AppImage --repo joswayski/captures
   sudo apt install ./Captures_VERSION_amd64.deb
   chmod +x ./Captures_VERSION_amd64.AppImage
   ./Captures_VERSION_amd64.AppImage
   ```

The attestation job must grant `contents: read`, `id-token: write`, and `attestations: write` and use GitHub's official `actions/attest` action.

An embedded GPG signature may also be added to the AppImage, but AppImage does not automatically verify it. Do not use an embedded AppImage signature as a replacement for checksums and build provenance.

If Captures later operates an APT repository, that repository must publish signed `InRelease` metadata or `Release` plus `Release.gpg`. Distribute the repository public key through an authenticated channel and configure users with a repository-specific keyring and `signed-by=`. Signing a standalone `.deb` is not a substitute for signing APT repository metadata.

## Bootstrap and acceptance

The first updater-enabled release must be downloaded and installed manually.
Subsequent validated releases appear in the rolling updater channel
automatically. Before relying on automatic updates, test this sequence on clean
machines:

1. Download one completed release and install it on macOS, Windows, and Ubuntu.
2. Merge another PR on the same New York calendar day and confirm the revision increments.
3. Confirm the completed release contains all platform assets and all three `latest.json` entries and is public.
4. When a subsequent validated release appears, choose **Later**, then install from the tray and verify the displayed version after restart.
5. On macOS, verify notarization, stapling, and Screen Recording permission survive the update.
6. On Windows, verify Authenticode on both the application executable and NSIS installer, then confirm NSIS updates in place. On Linux, verify `SHA256SUMS` and GitHub attestations before confirming AppImage updates in place and `.deb` directs the user to the release download.
7. Tamper with an updater archive in a test release and confirm signature verification rejects it.
8. Force one platform build to fail and confirm the failed draft/tag is deleted while published releases remain unchanged.
9. On macOS, rapidly resize a recording region and confirm window highlights use rounded corners. Open New Capture, use the direct region and display shortcuts to capture the visible controls, and verify the window shortcut selects real app windows instead of the full-screen macOS Screenshot surface. Drag the selector controls from any non-interactive panel surface, press Escape repeatedly to cancel, and verify the countdown and HUD can be captured by another app while Captures excludes them from its own output.
10. Record a visually static display for at least 10 seconds, then repeat while continuously moving content on a secondary display. Confirm the single full-display countdown fades from 3 to 2 to 1 without flashing 3 again, Escape cancels it, real motion continues past the initial frame, and both finalized durations are within 250 ms or 5% of wall-clock time, whichever is larger. On macOS 15+, enable **Show clicks** and verify clicks receive the system highlight.
11. Verify the recording HUD uses one compact control row with no border or shadow, shows unclipped tooltips and the faded **These controls won’t show in the recording** note below it, labels its active state **Recording**, responds on hover, drags from non-interactive background space, hides completely, and returns when Captures is opened from the macOS Dock. Confirm Restart requires explicit native confirmation.
12. Open a 1140×692 source in the editor and verify Fit and 100% previews, the centered video play/pause control, the 12-frame filmstrip, playhead, trim handles, crop overlay, resolution label, conditional audio controls, and millisecond labels stay synchronized.
13. Record a Retina region, window, and display with **Original** resolution and verify their masters use physical rather than logical pixels. Confirm moving content continues to update, cursor movement remains visible, **Show clicks** forces the cursor on and renders click feedback, and an untouched **Preserve quality** save updates the original without re-encoding. Verify audio-only changes preserve the video stream, visual edits use the high-quality H.264 path, a failed edit leaves the original intact, filename and folder remain editable when updating the original, optional **Keep original** saves a copy, collisions are rejected, strict maximum-size output does not exceed its selected KB, MB, or GB ceiling, and a successful save opens Finder with the measured final size and a **Show in Folder** action shown.
14. While recording, start a region screenshot and confirm the HUD fades away without flickering before the selector appears.
15. Delete a quick-access preview and confirm its native window becomes click-through immediately while the particle animation finishes.
16. On clean Windows and Linux machines, record a display, region, and fixed
    window area; pause and resume; save an MP4; export a GIF; and apply a crop
    and resize in the editor. Record desktop audio and each available microphone
    independently, verify the live microphone meter and mute control, and confirm
    the finalized MP4 exposes editable system and microphone tracks. On Windows
    and X11, verify cursor visibility and animated click highlights. On Wayland,
    confirm pointer options are disabled, choose the same display in the portal,
    and hide the recording HUD before capturing its area.
17. Confirm the packaged FFmpeg/ffprobe sidecars run on clean macOS, Windows,
    and Linux installations and the release includes every source and
    compliance asset listed in `docs/media-sidecars.md`.

## References

- [Apple: Developer ID certificates](https://developer.apple.com/help/account/certificates/create-developer-id-certificates)
- [Apple: notarizing macOS software](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)
- [Tauri: macOS code signing](https://v2.tauri.app/distribute/sign/macos/)
- [Microsoft: set up Artifact Signing](https://learn.microsoft.com/azure/artifact-signing/quickstart)
- [Microsoft: Artifact Signing integrations](https://learn.microsoft.com/azure/artifact-signing/how-to-signing-integrations)
- [Azure: Artifact Signing GitHub Action](https://github.com/Azure/artifact-signing-action)
- [Tauri: Windows code signing](https://v2.tauri.app/distribute/sign/windows/)
- [GitHub: artifact attestations](https://docs.github.com/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations)
- [Tauri: Linux code signing](https://v2.tauri.app/distribute/sign/linux/)
- [Debian: package and repository signing](https://www.debian.org/doc/manuals/securing-debian-manual/deb-pack-sign.en.html)
