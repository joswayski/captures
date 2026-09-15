#!/usr/bin/env bash
# Package the GTK4 frontend; distro FFmpeg supplies ffmpeg, ffprobe and ffplay.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="${1:-}"
output="${2:-$root/native-dist}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo 'Expected a three-part version' >&2; exit 2; }
[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]]
binary="$root/experiments/linux-native/target/release/captures-linux-native"
test -x "$binary"
mkdir -p "$output"
output="$(realpath "$output")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
package="$work/package"
app="$package/usr/lib/captures-native-preview"
mkdir -p "$app" "$package/usr/bin" "$package/usr/share/applications" \
  "$package/usr/share/icons/hicolor/128x128/apps" "$package/DEBIAN" "$work/debian"
install -m 755 "$binary" "$app/captures-linux-native"
cp "$root/LICENSE" "$app/LICENSE"
mkdir -p "$app/licenses/openh264"
cp "$root/apps/desktop/src-tauri/openh264/"* "$app/licenses/openh264/"
ln -s ../lib/captures-native-preview/captures-linux-native "$package/usr/bin/captures-native-preview"
cp "$root/apps/desktop/src-tauri/icons/128x128.png" "$package/usr/share/icons/hicolor/128x128/apps/captures-native-preview.png"
cat > "$package/usr/share/applications/es.captur.native-preview.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=Captures Native Preview
Comment=Experimental native screenshot and recording utility (X11)
Exec=captures-native-preview
Icon=captures-native-preview
Terminal=false
Categories=Graphics;Utility;
EOF
cat > "$work/debian/control" <<'EOF'
Source: captures-native-preview
Section: graphics
Priority: optional
Maintainer: Jose Valerio <contact@josevalerio.com>

Package: captures-native-preview
Architecture: amd64
Description: Experimental native screen capture utility
EOF
# Derive ABI dependencies from this actual build, rather than guessing GTK's
# transitive dependencies or promising compatibility with an older libc.
dependencies="$(cd "$work" && dpkg-shlibdeps -O -e"$binary")"
dependencies="${dependencies#shlibs:Depends=}"
test -n "$dependencies"
cat > "$package/DEBIAN/control" <<EOF
Package: captures-native-preview
Version: $version
Section: graphics
Priority: optional
Architecture: amd64
Maintainer: Jose Valerio <contact@josevalerio.com>
Depends: $dependencies, ffmpeg
Description: Experimental native screen capture utility (X11)
 GTK4 frontend with separate settings and history from the Tauri Preview.
 Updates are manual. Wayland capture is not supported.
EOF
dpkg-deb --root-owner-group --build "$package" "$output/Captures-Linux-x64.deb"

archive="$work/Captures-Native-Preview"
cp -a "$app" "$archive"
cat > "$archive/README.txt" <<'EOF'
Captures Native Preview — Linux x64, X11 only

This is a native binary archive, NOT a self-contained AppImage.
Official builds require glibc 2.39+ (Ubuntu 24.04 or newer), GTK4 4.8+,
ALSA, D-Bus, PipeWire, X11/XCB/XRandR and OpenSSL runtime libraries.
Install your distribution's ffmpeg package, including ffprobe and ffplay.
Run ldd ./captures-linux-native to check for missing shared libraries.
Then run ./captures-linux-native from this directory.

Settings and history are separate from Tauri; no migration or automatic
updates are provided. Quit Tauri before launching to avoid shortcut conflicts.
Download a new archive to update; do not delete your native profile.
Known gaps: https://github.com/joswayski/captures/blob/main/docs/native-platforms.md
EOF
tar -C "$work" -czf "$output/Captures-Linux-x64.tar.gz" Captures-Native-Preview
