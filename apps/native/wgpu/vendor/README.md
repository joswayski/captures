# Private native-workbench renderer patches

`winit/` is the crates.io **0.30.13** package, with the upstream Apache-2.0
license retained. Archive SHA-256:

```text
a6755fa58a9f8350bd1e472d4c3fcc25f824ec358933bba33306d0b63df5978d
```

Only `apps/native/wgpu/Cargo.toml` patches the registry dependency. The root
workspace and shipping Tauri application do not use this fork. The isolated
Wayland test probe references it directly.

Local changes add COPY-only `ActiveEventLoopExtX11/Wayland::start_file_drag` and protocol-correct
Wayland `set_visible`/`is_visible` support. Visibility uses NULL-buffer unmapping, waits for a
generation-checked `wl_display.sync` before reporting hidden, and gates redraw until the remap
configure. It reports client mapping readiness, not compositor occlusion or minimization.
Hide first stops render readiness, then submits NULL on the event queue after a
drain sync. A second sync acknowledges unmapping. The custom xdg-surface dispatch
filters stale configures before SCTK auto-ACK; the drain alone cannot exclude
later compositor events. Show/hide during a bufferless remap retains its handshake.
The persistent decoration object keeps its mode. Repeated set_mode can crash
Sway 1.7's retired-container arrangement; that compositor also lacks the fresh
initial-configure remap fix and is unsupported. The pinned headless-only Sway 1.9
fixture exercises immediate present/hide, cancelled remaps and exact pixels.
Completion reports `(accepted_copy, own_client, exact_source_window)`. The file
must outlive completion. Source handling shares winit's connection and event
queue: a second Wayland connection cannot use the original pointer serial.
X11 takes over the client's XI2 implicit grab, handles selection requests,
Escape, own-window drops and a five-second missing-Finished deadline. Wayland
uses SCTK's data device/source/offer dispatch and held-button serial, retaining
the drop destination before leave. Its one-shot five-second post-drop timer
recovers missing completion; normal completion removes the timer. Neither adds
general Wayland file import. Same-app source offers now deliver
`HoveredFile`/`DroppedFile` to the destination window as well as acknowledging
COPY, so editor receivers can import History or mini-preview files. Those files
come from this source's retained path, not from an unrelated client offer.

Patch surface: `src/platform/{x11,wayland}.rs`, `platform_impl/linux/mod.rs`,
the Linux backend module/event-loop initialization, X11 atoms/event processor,
Wayland state/seat/pointer modules, and the two new `outbound_drag.rs` modules.
Visibility also changes Wayland window/state and redraw/frame dispatch.
Other package files are upstream copies; `.cargo-ok` is a Cargo cache marker,
not part of the published archive.

## Private eframe patch

`eframe/` is the **0.36.2** Git package at
[`60d7caae`](https://github.com/emilk/egui/commit/60d7caaea38a795618e842925061ad2210028a2a),
the same revision already used by the native workbench. MIT and Apache-2.0
licenses and the upstream README are retained. `cargo +1.95.0 vendor --locked`
produced the standalone manifest; its egui sibling dependencies are explicitly
pinned back to that Git revision rather than resolving registry substitutes.
No root workspace or shipping Tauri patch is involved.

Only `src/native/wgpu_integration.rs` has source changes: refresh acknowledged
Wayland mapping state for all viewports before a root pass; preserve compositor
occlusion separately; avoid painting unmapped immediate surfaces; and recreate
GPU surfaces after remapping. An explicitly hidden root runs its initial UI pass
without presenting a buffer. One-shot `RequestPaintWhileHidden` requests declare
new children without remapping History; consumed requests do not turn ordinary
worker wakes into recurring UI passes. Normally visible roots keep their initial
paint. Presentation uses the private `WindowExtWayland::is_surface_ready` gate,
distinct from acknowledged `is_visible`; paint requests cannot bypass drain/remap.
Pending texture deltas now share the renderer's single texture namespace
across root, deferred and immediate viewports, preserving allocation-before-update
order when a hidden root does not paint. This queue is shared on every host;
extra mapping and surface-reset behavior is gated to actual Linux Wayland window
handles. Glow is unused by this workbench and unchanged.

The wgpu integration also publishes weak native-window references in temporary
context data, keyed by `("eframe-winit-window", ViewportId)`. Development shutdown
intent queries the actual Preferences window's `is_visible`, independently of
focus/occlusion. Missing/unknown visibility does not fabricate an intent; weak
references do not keep retired windows alive. No painting or mapping policy changes.

Secondary-window initialization now creates the existing AccessKit adapter before
storing each egui-winit state. The shared event-loop proxy reaches deferred,
immediate and recreated windows; action/tree routing remains window-specific.
This is feature-gated on `accesskit` and shared by Windows, X11 and Wayland.
Root-only initialization previously left real editor windows absent from AT-SPI
even though their egui trees were covered by unit tests.

To update, vendor the new locked Git package into a temporary directory, copy
only eframe, retain its licenses/README, repin all egui siblings to the same
revision, and reapply this one-file patch against upstream. Do not retain vendor
checksum metadata for changed sources. Update the private lockfile and run native
gates, the visibility/native-portal/resident-lifecycle smokes and X11
capture/editor regressions.

## Updating winit

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
   recovery. Run `x11_history_smoke.py` for History-to-editor layer imports and
   source retention; the Wayland probe checks delivery to a different window in
   the same event loop. Run `apps/native/wayland_visibility_smoke.py` for the live
   headless-Sway NULL-buffer hide/remap and exact-pixel checks and
   `apps/native/wayland_native_capture_smoke.py` for actual host exclusion,
   repeated restoration and portal lifecycle. Run native
   macOS/Windows CI and physical host acceptance separately.

Headless Sway tests include a receiver that never finishes and one that exits
after receiving bytes. These tests do not establish physical compositor parity.
Do not mark the native drag or mini-preview parity gates complete from this patch.
