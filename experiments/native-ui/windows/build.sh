#!/usr/bin/env bash
set -euo pipefail

[[ "${MSYSTEM:-}" == MINGW64 ]] || { echo 'Run from an MSYS2 MINGW64 shell.' >&2; exit 2; }
for tool in cargo rustc objdump python glib-compile-schemas; do
  command -v "$tool" >/dev/null || { echo "Missing tool: $tool" >&2; exit 2; }
done
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
experiment=$(cd "$here/.." && pwd)
out=${1:-"$here/dist/captures-windows-native"}
[[ ! -e "$out" ]] || { echo "Output already exists; choose a new directory: $out" >&2; exit 2; }
cargo build --manifest-path "$experiment/Cargo.toml" --locked --release \
  --target x86_64-pc-windows-gnu --no-default-features --features native \
  --bin captures-windows-native
python "$here/package.py" --prefix "$MINGW_PREFIX" \
  --binary "$experiment/target/x86_64-pc-windows-gnu/release/captures-windows-native.exe" \
  --output "$out"
