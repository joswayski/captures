#!/usr/bin/env bash
# Compare identical controlled SPA endpoints against real, private PipeWire builds.
set -euo pipefail
here="$(dirname "$(realpath "$0")")"
cache="${1:-${XDG_CACHE_HOME:-$HOME/.cache}/captures-native-tools}"
patched="$("$here/build_pipewire_fixture.sh" "$cache")"
unpatched="$("$here/build_pipewire_fixture.sh" "$cache" --unpatched)"
root="$(dirname "$patched")"
flags="$(PKG_CONFIG_PATH="$root/build/meson-uninstalled" pkg-config --cflags --libs libpipewire-0.3)"
# pkg-config emits compiler arguments, deliberately word-split here.
cc -Wall -Wextra -Werror -Wno-unused-parameter "$here/pipewire_sync_probe.c" \
  $flags -o "$root/pipewire-sync-probe"

run() {
  local build="$(dirname "$1")/build"
  shift
  env LD_LIBRARY_PATH="$build/src/pipewire" SPA_PLUGIN_DIR="$build/spa/plugins" \
    PIPEWIRE_MODULE_DIR="$build/src/modules" PIPEWIRE_CONFIG_DIR="$build/src/daemon" \
    timeout 10 "$root/pipewire-sync-probe" "$@"
}

for scenario in node output input output-error input-error; do
  # A broken fixture/setup or suppressed typed errors cannot count as a repro.
  run "$unpatched" "$scenario" --control
  run "$patched" "$scenario" --control
  status=0
  run "$unpatched" "$scenario" || status=$?
  if [[ "$status" != 2 ]]; then
    echo "Expected the specific premature-completion regression, got exit $status ($scenario)" >&2
    exit 1
  fi
  run "$patched" "$scenario"
done
echo "PASS: 5 stock regressions reproduced, 5 patched scenarios and 10 controls passed"
