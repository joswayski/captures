#!/bin/bash
set -euo pipefail

FFMPEG_VERSION="8.1.2"
FFMPEG_SHA256="464beb5e7bf0c311e68b45ae2f04e9cc2af88851abb4082231742a74d97b524c"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILD_ROOT="${CAPTURES_FFMPEG_BUILD_ROOT:-$ROOT/target/ffmpeg-build}"
SOURCE_ARCHIVE="$BUILD_ROOT/ffmpeg-$FFMPEG_VERSION.tar.xz"
SOURCE_SIGNATURE="$SOURCE_ARCHIVE.asc"
SOURCE_DIRECTORY="$BUILD_ROOT/ffmpeg-$FFMPEG_VERSION"
BINARIES_DIRECTORY="$ROOT/apps/desktop/src-tauri/binaries"
COMPLIANCE_DIRECTORY="$ROOT/apps/desktop/src-tauri/ffmpeg"
DIST_DIRECTORY="$ROOT/target/ffmpeg-dist"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "FFmpeg recording sidecars are currently built for macOS only." >&2
  exit 1
fi

case "$(uname -m)" in
  arm64) TARGET_TRIPLE="aarch64-apple-darwin" ;;
  x86_64) TARGET_TRIPLE="x86_64-apple-darwin" ;;
  *) echo "Unsupported macOS architecture: $(uname -m)" >&2; exit 1 ;;
esac

FFMPEG_OUTPUT="$BINARIES_DIRECTORY/ffmpeg-$TARGET_TRIPLE"
FFPROBE_OUTPUT="$BINARIES_DIRECTORY/ffprobe-$TARGET_TRIPLE"
CONFIGURE_FLAGS=(
  --disable-autodetect
  --disable-debug
  --disable-doc
  --disable-ffplay
  --disable-gpl
  --disable-network
  --disable-nonfree
  --disable-programs
  --disable-shared
  --disable-version3
  --enable-audiotoolbox
  --enable-ffmpeg
  --enable-ffprobe
  --enable-small
  --enable-static
  --enable-videotoolbox
  --enable-zlib
)

mkdir -p "$BUILD_ROOT" "$BINARIES_DIRECTORY" "$COMPLIANCE_DIRECTORY" "$DIST_DIRECTORY"
if [[ ! -f "$SOURCE_ARCHIVE" ]]; then
  curl -fsSL "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz" -o "$SOURCE_ARCHIVE"
fi
ACTUAL_SHA256="$(shasum -a 256 "$SOURCE_ARCHIVE" | awk '{print $1}')"
if [[ "$ACTUAL_SHA256" != "$FFMPEG_SHA256" ]]; then
  echo "FFmpeg source checksum mismatch: expected $FFMPEG_SHA256, got $ACTUAL_SHA256" >&2
  exit 1
fi
if [[ ! -f "$SOURCE_SIGNATURE" ]]; then
  curl -fsSL "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz.asc" -o "$SOURCE_SIGNATURE"
fi

if [[ ! -x "$FFMPEG_OUTPUT" || ! -x "$FFPROBE_OUTPUT" || "${CAPTURES_REBUILD_FFMPEG:-0}" == "1" ]]; then
  if [[ "${CAPTURES_REBUILD_FFMPEG:-0}" == "1" && -d "$SOURCE_DIRECTORY" ]]; then
    rm -rf "$SOURCE_DIRECTORY"
  fi
  if [[ ! -d "$SOURCE_DIRECTORY" ]]; then
    tar -xf "$SOURCE_ARCHIVE" -C "$BUILD_ROOT"
  fi
  pushd "$SOURCE_DIRECTORY" >/dev/null
  make distclean >/dev/null 2>&1 || true
  ./configure "${CONFIGURE_FLAGS[@]}"
  make -j"$(sysctl -n hw.logicalcpu)" ffmpeg ffprobe
  cp ffmpeg "$FFMPEG_OUTPUT"
  cp ffprobe "$FFPROBE_OUTPUT"
  popd >/dev/null
  chmod 755 "$FFMPEG_OUTPUT" "$FFPROBE_OUTPUT"
fi

cp "$SOURCE_DIRECTORY/COPYING.LGPLv2.1" "$COMPLIANCE_DIRECTORY/COPYING.LGPLv2.1"
cp "$SOURCE_ARCHIVE" "$DIST_DIRECTORY/ffmpeg-$FFMPEG_VERSION.tar.xz"
cp "$SOURCE_SIGNATURE" "$DIST_DIRECTORY/ffmpeg-$FFMPEG_VERSION.tar.xz.asc"
cp "$COMPLIANCE_DIRECTORY/BUILD_CONFIG.txt" "$DIST_DIRECTORY/ffmpeg-$FFMPEG_VERSION-BUILD_CONFIG.txt"
cp "$COMPLIANCE_DIRECTORY/COPYING.LGPLv2.1" "$DIST_DIRECTORY/ffmpeg-$FFMPEG_VERSION-COPYING.LGPLv2.1"
cp "$COMPLIANCE_DIRECTORY/NOTICE.md" "$DIST_DIRECTORY/ffmpeg-$FFMPEG_VERSION-NOTICE.md"

node "$ROOT/scripts/validate-ffmpeg-sidecars.mjs" \
  "$FFMPEG_OUTPUT" \
  "$FFPROBE_OUTPUT" \
  "$COMPLIANCE_DIRECTORY/BUILD_CONFIG.txt"
