#!/usr/bin/env bash
# Pinned, headless-only Sway fixture. No system installation or desktop replacement.
set -euo pipefail

cache="${1:-${XDG_CACHE_HOME:-$HOME/.cache}/captures-native-tools}"
mkdir -p "$cache"
root="$(realpath "$cache")/sway-fixture-1.9"
prefix="$root/prefix"
mkdir -p "$root" "$root/launch"

fetch() {
  local name="$1" url="$2" hash="$3"
  if [[ ! -f "$root/$name.tar.gz" ]]; then
    curl --fail --location --retry 3 "$url" -o "$root/$name.tar.gz" >&2
  fi
  printf '%s  %s\n' "$hash" "$root/$name.tar.gz" | sha256sum --check >&2
  if [[ ! -d "$root/$name" ]]; then
    tar -xzf "$root/$name.tar.gz" -C "$root"
  fi
}

build() {
  local name="$1"
  shift
  if [[ ! -f "$root/$name/build/meson-private/coredata.dat" ]]; then
    meson setup "$root/$name/build" "$root/$name" --prefix="$prefix" \
      --libdir=lib --buildtype=debugoptimized "$@" >&2
  fi
  ninja -C "$root/$name/build" -j2 >&2
  ninja -C "$root/$name/build" install >&2
}

fetch wayland-1.23.1 https://deb.debian.org/debian/pool/main/w/wayland/wayland_1.23.1.orig.tar.gz \
  158ec49af498f2558c7fbf7e8b070d010d4e270cc6076003a18a6c813f87e244
fetch wayland-protocols-1.36 https://gitlab.freedesktop.org/wayland/wayland-protocols/-/archive/1.36/wayland-protocols-1.36.tar.gz \
  c839dd4325565fd59a93d6cde17335357328f66983c2e1fb03c33e92d6918b17
fetch wlroots-0.17.4 https://gitlab.freedesktop.org/wlroots/wlroots/-/archive/0.17.4/wlroots-0.17.4.tar.gz \
  f424eadc64be3056542f98146707d5815d01ac8f6f745bcd8c5796b25058857a
fetch sway-1.9 https://codeload.github.com/swaywm/sway/tar.gz/refs/tags/1.9 \
  b6e4e8d74af744278201792bcc4447470fcb91e15bbda475c647d475bf8e7b0b

export PKG_CONFIG_PATH="$prefix/lib/pkgconfig:$prefix/share/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
export PATH="$prefix/bin:$PATH"
export LD_LIBRARY_PATH="$prefix/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
build wayland-1.23.1 -Dtests=false -Ddocumentation=false -Ddtd_validation=false
build wayland-protocols-1.36 -Dtests=false
build wlroots-0.17.4 -Dauto_features=disabled -Dbackends=[] -Drenderers=[] \
  -Dallocators=[] -Dsession=disabled -Dxwayland=disabled -Dexamples=false
build sway-1.9 -Dauto_features=disabled -Dxwayland=disabled -Dswaybar=true \
  -Dtray=enabled -Dsd-bus-provider=libsystemd -Dgdk-pixbuf=disabled \
  -Dswaynag=false -Dman-pages=disabled -Ddefault-wallpaper=false \
  -Dzsh-completions=false -Dbash-completions=false -Dfish-completions=false

# These launchers select only the fixture compositor's private libraries.
# Captures and the rest of the test process retain their normal environment.
for tool in sway swaymsg swaybar; do
  cat > "$root/launch/$tool" <<'LAUNCH'
#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
export LD_LIBRARY_PATH="$root/prefix/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export PATH="$root/prefix/bin:$PATH"
exec "$root/prefix/bin/$(basename "$0")" "$@"
LAUNCH
  chmod +x "$root/launch/$tool"
done
printf '%s\n' "$root/launch"
