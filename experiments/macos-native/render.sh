#!/usr/bin/env bash
# Build and render deterministic real SwiftUI/AppKit surface fixtures on macOS.
set -euo pipefail
experiment="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
output="${1:-$experiment/build/references-$(date +%Y%m%d-%H%M%S)}"
if [[ "$(uname -s)" != Darwin ]]; then
  printf 'Reference rendering requires AppKit on macOS; this host is %s.\n' "$(uname -s)" >&2
  exit 1
fi
if [[ -e "$output" ]]; then
  printf 'Reference output already exists; choose a new path: %s\n' "$output" >&2
  exit 1
fi
for tool in python3 sips ffmpeg ffprobe; do command -v "$tool" >/dev/null; done
bash "$experiment/build.sh" --test
data="$(mktemp -d)"
fixtures="$(mktemp -d)"
trap 'rm -rf "$data" "$fixtures"' EXIT
# Extract the shipping React harness's neutral 960 × 540 source fixture so
# platform gallery references use identical non-private content.
python3 - "$experiment/../../apps/desktop/ui/src/dev/previewBackend.ts" "$fixtures/source.svg" <<'PY'
from pathlib import Path
import re
import sys

source = Path(sys.argv[1]).read_text()
match = re.search(r'function sampleCapture\(.*?const svg = `(.+?)`;', source, re.DOTALL)
if match is None:
    raise SystemExit('React sampleCapture fixture changed; update macOS render extraction')
Path(sys.argv[2]).write_text(match.group(1).replace('${width}', '960').replace('${height}', '540'))
PY
sips -s format png "$fixtures/source.svg" --out "$fixtures/fixture.png" >/dev/null
dimensions="$(sips -g pixelWidth -g pixelHeight "$fixtures/fixture.png" | awk '/pixelWidth:/{w=$2}/pixelHeight:/{h=$2}END{print w "x" h}')"
[[ "$dimensions" == 960x540 ]] || { printf 'Fixture raster is %s, expected 960x540.\n' "$dimensions" >&2; exit 1; }
ffmpeg -v error -y -loop 1 -framerate 30 -i "$fixtures/fixture.png" -t 3 \
  -c:v libx264 -pix_fmt yuv420p -movflags +faststart "$fixtures/fixture.mp4"
probe="$(ffprobe -v error -select_streams v:0 -show_entries stream=width,height \
  -of csv=p=0:s=x "$fixtures/fixture.mp4")"
[[ "$probe" == 960x540 ]] || { printf 'Video fixture probe is %s, expected 960x540.\n' "$probe" >&2; exit 1; }
CAPTURES_NATIVE_DATA="$data" \
  CAPTURES_NATIVE_CAPTURE_ANIMATION="${CAPTURES_NATIVE_CAPTURE_ANIMATION:-0}" \
  CAPTURES_NATIVE_REFERENCE_IMAGE="$fixtures/fixture.png" \
  CAPTURES_NATIVE_REFERENCE_VIDEO="$fixtures/fixture.mp4" \
  "$experiment/build/render-references" "$output"
find "$output" -type f -name '*.png' -print0 | sort -z | xargs -0 shasum -a 256 > "$output/SHA256SUMS"
printf 'Rendered native references:\n%s\n' "$output"
