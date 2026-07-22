# FFmpeg sidecar notice

Captures bundles unmodified-command-line builds of FFmpeg and ffprobe 8.1.2.
They are separate executables used for local trim, crop, resize, audio mixing,
GIF conversion, and size-targeted export.

The bundled configuration is LGPL-only: GPL, nonfree, version-3-only, network,
and autodetected external components are disabled. The system zlib is enabled
explicitly for PNG posters. H.264 export uses Apple's VideoToolbox encoder;
Captures does not bundle libx264.

The corresponding source archive, detached FFmpeg signature, exact build
configuration, LGPL license, and this notice are distributed with Captures
release assets. FFmpeg is available from https://ffmpeg.org/.
