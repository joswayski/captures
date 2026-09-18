# Shared wgpu renderer candidate

An **experimental Windows/Linux fixture workbench**, not the chosen production UI
and not a capture app. macOS keeps its Swift/AppKit frontend. This candidate uses
Rust, winit native windows, egui custom-drawn controls, and wgpu. It has no WebView,
JS runtime, Tauri dependency, capture permissions, settings persistence, network
service, installer, or updater. Node is build-time only.

The candidate tests whether shared custom components are viable. It is not a
retained widget renderer: egui rebuilds the visible UI on an event-driven repaint,
while image textures remain resident until a scene closes. Static scenes request
no recurring repaint. Compare this approach with DirectComposition/Direct2D on
Windows and GTK4 custom snapshots on Linux before selecting a renderer.

## Build

Requires Node 24, Rust **1.95.0**, and native build tools (MSVC/Windows SDK on
Windows; a C compiler, pkg-config, Wayland/X11/xkbcommon development libraries on
Linux). A working Vulkan or other wgpu-supported graphics driver is required.
The isolated Cargo workspace/lockfile leaves the shipping Rust 1.94 workspace
unchanged. eframe 0.36.2 includes the native idle-loop fix absent from 0.34.3;
do not downgrade solely to match the shipping toolchain.

From the repository root, on either Windows or Linux:

```sh
rustup toolchain install 1.95.0 --profile minimal --component rustfmt --component clippy
node apps/native/prepare.mjs --output apps/native/wgpu/resources
cargo +1.95.0 build --manifest-path apps/native/wgpu/Cargo.toml --locked --release
```

Run `apps/native/wgpu/target/release/captures-wgpu-workbench` (add `.exe` on
Windows). Resources are embedded; no separate resource directory is needed at
runtime. CI also uploads unsigned experimental binaries and viewport screenshots;
these are not Preview installers. Its Linux binary targets Ubuntu 24.04, not every
supported production distro.

```sh
apps/native/wgpu/target/release/captures-wgpu-workbench --scene preferences
apps/native/wgpu/target/release/captures-wgpu-workbench --scene history --history-count 1000
apps/native/wgpu/target/release/captures-wgpu-workbench --scene hud --floating --appearance light
apps/native/wgpu/target/release/captures-wgpu-workbench --scene preview --floating
apps/native/wgpu/target/release/captures-wgpu-workbench --scene editor
```

Appearance: `--appearance system|light|dark`; palettes: `--theme cobalt` (or any
existing preset). Close the native window to quit; floating fixtures have Close
and Move window controls. Launch one instance at a time for measurements.

## Implemented probes and deliberate gaps

| Scene | Exercise | Not implemented / not accepted |
| --- | --- | --- |
| Preferences | Token palette, appearance/accent switching, editable search, fixture toggles | Persistence, full settings, custom colors, font/visual parity |
| History | Empty/100/1,000 rows, filters, virtualized scrolling, selection, image-backed rows | Real files, open/delete, thumbnail cache pressure: rows intentionally share one synthetic texture |
| HUD | Running/paused/muted fixture; fixed glass palette even in light mode | Real timer/recording; recording exclusion; tray or hidden-controls notice |
| Preview | Cold/reused texture, fade/settle, reset mid-animation, explicit Reduce motion, optional transparent native window | **Not the shipping dust effect**: no isolated-chip blur, dust trajectories, source treatment or pile/drag/hit-region parity |
| Editor | 2048×1152 synthetic image, clipped canvas, pan/zoom/rotate, separate outline/text layers, editable text field | Real document, layer editing/undo/export; outlines/text do not rotate with the image |
| Idle | Hidden native window; no scheduled application work except optional quit deadline | Process/GPU teardown after last window; production tray lifecycle |

The current screens are token-styled fixtures, not pixel-parity reproductions.
Default bundled fonts differ from the shipping system-font stack. AccessKit is
enabled, but screen-reader navigation and IME need real platform testing; painted
images/canvas layers lack full semantic nodes. Reduce motion is an explicit probe
switch, not yet connected to each OS setting. Transparency does not imply desktop
blur, click-through, topmost behavior, or correct Wayland overlay placement.

## Validate and collect evidence

```sh
cargo +1.95.0 fmt --manifest-path apps/native/wgpu/Cargo.toml --all -- --check
cargo +1.95.0 test --manifest-path apps/native/wgpu/Cargo.toml --locked
cargo +1.95.0 clippy --manifest-path apps/native/wgpu/Cargo.toml --locked --all-targets -- -D warnings
python -m unittest discover -s apps/native -p 'test_*.py'
python apps/native/wgpu/smoke.py --binary apps/native/wgpu/target/release/captures-wgpu-workbench --output native-smoke
python apps/native/profile.py --renderer wgpu --binary apps/native/wgpu/target/release/captures-wgpu-workbench --output native-resources
```

Add `.exe` to both binary paths on Windows. Output directories must not exist.
Smoke tests need an interactive desktop or a test compositor. They check two idle
cases, eleven framebuffer captures, and thirty scheduled actions. Inspect the
PNGs: their presence alone is not visual acceptance. Capture another state with
`--screenshot capture.png --screenshot-after 3 --exercise`; this reads only the
workbench's framebuffer, never your desktop. Screenshots stop the app after saving
and must be collected separately from resource trials.

The profiler takes about 44 minutes by default (eleven workloads, one excluded
warmup and three 60-second trials). It records process CPU-time deltas plus Linux
RSS or Windows working set, raw samples, initialization and action events. Neither
memory counter is physical footprint, GPU memory, or process-tree accounting.
Ready means renderer initialization, not first presented pixels. UI construction
timings are wall time, not input latency or GPU presentation timing. Use native
profilers for those acceptance gates. The wgpu preview probe and AppKit dust are
different workloads: **do not compare their effect timings as equivalent work**.

The `ready` event identifies the adapter/backend and whether wgpu reports a CPU
device. Xvfb/headless Weston plus Mesa llvmpipe can verify Linux rendering and
window lifecycle but cannot establish real-GPU performance, desktop-compositor
behavior, energy use, multi-monitor DPI, or screen-reader/IME acceptance. Test X11
and Wayland separately. CI Windows is also not a substitute for maintainer desktop
testing. No platform parity row is closed by this workbench.
