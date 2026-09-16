# GPUI source patch for this experiment

`gpui/` is the crates.io **gpui 0.2.2** source package, Apache-2.0 licensed.
The original license, package metadata, and VCS information are retained.
Registry archive SHA-256:
`979b45cfa6ec723b6f42330915a1b3769b930d02b2d505f9697f8ca602bee707`.
The registry extraction marker `.cargo-ok` is omitted.

There are three upstream-file changes:

- `Cargo.toml` declares the opt-in `subpixel-images` feature and a standalone
  workspace so Cargo formatting/metadata does not attach it to the root app.
- `src/window.rs`, `Window::paint_image`, preserves floating device-pixel bounds
  when that feature is enabled. Without it, upstream's floor-origin/ceil-size
  behavior remains unchanged. Layout, texture allocation, shaders and sampling
  are unmodified.
- `src/platform/mac/window.rs` creates titlebar-less `PopUp` panels with the
  borderless style, retaining their nonactivating-panel behavior. Upstream used
  `Titled | FullSizeContentView` even with `titlebar: None`; AppKit's frame inset
  made the matched adapter's requested 640×720 content measure 640×721 on
  macOS 26. The fix removes that frame decoration rather than subtracting a
  point, clipping the viewport, or weakening validation. Ordinary document
  windows and explicitly titled popups keep their upstream styles.

Captures enables this feature and uses `motion::translated` to move complete
preview subtrees, including hitboxes, after static layout. Otherwise Taffy's
layout rounding and `paint_image` rounding both discard CSS-like fractional
animation positions. Both the app and matched component adapter use this path.

This is a maintained renderer fork, not an upstream GPUI feature. It affects
only the isolated experiment. Keep the vendored snapshot out of workspace-wide
formatting/refactors so the patch remains reviewable. On upgrades, compare the
source against the published package and repeat native pixel and interaction
checks on macOS, Windows, and Linux; compiling alone is not acceptance.
