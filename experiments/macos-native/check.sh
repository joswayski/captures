#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
experiment="$root/experiments/macos-native"
out="$experiment/build"
mkdir -p "$out"
swiftc="${SWIFTC:-swiftc}"
python3 -m unittest discover -s "$experiment" -p 'test_*.py'
"$swiftc" -frontend -parse "$experiment"/Sources/CapturesNative/*.swift
"$swiftc" -swift-version 5 -parse-as-library \
  "$experiment/Sources/CapturesNative/Models.swift" "$experiment/Tests/ModelsTests.swift" \
  -o "$out/models-tests"
"$out/models-tests"
rust_args=(--manifest-path "$experiment/bridge/Cargo.toml")
if [[ "$(uname -s)" == Darwin ]]; then
  "$swiftc" -swift-version 5 -parse-as-library -typecheck -module-name CapturesNative \
    -target "$(uname -m)-apple-macosx13.0" "$experiment"/Sources/CapturesNative/*.swift
  case "$(uname -m)" in
    arm64) rust_target=aarch64-apple-darwin ;;
    x86_64) rust_target=x86_64-apple-darwin ;;
  esac
  rust_args+=(--target "$rust_target" --target-dir "$out/rust")
fi
cargo fmt --manifest-path "$experiment/bridge/Cargo.toml" -- --check
CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo test --release --locked "${rust_args[@]}"
CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo clippy --release --locked "${rust_args[@]}" --all-targets -- -D warnings
if [[ "$(uname -s)" == Darwin ]]; then
  bash "$experiment/build.sh" --test
else
  printf 'Swift parsing and Foundation tests only: AppKit/SwiftUI and editor tests require macOS.\n'
fi
