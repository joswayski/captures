#!/usr/bin/env bash
# Pinned gtk4-layer-shell for the GTK host's `layer-shell` feature, built
# into a private prefix. Prints a PKG_CONFIG_PATH entry; run binaries with
# LD_LIBRARY_PATH="$prefix/lib". Distributions that package
# libgtk4-layer-shell-dev (1.0 or newer) can use that instead.
set -euo pipefail

cache="${1:-${XDG_CACHE_HOME:-$HOME/.cache}/captures-native-tools}"
mkdir -p "$cache"
root="$(realpath "$cache")/gtk4-layer-shell-1.3.0"
prefix="$root/prefix"
mkdir -p "$root"
if [[ ! -f "$root/source.tar.gz" ]]; then
  curl --fail --location --retry 3 \
    https://github.com/wmww/gtk4-layer-shell/archive/refs/tags/v1.3.0.tar.gz \
    -o "$root/source.tar.gz" >&2
fi
printf '%s  %s\n' 1ebb01ab14e98afd1727f68f64981c37bd23305b1f131f5667c02b94cf593192 \
  "$root/source.tar.gz" | sha256sum --check >&2
if [[ ! -d "$root/source" ]]; then
  mkdir "$root/source"
  tar -xzf "$root/source.tar.gz" -C "$root/source" --strip-components=1
fi
if [[ ! -f "$root/build/meson-private/coredata.dat" ]]; then
  meson setup "$root/build" "$root/source" --prefix="$prefix" --libdir=lib \
    --buildtype=release -Dintrospection=false -Dvapi=false -Dexamples=false \
    -Ddocs=false -Dtests=false -Dsmoke-tests=false >&2
fi
ninja -C "$root/build" >&2
ninja -C "$root/build" install >&2
printf '%s\n' "$prefix/lib/pkgconfig"
