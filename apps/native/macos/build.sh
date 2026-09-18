#!/usr/bin/env bash
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ "$(uname -s)" != Darwin ]]; then
  echo 'The AppKit workbench requires macOS 13+ and Xcode command-line tools. No Linux/WebView fallback.' >&2
  exit 1
fi
node "$HERE/../prepare.mjs"
TARGET_DIR="${CARGO_TARGET_DIR:-$HERE/../../../target}"
cargo build --manifest-path "$HERE/../../../Cargo.toml" --target-dir "$TARGET_DIR" --release -p captures-settings-ffi
export CAPTURES_NATIVE_LIB_DIR="$(cd "$TARGET_DIR/release" && pwd)"
swift test --package-path "$HERE" -c release
swift build --package-path "$HERE" -c release
echo "Run: $HERE/.build/release/CapturesNative --scene preferences"
