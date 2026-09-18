#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
out="$root/apps/native/gtk/build"
resources="$root/apps/native/gtk/resources"

node "$root/apps/native/prepare.mjs" --output "$resources"
mkdir -p "$out"

cc -std=c17 -O2 -g0 -Wall -Wextra -Werror \
  "$root/apps/native/gtk/main.c" \
  -o "$out/captures-gtk-workbench" \
  $(pkg-config --cflags --libs gtk4 json-glib-1.0) -lm
