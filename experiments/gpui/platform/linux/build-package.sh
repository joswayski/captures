#!/usr/bin/env bash
set -euo pipefail

platform="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
experiment="$(cd "$platform/.." && pwd)"
target=x86_64-unknown-linux-gnu
dist="${1:-$platform/dist/linux-x86_64}"

[[ "$(uname -s)" == Linux ]] || { echo 'Run this helper on Linux.' >&2; exit 2; }
bash "$platform/check-media-tools.sh"
cargo test --manifest-path "$experiment/Cargo.toml" --locked --release --target "$target"
cargo build --manifest-path "$experiment/Cargo.toml" --locked --release --target "$target"

rm -rf "$dist"
mkdir -p "$dist"
cp "$experiment/target/$target/release/captures-gpui" "$dist/captures-gpui"
cp "$platform/README.md" "$dist/PRIVATE-TEST-NOTES.md"
if [[ -n "${CAPTURES_GPUI_MEDIA_DIR:-}" ]]; then
  for tool in ffmpeg ffprobe ffplay; do
    [[ -x "$CAPTURES_GPUI_MEDIA_DIR/$tool" ]] || {
      echo "Missing executable $CAPTURES_GPUI_MEDIA_DIR/$tool" >&2
      exit 2
    }
    cp "$CAPTURES_GPUI_MEDIA_DIR/$tool" "$dist/$tool"
  done
fi
tarball="$dist.tar.gz"
rm -f "$tarball"
tar -C "$(dirname "$dist")" -czf "$tarball" "$(basename "$dist")"
printf 'Built private-test package: %s\n' "$tarball"
