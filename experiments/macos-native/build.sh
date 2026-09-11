#!/usr/bin/env bash
# Local, opt-in native bundle. Never installs over or launches the shipping app.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
experiment="$root/experiments/macos-native"
if [[ "$(uname -s)" != Darwin ]]; then
  printf 'The native app must be linked on macOS with Xcode. Linux can test bridge logic and parse Swift, not build AppKit.\n' >&2
  exit 1
fi
for tool in cargo python3 xcrun codesign; do command -v "$tool" >/dev/null; done
sdk="$(xcrun --sdk macosx --show-sdk-path)"
arch="$(uname -m)"
case "$arch" in
  arm64) rust_target=aarch64-apple-darwin ;;
  x86_64) rust_target=x86_64-apple-darwin ;;
  *) printf 'Unsupported architecture: %s\n' "$arch" >&2; exit 1 ;;
esac
out="$experiment/build"
app="$out/Captures Native Experiment.app"
mkdir -p "$out"
# Ask rustc for the platform/link dependencies of the whole static library,
# including the existing ScreenCaptureKit/Swift media writer, not a guessed list.
CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo rustc --color never --release --locked \
  --manifest-path "$experiment/bridge/Cargo.toml" --target "$rust_target" \
  --target-dir "$out/rust" --lib -- --print native-static-libs 2>&1 | tee "$out/rust-link.log"
native_links=()
read -r -a native_links <<< "$(sed -n 's/.*native-static-libs: //p' "$out/rust-link.log" | tail -1)"
if ((${#native_links[@]} == 0)); then
  printf 'rustc did not report its native link dependencies; refusing to create an incomplete bundle.\n' >&2
  exit 1
fi
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$experiment/Info.plist" "$app/Contents/Info.plist"
python3 "$experiment/tokens.py" "$app/Contents/Resources/tokens.json"
xcrun swiftc -swift-version 5 -parse-as-library -O -whole-module-optimization \
  -module-name CapturesNative -target "$arch-apple-macosx13.0" -sdk "$sdk" \
  "$experiment"/Sources/CapturesNative/*.swift \
  "$out/rust/$rust_target/release/libcaptures_macos_bridge.a" \
  "${native_links[@]}" \
  -framework SwiftUI -framework AppKit -framework AVKit -framework Carbon \
  -framework ServiceManagement -framework UniformTypeIdentifiers -framework ImageIO \
  -o "$app/Contents/MacOS/captures-native"

# Prefer the repository's self-contained media sidecars when prepared. Otherwise
# the experiment discovers FFmpeg/ffprobe on PATH at runtime, not at build time.
for tool in ffmpeg ffprobe; do
  sidecar="$root/apps/desktop/src-tauri/binaries/$tool-$rust_target"
  if [[ -f "$sidecar" ]]; then
    test -f "$root/apps/desktop/src-tauri/ffmpeg/COPYING.LGPLv2.1"
    test -d "$root/target/ffmpeg-dist"
    mkdir -p "$app/Contents/Resources/notices/ffmpeg"
    cp "$root/apps/desktop/src-tauri/ffmpeg/"* "$app/Contents/Resources/notices/ffmpeg/"
    # Corresponding source/signature accompany redistributed sidecars too.
    cp "$root/target/ffmpeg-dist/"* "$app/Contents/Resources/notices/ffmpeg/"
    mkdir -p "$app/Contents/Resources/media"
    cp "$sidecar" "$app/Contents/Resources/media/$tool"
    chmod +x "$app/Contents/Resources/media/$tool"
    codesign --force --sign "${CAPTURES_NATIVE_SIGN_IDENTITY:--}" "$app/Contents/Resources/media/$tool"
  fi
done
codesign --force --options runtime --entitlements "$experiment/entitlements.plist" \
  --sign "${CAPTURES_NATIVE_SIGN_IDENTITY:--}" "$app"
codesign --verify --deep --strict "$app"
if [[ "${1:-}" == --test ]]; then
  sources=()
  for source in "$experiment"/Sources/CapturesNative/*.swift; do
    [[ "$(basename "$source")" == Main.swift ]] || sources+=("$source")
  done
  python3 "$experiment/tokens.py" "$out/tokens.json"
  test_platform="$(xcode-select -p)/Platforms/MacOSX.platform/Developer"
  # XCTest's Swift overlay lives beside the developer libraries, not inside
  # the Objective-C framework. Direct executables also need its runtime paths.
  xcrun swiftc -swift-version 5 -parse-as-library -module-name CapturesNativeTests \
    -target "$arch-apple-macosx13.0" -sdk "$sdk" \
    -I "$test_platform/usr/lib" -L "$test_platform/usr/lib" \
    -F "$test_platform/Library/Frameworks" \
    -Xlinker -rpath -Xlinker "$test_platform/usr/lib" \
    -Xlinker -rpath -Xlinker "$test_platform/Library/Frameworks" \
    -Xlinker -rpath -Xlinker "$test_platform/Library/PrivateFrameworks" \
    "${sources[@]}" "$experiment/Tests/EditorModelTests.swift" \
    "$out/rust/$rust_target/release/libcaptures_macos_bridge.a" "${native_links[@]}" \
    -framework SwiftUI -framework AppKit -framework AVKit -framework Carbon \
    -framework ServiceManagement -framework UniformTypeIdentifiers -framework ImageIO \
    -framework XCTest -o "$out/editor-tests"
  test_data="$(mktemp -d "$out/test-data.XXXXXX")"
  trap 'rm -rf "$test_data"' EXIT
  CAPTURES_NATIVE_DATA="$test_data" "$out/editor-tests"
  xcrun swiftc -swift-version 5 -parse-as-library -module-name CapturesNativeReferences \
    -target "$arch-apple-macosx13.0" -sdk "$sdk" \
    "${sources[@]}" "$experiment/Tests/RenderReferences.swift" \
    "$out/rust/$rust_target/release/libcaptures_macos_bridge.a" "${native_links[@]}" \
    -framework SwiftUI -framework AppKit -framework AVKit -framework Carbon \
    -framework ServiceManagement -framework UniformTypeIdentifiers -framework ImageIO \
    -o "$out/render-references"
fi
printf '\nBuilt (not installed or launched):\n%s\n' "$app"
printf 'Open this app to test; allow its own Screen Recording and microphone permissions.\n'
