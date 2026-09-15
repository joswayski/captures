#!/usr/bin/env bash
# Build, sign, notarize, and package the arm64 native Preview without installing it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="${1:-}"
output_dir="${2:-native-dist}"
app_name="Captures Native Preview"
artifact_name="Captures-macOS-Apple-Silicon.dmg"

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  printf 'Usage: %s <version, e.g. 2026.9.1401> [output-dir]\n' "$0" >&2
  exit 2
fi
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || {
  printf 'Native Preview packaging requires Apple Silicon macOS.\n' >&2; exit 1;
}
for tool in base64 codesign ditto hdiutil security spctl xcrun; do
  command -v "$tool" >/dev/null || { printf 'Required tool is missing: %s\n' "$tool" >&2; exit 1; }
done
for variable in APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD KEYCHAIN_PASSWORD APPLE_API_ISSUER APPLE_API_KEY APPLE_API_PRIVATE_KEY; do
  [[ -n "${!variable:-}" ]] || { printf 'Required environment variable is missing: %s\n' "$variable" >&2; exit 1; }
done

work="$(mktemp -d "${TMPDIR:-/tmp}/captures-native-package.XXXXXX")"
keychain="$work/signing.keychain-db"
certificate="$work/certificate.p12"
api_key="$work/AuthKey_${APPLE_API_KEY}.p8"
old_default="$(security default-keychain -d user | tr -d ' "')"
cleanup() {
  security default-keychain -d user -s "$old_default" >/dev/null 2>&1 || true
  security delete-keychain "$keychain" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
umask 077
printf '%s' "$APPLE_CERTIFICATE" | base64 -D > "$certificate"
printf '%s' "$APPLE_API_PRIVATE_KEY" > "$api_key"

security create-keychain -p "$KEYCHAIN_PASSWORD" "$keychain"
security default-keychain -d user -s "$keychain"
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$keychain"
security set-keychain-settings -t 3600 -u "$keychain"
security import "$certificate" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PASSWORD" "$keychain"
identity="$(security find-identity -v -p codesigning "$keychain" | awk -F\" '/Developer ID Application/{print $2; exit}')"
[[ -n "$identity" ]] || { printf 'The certificate does not contain a Developer ID Application identity.\n' >&2; exit 1; }

# Credentials remain private inside the 0700 temporary directory; app payloads
# must be readable by the people mounting the DMG, not just the CI build user.
umask 022
media_root="$root/apps/desktop/src-tauri"
for required in \
  "$media_root/binaries/ffmpeg-aarch64-apple-darwin" \
  "$media_root/binaries/ffprobe-aarch64-apple-darwin" \
  "$media_root/ffmpeg/COPYING.LGPLv2.1" \
  "$media_root/ffmpeg/NOTICE.md" \
  "$media_root/ffmpeg/BUILD_CONFIG.txt" \
  "$root/apps/desktop/src-tauri/icons/icon.icns"; do
  [[ -f "$required" ]] || { printf 'Required prepared packaging input is missing: %s\n' "$required" >&2; exit 1; }
done
for pattern in '*.tar.xz' '*.tar.xz.asc' '*-BUILD_CONFIG.txt' '*-COPYING.LGPLv2.1' '*-NOTICE.md'; do
  compgen -G "$root/target/ffmpeg-dist/$pattern" >/dev/null || {
    printf 'Required FFmpeg source/compliance input is missing: target/ffmpeg-dist/%s\n' "$pattern" >&2
    exit 1
  }
done

export CAPTURES_NATIVE_APP_NAME="$app_name"
export CAPTURES_NATIVE_APP_VERSION="$version"
export CAPTURES_NATIVE_REQUIRE_MEDIA=1
export CAPTURES_NATIVE_SIGN_IDENTITY="$identity"
"$root/experiments/macos-native/build.sh"
app="$root/experiments/macos-native/build/$app_name.app"

# Keep the experiment's bundle identifier so Screen Recording and microphone
# grants remain associated with the native Preview, never the Tauri app.
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")" == es.captur.native-experiment ]]
cp "$root/apps/desktop/src-tauri/icons/icon.icns" "$app/Contents/Resources/AppIcon.icns"
/usr/libexec/PlistBuddy -c 'Add :CFBundleIconFile string AppIcon' "$app/Contents/Info.plist"
cp "$root/LICENSE" "$app/Contents/Resources/LICENSE"
mkdir -p "$app/Contents/Resources/notices/openh264"
cp "$root/apps/desktop/src-tauri/openh264/"* "$app/Contents/Resources/notices/openh264/"
codesign --force --options runtime --entitlements "$root/experiments/macos-native/entitlements.plist" --sign "$identity" "$app"
codesign --verify --deep --strict --verbose=2 "$app"

archive="$work/native-preview.zip"
ditto -c -k --keepParent "$app" "$archive"
xcrun notarytool submit "$archive" --key "$api_key" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER" --wait
xcrun stapler staple -v "$app"
xcrun stapler validate "$app"
spctl --assess --type execute --verbose=2 "$app"

stage="$work/dmg"
mkdir -p "$stage"
ditto "$app" "$stage/$app_name.app"
ln -s /Applications "$stage/Applications"
mkdir -p "$output_dir"
output_dir="$(cd "$output_dir" && pwd)"
dmg="$output_dir/$artifact_name"
rm -f "$dmg"
hdiutil create -quiet -fs HFS+ -volname "$app_name" -srcfolder "$stage" "$dmg"
codesign --force --sign "$identity" "$dmg"
xcrun notarytool submit "$dmg" --key "$api_key" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER" --wait
xcrun stapler staple -v "$dmg"
codesign --verify --strict --verbose=2 "$dmg"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
xcrun stapler validate "$dmg"
printf 'Created (not installed or launched): %s\n' "$dmg"
