#!/usr/bin/env bash
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
LAB="$ROOT/experiments/macos-parity"
if [[ "$(uname -s)" != Darwin ]]; then
  echo 'Build graphical candidates on macOS (Xcode command-line tools, Rust, Node 24+ required).' >&2
  exit 1
fi
node "$LAB/generate.mjs"
"$ROOT/node_modules/.bin/vite" build --config "$LAB/vite.config.mjs"
swiftc -O -swift-version 5 "$LAB/native/Math.swift" "$LAB/test-cover.swift" -o "$LAB/.build/test-cover"
"$LAB/.build/test-cover"
swiftc -O -swift-version 5 -target "$(uname -m)-apple-macos13.0" "$LAB"/native/*.swift -o "$LAB/.build/captures-parity-native"
swiftc -O -parse-as-library -swift-version 5 -target "$(uname -m)-apple-macos13.0" "$LAB/profile/Observer.swift" -o "$LAB/.build/parity-observer"
clang -O2 -Wall -Wextra -Werror "$LAB/profile/resources.c" -o "$LAB/.build/parity-resources"
"$LAB/.build/parity-resources" --self-test
"$LAB/.build/captures-parity-native" --poses "$LAB/.build/poses-input.json" "$LAB/.build/poses-actual.json"
node "$LAB/verify-poses.mjs" "$LAB/.build/poses-actual.json"
cargo build --manifest-path "$LAB/tauri/Cargo.toml" --release --features custom-protocol --locked --target-dir "$LAB/tauri/target"
python3 -m venv "$LAB/.build/python"
"$LAB/.build/python/bin/pip" install --disable-pip-version-check -r "$LAB/requirements.txt"
"$LAB/.build/python/bin/python" "$LAB/benchmark.py" manifest
echo 'Built optimized candidates. Run: bash experiments/macos-parity/run.sh'
