# Private native-workbench winit patch

`winit/` is the crates.io **0.30.13** package, with the upstream Apache-2.0
license retained. Archive SHA-256:

```text
a6755fa58a9f8350bd1e472d4c3fcc25f824ec358933bba33306d0b63df5978d
```

Only `apps/native/wgpu/Cargo.toml` patches the registry dependency. The root
workspace and shipping Tauri application do not use this fork. The isolated
Wayland test probe references it directly.

Local changes add COPY-only `ActiveEventLoopExtX11/Wayland::start_file_drag`.
Completion reports `(accepted_copy, own_client, exact_source_window)`. The file
must outlive completion. Source handling shares winit's connection and event
queue: a second Wayland connection cannot use the original pointer serial.
X11 takes over the client's XI2 implicit grab, handles selection requests,
Escape, own-window drops and a five-second missing-Finished deadline. Wayland
uses SCTK's data device/source/offer dispatch and held-button serial, retaining
the drop destination before leave. Its one-shot five-second post-drop timer
recovers missing completion; normal completion removes the timer. Neither adds
general Wayland file import.

Patch surface: `src/platform/{x11,wayland}.rs`, `platform_impl/linux/mod.rs`,
the Linux backend module/event-loop initialization, X11 atoms/event processor,
Wayland state/seat/pointer modules, and the two new `outbound_drag.rs` modules.
Other package files are upstream copies; `.cargo-ok` is a Cargo cache marker,
not part of the published archive.

## Updating

1. Download the exact published archive from
   `https://static.crates.io/crates/winit/winit-0.30.13.crate`, verify the hash,
   and compare its extracted tree with this directory. Review the local patch,
   not the full vendored package as newly authored code.
2. Prefer an upstream supported outbound-drag API when compatible with eframe.
   Otherwise reapply the bounded protocol changes to the new release, update
   the exact pin, private lockfile and provenance here. Do not patch root Cargo.
3. Run private wgpu fmt/tests/Clippy, X11 preview `--drag-only` in normal and
   reduced-motion modes, and `apps/native/wayland_drag_smoke.py` (GTK receiver).
   Verify cancellation, repeated use, self-drop, Unicode URI bytes and pointer
   recovery. Run native macOS/Windows CI and physical host acceptance separately.

Headless Sway tests include a receiver that never finishes and one that exits
after receiving bytes. These tests do not establish physical compositor parity.
Do not mark the native drag or mini-preview parity gates complete from this patch.
