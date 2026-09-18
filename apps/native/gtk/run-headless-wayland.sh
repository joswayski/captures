#!/usr/bin/env bash
set -euo pipefail

runtime="$(mktemp -d)"
chmod 700 "$runtime"
socket="captures-gtk-$RANDOM"
log="$runtime/weston.log"
cleanup() {
  if [[ -n "${weston_pid:-}" ]]; then
    kill "$weston_pid" 2>/dev/null || true
    wait "$weston_pid" 2>/dev/null || true
  fi
  rm -rf "$runtime"
}
trap cleanup EXIT

XDG_RUNTIME_DIR="$runtime" weston --backend=headless-backend.so --socket="$socket" \
  --width=1280 --height=800 --idle-time=0 --log="$log" &
weston_pid=$!
for _ in {1..100}; do
  [[ -S "$runtime/$socket" ]] && break
  if ! kill -0 "$weston_pid" 2>/dev/null; then
    cat "$log" >&2
    exit 1
  fi
  sleep .05
done
[[ -S "$runtime/$socket" ]] || { cat "$log" >&2; exit 1; }

XDG_RUNTIME_DIR="$runtime" WAYLAND_DISPLAY="$socket" GDK_BACKEND=wayland \
  GSK_RENDERER="${GSK_RENDERER:-gl}" "$@"
