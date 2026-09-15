#!/usr/bin/env bash
set -euo pipefail

platform="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
experiment="$(cd "$platform/.." && pwd)"
root="$(cd "$experiment/../.." && pwd)"
target="${1:-}"
dist="${2:-$platform/dist/macos-$target}"

[[ "$(uname -s)" == Darwin ]] || { echo 'Run this helper on macOS with full Xcode.' >&2; exit 2; }
case "$target" in
  aarch64-apple-darwin) expected=arm64 ;;
  x86_64-apple-darwin) expected=x86_64 ;;
  *) echo 'usage: build-package.sh aarch64-apple-darwin|x86_64-apple-darwin [DIST]' >&2; exit 2 ;;
esac
[[ "$(uname -m)" == "$expected" ]] || {
  echo "Build $target natively on a $expected runner; this helper does not create cross-architecture bundles." >&2
  exit 2
}

for tool in cargo codesign pkg-config xcodebuild xcrun; do command -v "$tool" >/dev/null; done
xcodebuild -version
xcrun --sdk macosx --find metal >/dev/null
xcrun --sdk macosx --find metallib >/dev/null
pkg-config --atleast-version=1.16 cairo
bash "$platform/check-media-tools.sh"

export MACOSX_DEPLOYMENT_TARGET=13.0
cargo test --manifest-path "$experiment/Cargo.toml" --locked --release --target "$target"
cargo build --manifest-path "$experiment/Cargo.toml" --locked --release --target "$target"

app="$dist/Captures GPUI Experiment.app"
rm -rf "$dist"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$platform/macos/Info.plist" "$app/Contents/Info.plist"
cp "$root/apps/desktop/src-tauri/icons/icon.icns" "$app/Contents/Resources/icon.icns"
cp "$experiment/target/$target/release/captures-gpui" "$app/Contents/MacOS/captures-gpui"
if [[ -n "${CAPTURES_GPUI_MEDIA_DIR:-}" ]]; then
  mkdir -p "$app/Contents/Resources/bin"
  for tool in ffmpeg ffprobe ffplay; do
    [[ -x "$CAPTURES_GPUI_MEDIA_DIR/$tool" ]] || {
      echo "Missing executable $CAPTURES_GPUI_MEDIA_DIR/$tool" >&2
      exit 2
    }
    cp "$CAPTURES_GPUI_MEDIA_DIR/$tool" "$app/Contents/Resources/bin/$tool"
    codesign --force --options runtime --sign - "$app/Contents/Resources/bin/$tool"
  done
fi
codesign --force --options runtime --entitlements "$platform/macos/entitlements.plist" --sign - "$app"
codesign --verify --deep --strict "$app"
plutil -lint "$app/Contents/Info.plist"
[[ "$(defaults read "$app/Contents/Info" CFBundleIdentifier)" == io.github.joswayski.captures.gpui-experiment ]]
"$app/Contents/MacOS/captures-gpui" --help >/dev/null

zip="$dist/Captures-GPUI-Experiment-$target.zip"
ditto -c -k --sequesterRsrc --keepParent "$app" "$zip"
printf 'Built ad-hoc-signed private-test bundle: %s\n' "$zip"
