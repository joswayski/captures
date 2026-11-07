# Captures GTK4 host (Linux)

The Linux replacement for the egui/wgpu workbench: GTK4 for windows, text,
input and AT-SPI, with Captures' own token-drawn styling and controls. See
[native-rewrite.md](../../../docs/native-rewrite.md#host-direction) for the
direction and the gates it must pass. Preferences (every card, Find, the
compact layout, the microphone list and the Preview-only update check),
Capture History, the screenshot and recording editors, Send Feedback and the
floating previews (History's Restore) exist so far.

```sh
node apps/native/prepare.mjs --output apps/native/gtk/resources
cargo +1.95.0 run --manifest-path apps/native/gtk/Cargo.toml -- \
  --settings-file /tmp/captures-gtk/settings.json
```

`--appearance light|dark`, `--size WxH` and `--screenshot out.png` render one
frame and exit, for parity comparisons. `--scene history --history-root DIR`
opens Capture History, and `--scene editor --artifact ID --history-root DIR`
opens the screenshot editor on one History entry, `--scene recording
--artifact ID` opens the recording editor on a video or GIF, and `--scene preview`
restores the two newest entries (`--count N` for more, or `--artifact ID`) as floating previews,
`--scene feedback` opens Send Feedback (a fixture whose Send stays off unless
`--feedback-endpoint URL` is given; loopback HTTP is allowed for tests, and
`https://captur.es/api/feedback` sends real feedback), and
`--find QUERY` opens Preferences Find with a typed query. Open With:
`--open-media PATH` (repeatable) or `-- FILE...` imports each file into
History through the shared `OpenMedia` request and opens its editor; without
`--scene` no Preferences window opens, and a file Captures cannot open reports
the shared reason in a "Captures" error dialog.

`--live` runs Captures as the resident app instead of one fixture window.
Before GTK starts, it takes the shared `captures_app::instance` election for
its History root (`--history-root`, else the native Preview profile, else the
default): a second launch for the same profile hands its files (or a bare
relaunch) to the running one and exits without touching settings or opening
a window. The resident process keeps running when its last window closes and
quits on SIGINT/SIGTERM (or `--quit-after`), releasing the profile. At launch
it shows what shipping does: Preferences, nothing when opening files, and
nothing on a quiet `--captures-autostart` launch. A bare relaunch focuses an
open editor, then History, then Preferences, or opens Preferences.
The live app shows shipping's tray menu as a StatusNotifierItem
(`tray-icon` with `ksni`, as the wgpu host, so no GTK 3 in the process):
Screenshot Display, Capture History…, Open Save Location, Preferences, Send
Feedback… and Quit work; the other capture rows stay in shipping's order but
are disabled until the GTK host has those captures, and Check for Updates…
is enabled only in a Preview build with update metadata (it opens
Preferences). Without a StatusNotifier host the app opens Preferences and
quits when its last window closes. Once setup is finished, the saved global
shortcuts are live through the shared `CaptureShortcuts`: X11 keys, or on
Wayland the GlobalShortcuts portal (no X11 fallback); Screenshot Display
runs, the other actions wait for their captures. The tray accelerators and
the shortcuts follow the saved settings file. GTK Preferences does not yet
show the portal's returned bindings or its Configure/Retry controls.
`--capture display` takes a Screenshot Display once the live app is ready
(the tray and shortcuts call the same path): the shared `CaptureFlow` and
`CaptureDisplay` on X11, for the display under the pointer with Escape
cancelling, or the desktop portal's Screenshot on Wayland with every Captures
window unmapped. The preview stack leaves the screen first unless Preferences
includes it in captures. A saved capture joins History and the preview stack,
and auto-copy uses `arboard` (wlr-data-control on Wayland, which needs no
input serial), as shipping does. The configured countdown delays the capture,
but has no on-screen countdown yet. Region and window selection, recording and
the launch notice are not in the GTK host yet. A fresh profile opens
first-run setup (`--scene setup` renders it as a fixture): shipping's
`Onboarding.tsx` with the shared `captures_app::onboarding` copy, state and
completion. Until it finishes, the tray shows setup and no global shortcut
is live. The elected live process keeps the shared local crash diagnostics
(`captures_app::crash`): a session that did not quit cleanly makes the next
visible launch open Send Feedback with a review card (Copy summary, Add to
message, Dismiss diagnostics). Nothing is added or sent unasked, and Quit or
SIGTERM marks a clean exit. `python3 apps/native/instance_smoke.py --binary
apps/native/gtk/target/debug/captures-gtk --output DIR` runs the shared
forwarding check against it.

`python3 apps/native/history_fixture.py DIR --count 1000`
writes a disposable 1000-entry History (`--video` adds one real FFmpeg
recording and prints its id). `--exercise` and `--quit-after SECONDS`
emit the same `ready` and `scripted-action` JSON lines as the wgpu workbench
for `profile.py`-style measurement. Needs `libgtk-4-dev` (GTK 4.6 or newer).
The default `x11` feature places the preview stack on X11 (work-area corner,
keep-above, every workspace); `--no-default-features` builds without it. The
opt-in `layer-shell` feature makes the stack a wlr-layer-shell surface on
compositors that offer the protocol (wlroots compositors such as Sway, and
KDE): the same corner, gaps and carry as X11, above windows on every
workspace. Elsewhere, GNOME included, the compositor places the stack as
before. It links gtk4-layer-shell 1.x: `apps/native/gtk/build_layer_shell.sh`
builds the pinned 1.3.0 into a private prefix and prints its pkg-config
directory, so `PKG_CONFIG_PATH=$(apps/native/gtk/build_layer_shell.sh) cargo
+1.95.0 build --manifest-path apps/native/gtk/Cargo.toml --features
layer-shell`, then run with `LD_LIBRARY_PATH` set to that prefix's `lib`.
Preferences' Updates card checks only the native Preview channel, and only in
a build with `CAPTURES_NATIVE_PREVIEW_VERSION` and
`CAPTURES_NATIVE_PREVIEW_PUBLIC_KEY` set; development builds never make an
update request, and the GTK host never installs.

The host uses GTK's default renderer. On a machine whose GL is software-only
or broken, `GSK_RENDERER=cairo` is a manual override; the host never switches
renderers itself.

Hardware checks waiting for the owner's Linux PC: measure the GSK renderers
(GL/NGL and cairo) on real GPUs with the resource-gate journeys, Orca, and
the physical X11, GNOME Wayland and KDE Wayland runs.

History is one virtualized `GtkListView` of card rows. GtkListView keeps up
to 200 rows alive before GTK 4.12, so only rows near the viewport hold cards
(from a shared pool) and thumbnail textures, decoded off the UI thread at card
size from `preview.png`, never the full capture.

`smoke.py` checks every window it opens for unnamed controls and focusable generic nodes, and drives real widgets over AT-SPI (History filter and two-step
Delete with 1000 entries, Preferences (a switch's role, checked state and
save, Interface theme, accent chips, a custom accent, the preview corner, a
select and the shortcut recorder), and the editor opened from History: an xdotool drag draws a rectangle,
then Undo, Redo and Save, the Text tool types and commits a label, the Eraser's slider, check box and Wand click (one undo step), shape and text Properties (shadow, opacity drag as one undo step, fill, Bold, alignment), a drawing default fill reaching a new rectangle, Add images through the GTK chooser, a staged 1 : 1 Crop with Clear, Apply crop and Undo, the Canvas toolbar (width, Trim edges, transparent background), Export settings (Compress, 50%, JPEG saved and measured) Change… save location, layer rename plus drag reorder with the real pointer, the ⋯ layer settings card (opacity, Send to back, Duplicate, Flatten image), and the Compress before/after comparison (split, Hide, Show); Restore into previews: Copy, Close and Delete; Send Feedback as a fixture and against a loopback service (keyboard entry, one send, the cooldown); Open With (a PNG and an MP4 open their editors, a text file is refused); `--live` (a quiet launch stays windowless, a forwarded relaunch opens Preferences, closing it keeps the app running, a forwarded PNG opens its editor, SIGTERM releases the profile); live Screenshot Display (X11 reads the screen; on Wayland a Screenshot portal fixture answers while no Captures window is mapped; the capture reaches History, the preview stack and the clipboard; an unlocked ScreenSaver fixture satisfies the shared session check); the tray (a StatusNotifierWatcher fixture; shipping's 15 rows in order with unsupported captures disabled, and real dbusmenu clicks for Capture History…, Screenshot Display on X11 and Quit); first-run setup (a fresh profile opens setup with Linux's ready screen access; Start capturing saves completion and turns the shortcuts on); crash review (a SIGKILLed live session opens Send Feedback with the review on relaunch, Add to message appends the summary, Dismiss clears it, and clean quits leave nothing); global shortcuts (on X11 the saved Display chord captures, and after a settings change the new chord captures and the old one does not; on Wayland the portal owner starts); and
the recording editor: keyboard trim, play/pause and Save as new, then the Compress comparison, crop with the crop handle sliders, GIF format and a saved 320 × 180 GIF). Run it with `xvfb-run -a dbus-run-session -- /usr/bin/python3
apps/native/gtk/smoke.py --binary apps/native/gtk/target/debug/captures-gtk`.
`--only STEP` (repeatable) runs single steps. `--wayland --injector PATH` runs the same smoke on a private headless Sway
instead (the `sway` from `apps/native/build_wayland_compositor_fixture.sh` on
`PATH`, and `apps/native/wayland_drag_probe` built for its virtual pointer and
keyboard; no X display). It needs `python3-gi`, `gir1.2-atspi-2.0`, `gir1.2-gdkpixbuf-2.0`, `xdotool`,
`at-spi2-core` and `ffmpeg`. The recording editor uses `CAPTURES_FFMPEG` and
`CAPTURES_FFPROBE`, then the desktop app's sidecars, then `ffmpeg` on `PATH`.
