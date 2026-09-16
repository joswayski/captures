#!/usr/bin/env bash
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ "$(uname -s)" != Darwin ]]; then
  echo 'The AppKit workbench requires macOS 13+ and Xcode command-line tools. No Linux/WebView fallback.' >&2
  exit 1
fi
node "$HERE/../prepare.mjs"
swift test --package-path "$HERE" -c release
swift build --package-path "$HERE" -c release
echo "Run: $HERE/.build/release/CapturesNative --scene preferences"
