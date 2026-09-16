# GPUI source patch for this experiment

`gpui/` is the crates.io **gpui 0.2.2** source package, Apache-2.0 licensed.
The original license, package metadata, and VCS information are retained.
Registry archive SHA-256:
`979b45cfa6ec723b6f42330915a1b3769b930d02b2d505f9697f8ca602bee707`.
The registry extraction marker `.cargo-ok` is omitted.

The maintained upstream changes are:

- `Cargo.toml` declares the opt-in `subpixel-images` feature and a standalone
  workspace so Cargo formatting/metadata does not attach it to the root app.
- `src/window.rs`, `Window::paint_image`, preserves floating device-pixel bounds
  when that feature is enabled. Without it, upstream's floor-origin/ceil-size
  behavior remains unchanged. Layout, shaders and sampling are unmodified.
  `Window::atlas_stats` exposes optional ownership diagnostics (not RSS).
- `src/platform/mac/window.rs` creates titlebar-less `PopUp` panels with the
  borderless style, retaining their nonactivating-panel behavior. Upstream used
  `Titled | FullSizeContentView` even with `titlebar: None`. Ordinary document
  windows and explicitly titled popups keep their upstream styles. The initial
  native window origin is also rounded to integral **points**, keeping content
  size unchanged: AppKit's outward rounding expanded the adapter to 640×721 at
  a half-point Y origin on a 2× Mac display. Borderless style alone did not fix
  this; native Mac CI reproduced 641×721 with both origin axes fractional. The
  aligned origin is used for creation and initial placement. No content-size
  subtraction, viewport clipping or weakened validation is involved.
- `src/platform.rs` and `src/platform/atlas_retirement.rs` add atlas ownership
  counters and completion-ordered retirement. Metal's atlas/renderer remove
  keys immediately and idempotently, then reclaim their allocator regions and
  empty textures only after all prior submitted GPU readers finish. An older
  command-buffer completion cannot release a newer frame's allocation.
- Blade's atlas/renderer use the same retirement queue, including pending
  uploads, and reclaim at the existing completed-sync-point boundary. No new
  GPU wait or frame-rate cap is added. Idle Blade retirement may retain its
  last batch until the next existing completion wait or window teardown.
  DirectX keeps its existing eviction backend and reports no atlas counters;
  native Windows resource-lifetime validation remains outstanding.

Captures enables this feature and uses `motion::translated` to move complete
preview subtrees, including hitboxes, after static layout. Otherwise Taffy's
layout rounding and `paint_image` rounding both discard CSS-like fractional
animation positions. Both the app and matched component adapter use this path.

This is a maintained renderer fork, not an upstream GPUI feature. It affects
only the isolated experiment. Keep the vendored snapshot out of workspace-wide
formatting/refactors so the patch remains reviewable. On upgrades, compare the
source against the published package and repeat native pixel and interaction
checks on macOS, Windows, and Linux; compiling alone is not acceptance.
