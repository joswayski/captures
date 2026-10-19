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
recovers missing completion; normal completion removes the timer. Same-app source
offers deliver
`HoveredFile`/`DroppedFile` to the destination window as well as acknowledging
COPY, so editor receivers can import History or mini-preview files. Those files
come from this source's retained path, not from an unrelated client offer.

External COPY-only Wayland imports use `inbound_drag.rs`: nonblocking calloop
reads (64 KiB per dispatch), a 1 MiB URI-list limit, a two-second absolute receive
deadline and eight retained offers. Lists are split before byte-preserving percent
decoding; remote authorities, malformed escapes, NUL and URL query/fragment text
reject the entire offer. Completion requires valid EOF and COPY at drop. Leave,
window/device loss, timeout and read failure destroy unfinished offers without
`finish`. Dropped offers retain their original window identity and release position
across subsequent drags. `WindowExtWayland::take_file_drop_batches` exposes complete
offers during `DroppedFile` callbacks; per-file events remain for other consumers,
and unconsumed metadata expires after dispatch. Same-client drops use this metadata
too. Clipboard selection handling is unchanged.

Patch surface: `src/platform/{x11,wayland}.rs`, `platform_impl/linux/mod.rs`,
the Linux backend module/event-loop initialization, X11 atoms/event processor,
Wayland state/seat/pointer modules, and the two new `outbound_drag.rs` modules.
Visibility also changes Wayland window/state and redraw/frame dispatch.
Other package files are upstream copies; `.cargo-ok` is a Cargo cache marker,
not part of the published archive.

Wayland `ActiveEventLoopExtWayland::set_clipboard_marker` reuses the existing
SCTK data-device/source dispatch to offer a private editor-layer MIME on the
focused seat, using its latest keyboard/pointer input serial. Ownership starts
false, becomes true only on a matching selection offer, and is invalidated by
replacement or source cancellation. The retained source serves only the opaque
marker, not text or serialized document data. It adds no dependency and does
not change file-drag routing or PRIMARY. The additional patch surface is the
keyboard serial/focus bookkeeping and clipboard state in the modules above.
`clipboard_offer` reports whether that seat advertises PNG and receives only
an advertised `text/uri-list`, returning its pipe for bounded worker-side reads.
This lets unsupported text/HTML-only and empty selections remain silent without
hiding real image-transfer failures. It neither decodes paths nor blocks the
native queue, and does not require data-control. Run
`apps/native/wayland_editor_clipboard_smoke.py` for real compositor-delivered
Copy/Paste, private payload bytes, external replacement, cancellation/reclaim,
image pixels and CRLF file ordering; physical compositor acceptance stays open.

X11 capture-key registration now uses `EventLoopExtX11::hotkey_manager` on
winit's existing X connection. Root passive-grab key edges dispatch before
IME conversion on the same native queue as pointer input, rather than a
second global-hotkey worker. Checked registration rolls back its successful
lock-key variants on conflict; unregister/Drop remove only owned grabs.
Detectable autorepeat suppresses duplicate presses without losing releases
after modifiers lift. The host retains this manager across settings changes
and recorder suspension. This is X11-specific: Windows still uses its existing
transport, and Wayland retains desktop-portal shortcut consent and ownership.
The additional patch surface is `platform_impl/linux/x11/hotkeys.rs` and the
X11 manager owner/factory/event dispatch in the files listed above.

## Private eframe patch

`eframe/` is the **0.36.2** Git package at
[`60d7caae`](https://github.com/emilk/egui/commit/60d7caaea38a795618e842925061ad2210028a2a),
the same revision already used by the native workbench. MIT and Apache-2.0
licenses and the upstream README are retained. `cargo +1.95.0 vendor --locked`
produced the standalone manifest; its egui sibling dependencies are explicitly
pinned back to that Git revision rather than resolving registry substitutes.
No root workspace or shipping Tauri patch is involved.

`src/native/wgpu_integration.rs` refreshes acknowledged
Wayland mapping state for all viewports before a root pass; preserves compositor
occlusion separately; avoids painting unmapped immediate surfaces; and recreates
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
the window's first show. An initially hidden creation-builder copy preserves
the retained builder's desired visibility; intentionally hidden windows stay hidden.
The shared event-loop proxy reaches deferred, immediate and recreated windows;
action/tree routing remains window-specific.
This is feature-gated on `accesskit` and shared by Windows, X11 and Wayland.
Root-only initialization previously left real editor windows absent from AT-SPI
even though their egui trees were covered by unit tests.

The same integration consumes Wayland drop batches instead of forwarding duplicate
per-file events. `src/lib.rs` exposes `FileDropBatch::take` for the current viewport,
with positions converted to egui points. Viewport removal/recreation clears queued
drops. Other platforms keep egui-winit's raw-file path; no egui-winit fork is needed.

To update, vendor the new locked Git package into a temporary directory, copy
only eframe, retain its licenses/README, repin all egui siblings to the same
revision, and reapply these local patches against upstream. Do not retain vendor
checksum metadata for changed sources. Update the private lockfile and run native
gates, the visibility/native-portal/resident-lifecycle smokes and X11
capture/editor regressions.

## Private SCTK offer ownership patch

`sctk/` is the crates.io **smithay-client-toolkit 0.19.2** package, with its MIT
license retained. Archive SHA-256:

```text
3457dea1f0eb631b4034d61d4d8c32074caa6cd1ab2d59f2327bd8461e2c0016
```

`src/data_device_manager/data_device.rs` adds `take_drag_offer`, which transfers
ownership to the destination. Upstream snapshots cannot survive a new enter, which
destroys even the previous dropped offer; an ordinary leave destroys an undropped
offer. Detached offers still receive action callbacks, while the caller tracks
motion/leave/drop and owns destruction. Winit uses this only for external transfers;
its existing own-source and clipboard paths retain SCTK ownership. Both isolated
native manifests patch this version; root Cargo/Tauri do not. To update, compare
against the verified archive and reapply this method only if upstream still needs
it. Do not copy `.cargo-ok` or vendor checksum metadata for modified sources.
Five existing signatures also have compiler-suggested, behavior-neutral syntax
updates for Rust 1.95's warnings-as-errors gate: redundant trait-object parentheses
in `src/compositor.rs`, `src/shell/xdg/mod.rs` and `src/shm/raw.rs`, and explicit
lifetimes in `src/data_device_manager/write_pipe.rs` and `src/registry.rs`.
Run the editor clipboard/picker/external-GTK drop smoke and outbound drag smoke
after every ownership/backend update; physical compositor acceptance remains open.

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
