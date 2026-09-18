# GTK4 custom snapshot candidate

This is a bounded **Linux renderer comparison**, not a capture app or selected
production frontend. It hosts custom Captures-styled `GtkSnapshot` scenes in a
native GTK4 toplevel. Native `GtkEntry` children supply text editing, keyboard
focus, IME plumbing and text-box accessibility semantics without accepting stock
GTK visual styling. It uses synthetic fixture state only: no capture access,
settings persistence, history files, shortcuts, tray, network, updater or Tauri.

The probe is intentionally C/GTK rather than another implementation of native
business state. Production state remains owned by `captures-app` and
`captures-settings`; this workbench only measures whether GTK is a suitable Linux
host and renderer. Build-time Node reuses `apps/native/prepare.mjs`, so
`shared/design.css` and `shared/themes.css` remain the token source.

## Build and run

Requires GTK 4.6+ headers, json-glib, a C17 compiler and Node 24 at build time.
On Ubuntu 24.04:

```sh
sudo apt-get install build-essential libgtk-4-dev libjson-glib-dev
bash apps/native/gtk/build.sh
apps/native/gtk/build/captures-gtk-workbench --scene preferences
apps/native/gtk/build/captures-gtk-workbench --scene history --history-count 1000
apps/native/gtk/build/captures-gtk-workbench --scene hud --floating --exercise
apps/native/gtk/build/captures-gtk-workbench --scene preview --floating
apps/native/gtk/build/captures-gtk-workbench --scene editor --exercise
apps/native/gtk/build/captures-gtk-workbench --scene idle --quit-after 60
```

Options match the comparison contract: `--scene`, `--appearance light|dark`, `--theme`,
`--history-count`, `--exercise`, `--quit-after`, `--floating`,
`--reduced-motion`, `--screenshot`, and `--screenshot-after`. JSONL events report
ready state, native visibility/mapping, declared accessibility roles, edits, selections,
scripted outcomes, animation settlement, rendered alpha and teardown. A screenshot
uses GTK's GSK renderer for this native widget tree; it never captures the desktop.
`system` appearance is explicitly rejected because this bounded candidate does not
yet subscribe to GTK appearance changes; it must not silently resolve to dark.

## Implemented probes and boundaries

| Scene | Representative behavior | Deliberate gap |
| --- | --- | --- |
| Preferences | Custom token UI, styled native search entry, keyboard focus/editing, text-box semantics | Fixture values only; no persistence or complete settings |
| History | Empty/100/1,000 image-backed rows, visible-range snapshot construction, scroll/selection endpoints | One retained 2048×1152 synthetic texture, not real files or cache eviction pressure |
| HUD | Transparent undecorated window, running/paused and muted states | No recording, exclusion, topmost or layer-shell behavior |
| Preview | Transparent toplevel, pile/controls, visible/saved-hidden and reduced-motion states, bounded frame callback | Not dust parity, drag/drop, click-through, blur or placement parity |
| Editor | 2048×1152 synthetic texture, clipped pan/zoom/rotation, custom layers and a styled editable native text field | No document model, undo, export or full layer editing |
| Idle | Created but hidden GTK root, visibility/mapping query and clean teardown | No tray lifecycle, suspend/resume or long-run churn |

All static drawing is custom snapshot content. GTK only invokes snapshot again
after invalidation. The animated preview owns one frame-clock callback and removes
it after settling; smoke checks six settle events and rejects recurring static
snapshot passes. The prototype does not use `gtk4-layer-shell`: ordinary Wayland
toplevel placement remains compositor-controlled, which is an explicit overlay
gate rather than a silently claimed capability.

## Verify

```sh
# X11 + software GL
xvfb-run -a env GDK_BACKEND=x11 GSK_RENDERER=gl \
  python apps/native/gtk/smoke.py \
  --binary apps/native/gtk/build/captures-gtk-workbench \
  --output native-gtk-x11

# Headless Wayland/Weston + software GL
apps/native/gtk/run-headless-wayland.sh \
  python apps/native/gtk/smoke.py \
  --binary apps/native/gtk/build/captures-gtk-workbench \
  --output native-gtk-wayland
```

