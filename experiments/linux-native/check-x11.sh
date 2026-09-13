#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")" && pwd)
repository=$(cd "$root/../.." && pwd)
artifacts=${1:-"$repository/.amp/in/artifacts/linux-native"}
lab=$(mktemp -d -t captures-linux-native-x11.XXXXXX)
cleanup() {
  rm -rf "$lab"
}
trap cleanup EXIT
mkdir -p "$artifacts"

dbus-run-session -- xvfb-run -a \
  -s '-screen 0 1600x1000x24 +extension GLX +render -noreset' \
  bash -eu -o pipefail -c '
    root=$1
    lab=$2
    artifacts=$3
    /usr/bin/python3 "$root/native_desktop.py" "$lab" &
    fixture=$!
    trap '\''kill "$fixture" 2>/dev/null || true; wait "$fixture" 2>/dev/null || true'\'' EXIT
    for _ in $(seq 1 200); do
      test -f "$lab/environment.json" && break
      sleep .05
    done
    test -f "$lab/environment.json"
    export GDK_BACKEND=x11 GSK_RENDERER=gl
    /usr/bin/python3 "$root/native_check.py" --lab "$lab" --artifacts "$artifacts"
  ' -- "$root" "$lab" "$artifacts"
