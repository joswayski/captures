#!/usr/bin/env bash
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
LAB="$ROOT/experiments/macos-parity"
if [[ ! -x "$LAB/.build/captures-parity-native" || ! -x "$LAB/.build/python/bin/python" ]]; then
  bash "$LAB/build.sh"
fi
exec "$LAB/.build/python/bin/python" "$LAB/benchmark.py" "$@"
