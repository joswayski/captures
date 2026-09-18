# Windows DirectComposition comparison

This is a **synthetic renderer candidate**, not a replacement app or a selected
Windows frontend. It compares Win32 windowing through winit plus Direct2D,
DirectWrite and DirectComposition with the [wgpu candidate](../wgpu/README.md).
No Tauri, React, WebView, JavaScript runtime or capture permission is involved.
The shipping Preview and native live workspace are unchanged.

## Build and exercise on Windows

Requires Windows 11, Rust 1.94 with the MSVC toolchain/Windows SDK, Node 24 for
build-time shared token compilation, and Python 3.12 for diagnostics.

```powershell
node apps/native/prepare.mjs --output apps/native/dcomp/resources
cargo build --manifest-path apps/native/dcomp/Cargo.toml --locked --release
apps/native/dcomp/target/release/captures-dcomp-workbench.exe --scene preferences
apps/native/dcomp/target/release/captures-dcomp-workbench.exe --scene history --history-count 1000
apps/native/dcomp/target/release/captures-dcomp-workbench.exe --scene preview --floating --exercise --quit-after 30
apps/native/dcomp/target/release/captures-dcomp-workbench.exe --scene editor
```

The shared CLI also accepts light/dark/system appearance, all nine themes, an
empty history and static countdown. `--live` and `--settings-file` fail explicitly:
this comparator must not silently pretend to integrate the shared application.
Preferences has a basic editable filter and appearance toggle. History creates
only visible rows and retains one shared synthetic image. The HUD toggles pause
and mute. The editor probes clipped image pan/zoom and simple annotation text.
These are not full settings, media or editor implementations.

`--floating` creates a transparent HUD or preview window. Space fades/restores
the entire preview through a DWM opacity animation; Reduce Motion makes it
immediate. There is no per-frame application timer. This is **not** the shipping
dust/settle effect. Ordinary (nonfloating) preview toggles fixture content without
animation. Floating windows do not yet implement shaped hit regions, click-through
or tray behavior. Escape closes the probe, not a global capture cancellation.

## Validation and evidence

```powershell
cargo fmt --manifest-path apps/native/dcomp/Cargo.toml --all -- --check
cargo test --manifest-path apps/native/dcomp/Cargo.toml --locked
cargo clippy --manifest-path apps/native/dcomp/Cargo.toml --locked --all-targets -- -D warnings
python -m unittest discover -s apps/native -p 'test_*.py'
python -m pip install Pillow==11.3.0
python apps/native/dcomp/smoke.py --binary apps/native/dcomp/target/release/captures-dcomp-workbench.exe --output dcomp-smoke
python apps/native/profile.py --renderer dcomp --binary apps/native/dcomp/target/release/captures-dcomp-workbench.exe --output dcomp-profile
```

Output directories must be new. The smoke script checks native hidden visibility,
settled repaint counts, thirteen surface readbacks (including light/dark,
empty/populated, paused and transformed states), and thirty scripted actions.
It separately captures the floating window's **desktop-composited client area**
before/after/restore, plus observer GIFs with raw timestamps, under normal and
reduced motion. Those images include the desktop visible behind transparency;
run them on an empty test desktop before sharing evidence. Surface readbacks
alone cannot verify DWM effects. GIF sampling is not displayed-FPS measurement.

The profiler samples process CPU and working set for static and active screens,
including a separate floating composition workload. Its default uses an excluded
warmup and three rotated 60-second trials. CI uses shorter 30-second warmup/trials
for diagnostics, not hardware acceptance. D3D uses hardware when available and
falls back to WARP explicitly reported in `ready`. CPU excludes DWM; working set
is not physical footprint or total GPU allocation. Readiness means the renderer
initialized, not first presentation. UI timings include layout and D2D submission,
not GPU completion. Do not compare these synthetic figures to the full Tauri app.

## Open selection gates

- Custom-drawn controls have **no UI Automation provider**. Tab selects the text
  field; there is no complete focus traversal. Basic Unicode typing/backspace and
  IME commits are a probe, not selection/clipboard/preedit or grapheme parity.
- Layout shares Captures colors, font sizes, corner radii and selected spacing,
  but remains synthetic. It is not pixel/UX parity and has no editor rotation,
  layered document model, real media, dust or backdrop blur.
- Device graph recreation and DPI-dependent surface sizing are implemented;
  hardware device-loss injection, mixed/fractional DPI, screen sleep, hit testing,
  Narrator/IME and long-run resource churn still require interactive Windows tests.
- Linux tests/cross-clippy only check the scene/CLI and Windows API compilation.
  They cannot execute DirectComposition. Actual Windows CI/render evidence and
  real-hardware trials are separate gates. No Windows renderer is selected here.

Use the [migration checklist](../../../docs/native-rewrite.md) for full acceptance.
