#!/usr/bin/env bash
set -euo pipefail

binary="${1:?usage: smoke-render.sh PATH_TO_CAPTURES_GPUI}"
for tool in xdotool; do command -v "$tool" >/dev/null; done
[[ -n "${DISPLAY:-}" ]] || { echo 'Run under xvfb-run or an X11 session.' >&2; exit 2; }

profile="$(mktemp -d)"
log="$(mktemp)"
pid=
cleanup() {
  [[ -z "$pid" ]] || kill "$pid" 2>/dev/null || true
  [[ -z "$pid" ]] || wait "$pid" 2>/dev/null || true
  rm -rf "$profile" "$log"
}
trap cleanup EXIT

CAPTURES_GPUI_DATA="$profile" XDG_SESSION_TYPE=x11 "$binary" --preferences >"$log" 2>&1 &
pid=$!
for _ in $(seq 1 100); do
  if ! kill -0 "$pid" 2>/dev/null; then
    cat "$log" >&2
    echo 'GPUI exited before presenting a window.' >&2
    exit 1
  fi
  for window in $(xdotool search --onlyvisible --pid "$pid" 2>/dev/null || true); do
    geometry="$(xdotool getwindowgeometry --shell "$window")"
    width="$(sed -n 's/^WIDTH=//p' <<<"$geometry")"
    height="$(sed -n 's/^HEIGHT=//p' <<<"$geometry")"
    # GPUI also owns a mapped 1×1 input-method helper. Only a substantial
    # top-level surface demonstrates that application UI reached X11.
    if ((width >= 200 && height >= 150)); then
      name="$(xdotool getwindowname "$window")"
      printf 'Mapped visible GPUI X11 window (pixels not verified): %s (%s)\n' "$name" "$(tr '\n' ' ' <<<"$geometry")"
      exit 0
    fi
  done
  sleep 0.1
done
cat "$log" >&2
echo 'GPUI kept running but no visible X11 window appeared within 10 seconds.' >&2
exit 1
