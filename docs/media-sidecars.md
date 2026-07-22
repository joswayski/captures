# FFmpeg media sidecars

Captures uses FFmpeg and ffprobe only as separately bundled command-line programs for recording finalization, editing, GIF conversion, and file-size-targeted exports. Screen capture and the initial H.264/AAC recording are handled by macOS frameworks rather than FFmpeg.

The macOS sidecars are built from the source version and SHA-256 pinned in `scripts/build-ffmpeg-sidecars.sh`. The configuration intentionally disables GPL, nonfree, version-3-only, network, and autodetected external components. The system zlib is enabled explicitly for PNG posters. The build does not include libx264; H.264 export uses Apple's VideoToolbox encoder.

Build and validate the sidecars with:

```sh
npm run prepare:media
```

The command writes target-triple-suffixed executables under `apps/desktop/src-tauri/binaries`, which is ignored by Git, and prepares the matching source archive, detached upstream signature, exact build configuration, LGPL license, and notice under `target/ffmpeg-dist`.

Tauri bundles the executables and notices only into the macOS app. CI runs the same validation against the packaged app, including the required encoders and filters. Official releases attach the corresponding source and compliance files, and the final release gate rejects missing or GPL/nonfree configurations.

When changing the pinned FFmpeg release or configuration:

1. Update the version, source SHA-256, and flags in `scripts/build-ffmpeg-sidecars.sh`.
2. Keep `apps/desktop/src-tauri/ffmpeg/BUILD_CONFIG.txt` and `NOTICE.md` exact and current.
3. Run `CAPTURES_REBUILD_FFMPEG=1 npm run prepare:media` on each supported macOS architecture.
4. Run the repository validation gates and inspect the packaged application.
5. Confirm the draft release includes the matching source archive, signature, build configuration, LGPL license, and notice before publishing.

See [FFmpeg's legal guidance](https://ffmpeg.org/legal.html) and [Tauri's sidecar documentation](https://v2.tauri.app/develop/sidecar/) for the upstream requirements this packaging follows.
