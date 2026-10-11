#!/usr/bin/env bash
# Private test daemon only. Never install or export its libraries to capture clients.
set -euo pipefail

cache="${1:-${XDG_CACHE_HOME:-$HOME/.cache}/captures-native-tools}"
variant="${2:---patched}"
case "$variant" in
  --patched|--unpatched) ;;
  *) echo "usage: $0 [cache-directory] [--patched|--unpatched]" >&2; exit 2 ;;
esac
mkdir -p "$cache"
root="$(realpath "$cache")/pipewire-1.0.5-sync-v1${variant}"
source="$root/source"
build="$root/build"
revision=a2287be601710eea0d073261223ec34b92384c8a
patch="$(dirname "$(realpath "$0")")/pipewire-fixture-sync.patch"
mkdir -p "$root"
if [[ ! -d "$source" ]]; then
  git clone --quiet --depth 1 --branch 1.0.5 https://github.com/PipeWire/pipewire.git "$source"
fi
[[ "$(git -C "$source" rev-parse HEAD)" == "$revision" ]]
if [[ "$variant" == --patched ]]; then
  # The exact upstream pin bounds this zero-context patch, like the wlr fixture.
  if ! git -C "$source" apply --unidiff-zero --reverse --check "$patch" 2>/dev/null; then
    git -C "$source" diff --exit-code >&2
    git -C "$source" apply --unidiff-zero --check "$patch"
    git -C "$source" apply --unidiff-zero "$patch"
  fi
  # Reverse-check plus exact edit counts rejects drift elsewhere in the cache.
  [[ "$(git -C "$source" diff --numstat)" == $'1\t1\tsrc/pipewire/impl-link.c\n1\t1\tsrc/pipewire/impl-node.c' ]]
else
  git -C "$source" diff --exit-code >&2
fi
git -C "$source" diff --cached --exit-code >&2
if [[ ! -f "$build/meson-private/coredata.dat" ]]; then
  meson setup "$build" "$source" --prefix="$root/unused-prefix" --libdir=lib \
    --buildtype=debugoptimized -Dauto_features=disabled -Dsession-managers=[] \
    -Ddbus=enabled -Dflatpak=enabled -Dtests=disabled -Dexamples=disabled \
    -Dpipewire-jack=disabled -Dpipewire-v4l2=disabled -Dsystemd-user-service=disabled \
    -Drlimits-install=false >&2
fi
ninja -C "$build" -j2 >&2

# Keep all four overrides inside this process. The portal, WirePlumber and
# Captures still load their installed libraries and use the ordinary protocol.
cat > "$root/pipewire" <<'LAUNCH'
#!/usr/bin/env bash
set -euo pipefail
root="$(dirname "$(realpath "$0")")"
export LD_LIBRARY_PATH="$root/build/src/pipewire"
export PIPEWIRE_MODULE_DIR="$root/build/src/modules"
export SPA_PLUGIN_DIR="$root/build/spa/plugins"
export PIPEWIRE_CONFIG_DIR="$root/build/src/daemon"
exec "$root/build/src/daemon/pipewire" "$@"
LAUNCH
chmod +x "$root/pipewire"
printf 'Private PipeWire %s %s\n' "$revision" "$variant" >&2
if [[ "$variant" == --patched ]]; then sha256sum "$patch" >&2; fi
printf '%s\n' "$root/pipewire"
