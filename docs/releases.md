# Captures releases

Every push to `main` runs the inexpensive scope check in
`.github/workflows/release.yml`. Website, hosted API, documentation, standalone
desktop UI test-file, and release-tooling-only changes stop there: they do not
choose a new version, build installers, publish a Preview, or notify installed
desktop apps. Changes to desktop source, shared UI source, Rust crates and
manifests, or the desktop's reachable `package-lock.json` dependency graph
continue through the release. Manual builds always run, including when a
release-tooling change intentionally needs a new installer set.

Qualifying Preview workflows wait in commit order without cancelling older
pushes, run the frontend and Rust quality gates, and then build macOS Apple
Silicon, Windows x64, and Linux x64 packages. A Preview is published only when
every job succeeds. Later runs share a GitHub concurrency group with `queue: max`
so they stay queued instead of cancelling an older or intermediate build
(`cancel-in-progress: false` alone still drops pending runs when a third push
arrives). The wait job retries transient GitHub API errors instead of failing the
Preview.

Previews are GitHub pre-releases with CalVer versions in `YYYY.MM.DD.N` form,
using the
`America/New_York` date of the main-branch commit and a same-day revision from 1
through 99. A Preview named `Captures Preview 2026.07.19.1` uses tag
`v2026.07.19.1`. Tauri receives the SemVer-compatible internal version
`2026.7.1901`; source manifests remain at the development version. The updater
channel and fixed Git tag are both named `preview` and update after every
installed-app change.

The workflow stages a draft Preview at the exact tested commit. Each platform
builds and validates its pinned LGPL FFmpeg sidecars, then uploads its installer,
updater archive, and updater signature. The macOS job also verifies the sidecars
inside `Captures.app` and uploads the shared FFmpeg source archive, detached
signature, build configuration, LGPL license, and notice. When packaging an
older `target_sha`, that job overlays `scripts/github-release-assets.mjs` and
`scripts/release-assets.mjs` from the workflow commit so the stapled DMG upload
uses the current helper, including its local import. The final job requires
those files plus a DMG, NSIS installer, AppImage, Debian package, complete
`latest.json`, and `SHA256SUMS`, confirms the release is still staged, and then
publishes it as a pre-release. A failed build removes its draft and tag, leaving
published Previews untouched. If draft creation itself is interrupted, the next run
removes only stale drafts with its generated tag before retrying.

Release notes are generated from only the commits in the release range that
match the same installed-app scope check. This keeps skipped website and API
changes from appearing later in the next real desktop update message.

If an in-app update download fails, the notice keeps the error on screen and
offers **download from captur.es**, which opens the website installer section.

Creating the draft early is only a private staging step. The **Publish Preview**
job runs only after **Validate staged release** succeeds and `SHA256SUMS` is
present; that marker means every required macOS, Windows, and Linux artifact was
downloaded and validated together. The existence of a draft page by itself does
not mean the release is complete.

The fixed `preview` pre-release is the permanent **Captures Preview — Latest**
download page, not a historical build. It holds the macOS, Windows, Debian, and
AppImage installers plus the `latest.json` updater manifest for the greatest
published CalVer version. That manifest includes a `changelog` of each dated
Preview’s notes so an installed copy can list every change between its version
and the latest Preview, not only the newest release. Installed Preview builds ask `https://captur.es/api/updates/preview` for that
manifest about every five minutes (and again shortly after launch). The website
caches GitHub's `latest.json` for one minute and serves it unchanged; Tauri
still falls back to the GitHub download URL if captur.es is unreachable.
Platform download URLs in `latest.json` are rewritten from GitHub API asset
endpoints to public `releases/download/<CalVer-tag>/…` links so the archive
download itself does not consume the unauthenticated API rate limit (HTTP 403)
and does not keep draft `untagged-*` paths that 404 after the Preview is
published. Validation passes the workflow’s
CalVer tag into that rewrite because GitHub draft releases keep reporting
`tag_name` as `untagged-*` until they are published, even when the git tag
already exists. Installers on this channel use **stable filenames**
(`Captures-macOS-Apple-Silicon.dmg`, `Captures-Windows-x64-setup.exe`,
`Captures-Linux-x64.deb`, `Captures-Linux-x64.AppImage`) so the root README can
link directly without updating URLs on every merge. Dated immutable Previews keep
versioned package names. The channel page links to the corresponding immutable
Preview, while the Releases page remains the dated build archive. Selection uses
the version, not publication time, so publishing an older backfill cannot
downgrade the download page. Future stable releases can use normal GitHub release
metadata and their own updater endpoint without replacing the Preview archive.