The smoke gate checks native hidden/visible/mapped state, no recurring snapshot
after a static scene settles, real X11 keyboard editing/focus, declared GTK
accessibility roles (not AT-SPI verification), eleven default/non-default screenshots, transparent render-target
corners, history/editor outcomes, thirty scripted actions, animation settlement
at the shared 2/6/10/14/18/22-second cadence, and clean teardown. Weston in CI has no virtual-keyboard automation protocol, so
Wayland verifies programmatic GTK editable focus/text outcomes but **not physical
keyboard/IME input**. AT-SPI screen-reader navigation also remains a real-desktop
gate; role declarations alone do not accept it.

The headless environments establish software-GL rendering and lifecycle behavior,
not hardware GPU cost, compositor placement, fractional scaling, production
overlay policy, screen-reader/IME quality, blur, click-through, energy, or
multi-monitor behavior. A GSK texture with alpha proves the scene is transparent;
it does not prove every production compositor will preserve that alpha.

### Resource diagnostics

Do not mix screenshots with resource trials. `apps/native/profile.py` already
understands this binary's CLI and JSONL readiness/action contract. The shared
runner needs only these centralized changes before using it for both comparison
candidates:

1. Add `gtk` to `--renderer` and allow it on Linux only.
2. Give `gtk` the same editor-inclusive workload list as `wgpu`.
3. Treat every non-AppKit workload label as `probe` (do not pass
   `--reference-chips`).

Until that shared integration lands, reuse the tested `trial()` helper directly
from `apps/native/profile.py`; do not invent different CPU/RSS definitions. Linux
RSS remains a diagnostic, not physical footprint, process-tree or GPU memory.

One orb diagnostic run (one trial per workload, software GL, 60.00-second sample)
measured the following. These are absolute local costs, not benchmark medians or
hardware acceptance:

| Backend/workload | CPU, one-core definition | Median / peak process RSS |
| --- | ---: | ---: |
| Xvfb X11, hidden | 0.000% | 90.4 / 90.4 MiB |
| Xvfb X11, Preferences static | 0.067% | 163.8 / 163.8 MiB |
| Weston Wayland, hidden | 0.000% | 31.8 / 31.8 MiB |
| Weston Wayland, Preferences static | 0.167% | 172.8 / 172.8 MiB |

These final samples retain the comparison-aligned 2048×1152 texture. An earlier
run with the undersized 568×320 backing texture measured Wayland software-GL
Preferences at 1.217% / 164.5 MiB; a separate 30-second Cairo diagnostic measured
0.033% / 31.6 MiB. Those are not equivalent final workloads, but the variance is
itself a warning against treating one headless software trial as acceptance. The
custom widget recorded one static snapshot pass in each case. Repeat warmups and
rotated trials on real integrated-GPU hardware before drawing a performance or
renderer conclusion.

## Candidate findings

- GTK can create, query and keep an unmapped root on both tested X11 and Wayland
  backends. This improves the specific hidden-idle gate that blocks the winit
  Wayland candidate.
- Static visible and hidden probes settle without recurring custom snapshot
  callbacks in both headless backends.
- GSK software GL renders transparent HUD/preview targets with zero-alpha corners,
  unlike the observed wgpu/Vulkan/Xvfb transparent-window failure. Real desktop
  compositor and hardware results are still required.
- Ordinary GTK4 Wayland toplevels cannot choose absolute overlay placement.
  Layer-shell/topmost/click-through policy is unsupported here and remains a
  production gate.
- Native entries avoid implementing text/IME from scratch, but styled entry
  snapshots can wake for cursor blinking while focused. Static-idle measurements
  intentionally leave entries unfocused; focused input must be measured as an
  active workload.

The stripped prototype executable is 62,384 bytes in this Ubuntu build and links
dynamically to system GTK4/json-glib and their dependencies. GTK and json-glib are
LGPL-licensed system libraries; distro/runtime availability and aggregate shared
library footprint matter more than this executable size and remain packaging
costs to compare.
