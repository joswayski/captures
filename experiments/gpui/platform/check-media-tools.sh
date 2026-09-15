#!/usr/bin/env bash
set -euo pipefail

for tool in ffmpeg ffprobe ffplay; do
  command -v "$tool" >/dev/null || {
    printf 'Missing runtime media tool on PATH: %s\n' "$tool" >&2
    exit 2
  }
  "$tool" -version </dev/null | head -n 1
done