The first successful Preview publication removes the obsolete `nightly` rolling
release and tag after the new `preview` page and its installer set are verified.
No compatibility redirect is retained during this pre-release phase.

For a historical backfill, dispatch the workflow from `main` with `target_sha`
set to a commit that is already on `main`. The workflow checks out and rebuilds
that commit, derives its New York date from the commit timestamp, and publishes
the next revision for that date. Dispatch historical commits from oldest to
newest so their revisions preserve merge order.

## Install the latest Preview

Use a local signed macOS build when iterating on first-run setup. See
[DEVELOPMENT.md](../DEVELOPMENT.md#packaging) for `npm run build:signed`. That
command signs with the Developer ID identity, notarizes and staples a DMG, and
installs it with Gatekeeper quarantine. It does not publish a Preview or build
the Windows and Linux installers.

Use the CI-produced Preview when you need the published multi-platform
installer set, updater channel, or `SHA256SUMS` completion marker:

```sh
npm run install:preview
```

The command requires Node.js 24 and an authenticated GitHub CLI. It always
selects the greatest published Preview CalVer and requires both the current
system's installer and `SHA256SUMS`. The checksum file is the completion marker
uploaded only after the entire staged Preview passes validation.

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
# Show whether the newest Preview is ready without changing the installed app
npm run install:preview -- --dry-run

# Fail instead of waiting when the newest Preview is incomplete
npm run install:preview -- --no-wait

# Install without launching Captures afterward
npm run install:preview -- --no-launch
```

## Stable-release gates

Previews are intentionally experimental. Creating an installer and signing a
Tauri updater archive do not by themselves make a stable release. Do not publish
the stable channel until every gate below is enforced and passes:

| Platform or concern | Required before production publishing | Current implementation |
| --- | --- | --- |
| macOS | Developer ID Application signature, Apple notarization, stapled ticket, and clean-machine Gatekeeper validation | CI signs and notarizes the app, notarizes and staples the DMG, and verifies both; clean-Mac validation remains manual |
| Windows | Publicly trusted Authenticode signatures and RFC 3161 timestamps on both `captures.exe` and the NSIS installer | Updater archives are signed, but Authenticode is not configured |
| Linux | `SHA256SUMS` plus GitHub build-provenance attestations for the `.deb` and AppImage | Checksums and updater signatures exist; attestations are not configured |
| All platforms | Every downloadable artifact must come from the tested commit, pass clean-machine installation, and remain unpublished if any platform is incomplete | The workflow validates the complete staged release before publishing; Windows and provenance gates remain |

These signatures have separate trust boundaries:

- Apple and Authenticode signatures establish the operating-system publisher.
- `TAURI_SIGNING_PRIVATE_KEY` lets installed copies authenticate automatic updates; it does not identify the Windows publisher or replace Apple notarization.
- GitHub artifact attestations establish which repository, commit, and workflow produced a downloaded artifact.
- `SHA256SUMS` detects file changes after publication.

## GitHub release environment

The `release` environment protects signing and notarization credentials; it is
not a release channel. Preview and stable builds can share this environment
while using different release metadata and updater endpoints.

Create a GitHub environment named `release`, restrict its deployment branches to `main`, and add these environment secrets:

| Secret | Purpose |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | Dedicated Tauri updater private key |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password for that updater key |
| `APPLE_CERTIFICATE` | Base64-encoded Developer ID Application `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | Export password for the `.p12` |
| `KEYCHAIN_PASSWORD` | Temporary CI keychain password |
| `APPLE_API_ISSUER` | App Store Connect API issuer ID |
| `APPLE_API_KEY` | App Store Connect API key ID |
| `APPLE_API_PRIVATE_KEY` | Contents of the App Store Connect `.p8` private key |

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
5. Add the Apple values to the `release` environment using the exact secret names above.
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

| Variable | Value |
| --- | --- |
| `AZURE_CLIENT_ID` | Entra application or workload identity client ID |
| `AZURE_TENANT_ID` | Entra tenant ID |
| `AZURE_SUBSCRIPTION_ID` | Azure subscription containing Artifact Signing |
| `AZURE_ARTIFACT_SIGNING_ENDPOINT` | Regional Artifact Signing endpoint |
| `AZURE_ARTIFACT_SIGNING_ACCOUNT` | Artifact Signing account name |
| `AZURE_ARTIFACT_SIGNING_PROFILE` | Public Trust certificate profile name |

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

The first updater-enabled Preview must be downloaded and installed manually.
Subsequent validated Previews appear in the rolling updater channel
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
