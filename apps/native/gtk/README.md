# Captures GTK4 host (Linux)

The Linux replacement for the egui/wgpu workbench: GTK4 for windows, text,
input and AT-SPI, with Captures' own token-drawn styling and controls. See
[native-rewrite.md](../../../docs/native-rewrite.md#host-direction) for the
direction and the gates it must pass. Preferences (every card, Find, the
compact layout, the microphone list and Preview updates),
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
New Capture…, Screenshot Display, Screenshot Region, Screenshot Window,
Capture History…, Open Save Location, Preferences, Send Feedback… and Quit
work; the recording rows stay in shipping's order but are disabled until the
recording selection/options menu is connected, and Check for Updates…
is enabled only in a Preview build with update metadata (it shows the update notice
or checks, as shipping). Without a StatusNotifier host the app opens Preferences and
quits when its last window closes. Once setup is finished, the saved global
shortcuts are live through the shared `CaptureShortcuts`: X11 keys, or on
Wayland the GlobalShortcuts portal (no X11 fallback); New Capture, Screenshot
Display and the Region and Window screenshots run; recording waits. The tray accelerators and
the shortcuts follow the saved settings file. GTK Preferences does not yet
show the portal's returned bindings or its Configure/Retry controls.
`--capture display` opens New Capture on Full screen once the live app is ready
(the tray and shortcuts call the same path), with Enter or Take screenshot
confirming. X11 selects the display under the pointer; Wayland acquires a
Screenshot portal still with every Captures window unmapped, then persists
that still without another consent request. The preview stack leaves the screen
first unless Preferences includes it in captures. A saved capture joins History and the preview stack,
and auto-copy uses `arboard` (wlr-data-control on Wayland, which needs no
input serial), as shipping does. The configured countdown shows shipping's
remaining seconds, glass scrim and Escape hint on the selected X11 display,
with accessible labels and reduced-motion support. Escape cancels without saving
and restores hidden previews; the countdown leaves the screen before acquisition.
Wayland Window shows it before the portal picker. Already-approved portal stills
and explicit frozen-controls captures skip it. `--capture region` and `--capture window`
open shipping's selectors: on X11 an unmanaged overlay over the frozen display
under the pointer (the shared `RegionSession` / `WindowSession`), with the
glass guidance chip, "Click and drag" feedback on an empty click, Shift for a
square, the accent marquee and its logical-pixel size badge, the hovered
window's ring and title chip, and Escape to cancel. On Wayland Region selects
over the Screenshot portal's still on an Overlay layer surface (the
`layer-shell` feature) or a fullscreen window, and Window goes to the
portal's ScreenCast window picker. `--capture new` (the tray's New Capture…
and its shortcut) opens shipping's New Capture controls in Screenshot mode
over the same overlay: the glass bar with Close, Screenshot/Record (Record
disabled with shipping's "not available" title), Region/Window/Full screen,
the Aspect select (Free, 1 : 1, 4 : 3, 3 : 2, 16 : 9, 9 : 16) or the Display
select, Take screenshot and the "These controls will show in screenshots ·
Press Enter to confirm" note, all from the shared `capture_menu`. Region
draws, moves and corner-resizes a framed selection with handles and a
centred size badge; Window keeps the clicked window until confirmed (and is
disabled when X11 lists no windows); Full screen shows the display identity
card. Enter confirms from the surface, a segment or Take screenshot, but not
from Close or a select; Escape and Close cancel; auto-capture follows
Preferences. While the controls are open, the saved Region, Window and
Display chords (and New Capture's, as Region) switch the target in place
through the shared selector routing. Repeating the current target freezes the
visible controls into a new selection: Display and New Capture reopen the bar;
Region and X11 Window open their direct selector. The explicit frozen capture
skips the countdown, as shipping does. On X11 the controls claim keyboard focus
even when opened over the launch notice. The Display select reopens the
controls on another display while retaining the target. On Wayland Region crops
the portal still, Full screen preserves its exact pixels and Display metadata,
and Window goes to the ScreenCast picker. Repeating a Wayland target remains
portal-mediated; the compositor decides which UI its new still contains.
On X11, Record mode offers Region, Window and Full screen targets with temporary
FPS, resolution, cursor, clicks, desktop-audio and microphone options. Normal
tray and shortcut entry points use the same recording flow. The development-only
`--live --capture record-display` entry point records the display under the pointer
using saved recording settings. The shared serialized worker runs the recording
countdown and opens fixed-glass controls for Stop and
save, Pause/Resume, Restart (with confirmation), a region screenshot beside
the take, microphone mute, Delete (with confirmation), and Hide. New Capture
or a bare relaunch restores hidden controls; Hide is disabled without a tray.
Controls show immediate, fixed-glass tooltips on hover or keyboard focus,
including disabled controls; accessible names retain unavailable reasons.
The last three tooltips align to the right. Transitions honor reduced motion.
Hide shows a click-through notice with the saved New Capture shortcut, then
expires without restoring the HUD.
Accepted takes save on Quit/SIGTERM; pre-start Escape cancels without publishing,
and failed finalization retains recovery media. Saved takes join History and
honor `open_editor_after_recording`. Use a disposable `--history-root` and
`--settings-file` for this development path. Wayland recording is explicitly
refused, including its menu/tray actions; this is not a native release or
completed physical-device parity gate.

A quiet launch into the tray shows shipping's
launch notice for 5 s, and finishing setup shows it for 15 s. Linux tray
hosts never report the icon's position, so it is always the compact
caretless rectangle in the work area's tray corner (`--scene startup`
renders it): an unmanaged notification window on X11, an Overlay layer
surface with the `layer-shell` feature on wlroots/KDE Wayland, and a
compositor-placed window elsewhere. On Wayland it names the tray route
instead of the New Capture keys, as the portal decides the real trigger.
A fresh profile opens
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
Preview updates run only in a build with `CAPTURES_NATIVE_PREVIEW_VERSION` and
`CAPTURES_NATIVE_PREVIEW_PUBLIC_KEY` set; development builds never make an
update request and keep "Updates unavailable". In such a build the resident
app checks the native Preview channel at launch and every five minutes
(`src/updates.rs`, the shared `CheckWorker::preview`), and Preferences'
Updates card shows shipping's states: Check Now, Checking now…, the available
version with its size and Update now, Installing… with progress, Up to date,
Couldn't check, Update failed with Try again, and the captur.es fallback.
Update now verifies the AppImage, then hands the open previews, editors and
Preferences to the shared update helper and quits; the helper replaces the
AppImage and relaunches it, which reopens those windows. A `.deb` install
updates manually: its card offers View release. A newly available version also
opens shipping's floating update notice once (after any capture or recording),
which refreshes in place and runs the same check, download, install and links.
The tray's Check for Updates shows the notice for an update in hand; otherwise
it checks and shows the notice, or says up to date or why the check failed. `--update-card NAME` (`idle`, `checking`,
`current`, `available`, `closing`, `manual`, `downloading`, `verifying`,
`restarting`, `failed`, `check-failed`) renders a fixture card in Preferences.
`--live --native-update-ready-file PATH --native-update-ready-token UUID` is
the helper's relaunch; a development build needs explicit absolute
`--history-root` and `--settings-file` with it. The update notice
(`--scene update --update NAME`, one of `captures_app::update_notice::FIXTURES`)
renders shipping's card from the shared presentation and walks the shared
stub (Update now downloads, restarts and closes; nothing is downloaded or
installed). `--scene update --update-live NAME` (`available`, `closing`,
`manual`, `downloading`, `verifying`, `installing`, `failed`, `check-failed`)
renders the live notice for one worker state, and a development `--live` app
with `--update-script available|retry` runs a scripted update source (no
network or install; installing only quits) for the smoke's
`update_notice_live` step.

`node scripts/native-preview-build.mjs build x86_64-unknown-linux-gnu VERSION
--unsigned` packages this host (release, default `x11` feature, built in
`target/native-gtk-preview`) as the unpublished, experimental Linux AppImage and
`.deb`, with the target-suffixed FFmpeg/FFprobe sidecars and
`native_preview_update_helper`; see
[releases.md](../../../docs/releases.md#native-preview-packaging-and-recovery).
Building needs `libgtk-4-dev` plus the shared capture stack's development
packages (`libpipewire-0.3-dev`, `libclang-dev`, `libdbus-1-dev`,
`libxkbcommon-dev`, X11/XCB/Wayland, ALSA, EGL/GBM). The packaged binary is
`captures` and answers `--preview-build-info` with renderer `gtk`. Such a build
starts `--live` without flags, uses the imported Preview profile, holds the
package-use lock and opens `%U` paths or `file:` URIs from its desktop entry.
It starts the startup recovery companion before importing the profile and
acknowledges readiness once settings, the display and the packaged
FFmpeg/FFprobe are ready. The `.deb` needs `libgtk-4-1`
(GTK 4.6 or newer) installed. The AppImage carries its GTK 4 libraries, but
the Tauri CLI's GTK hook exports `GDK_BACKEND=x11`, so it runs on X11 or
XWayland only.

The host uses GTK's default renderer. On a machine whose GL is software-only
or broken, `GSK_RENDERER=cairo` is a manual override; the host never switches
renderers itself. Under software GL (Mesa llvmpipe, as in Xvfb and CI)
the default renderer draws a dark band beside large `--glass-shadow` boxes
such as the New Capture bar; Cairo and the same GL renderer on zink are
clean, so the shared token stays.

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
then Undo, Redo and Save, the Text tool types and commits a label, the Eraser's slider, check box and Wand click (one undo step), shape and text Properties (shadow, opacity drag as one undo step, fill, Bold, alignment), a drawing default fill reaching a new rectangle, Add images through the GTK chooser, a staged 1 : 1 Crop with Clear, Apply crop and Undo, the Canvas toolbar (width, Trim edges, transparent background), Export settings (Compress, 50%, JPEG saved and measured) Change… save location, layer rename plus drag reorder with the real pointer, the ⋯ layer settings card (opacity, Send to back, Duplicate, Flatten image), and the Compress before/after comparison (split, Hide, Show); Restore into previews: Copy, Close and Delete; Send Feedback as a fixture and against a loopback service (keyboard entry, one send, the cooldown); Open With (a PNG and an MP4 open their editors, a text file is refused); `--live` (a quiet launch stays windowless, a forwarded relaunch opens Preferences, closing it keeps the app running, a forwarded PNG opens its editor, SIGTERM releases the profile); live Screenshot Display (X11 reads the screen; on Wayland a Screenshot portal fixture answers while no Captures window is mapped; the capture reaches History, the preview stack and the clipboard; an unlocked ScreenSaver fixture satisfies the shared session check); the tray (a StatusNotifierWatcher fixture; shipping's 15 rows in order with unsupported captures disabled, and real dbusmenu clicks for Capture History…, Screenshot Display on X11 and Quit); first-run setup (a fresh profile opens setup with Linux's ready screen access; Start capturing saves completion and turns the shortcuts on); the launch notice (a quiet live launch shows it with the New Capture keys on X11 and the tray route on Wayland; Close dismisses it; finishing setup shows it for 15 s); the Updates card fixtures (each state's button, status, progress, error alert and captur.es link); the update notice fixture (Hide persists the notes setting and What's new restores it, pull requests are real buttons, Update now downloads, restarts and closes, an error's Close dismisses it); crash review (a SIGKILLed live session opens Send Feedback with the review on relaunch, Add to message appends the summary, Dismiss clears it, and clean quits leave nothing); Region and Window selectors (guidance, empty-click feedback, Escape, a drag saving exactly the selection, and on X11 the hovered window named and saved; on Wayland a crop of the portal still and the ScreenCast picker's refusal cancelling); New Capture (the bar's controls and states over AT-SPI, Close cancelling, a drawn region saved with Enter, Full screen and on X11 the clicked window saved, the Display and Window chords switching the open bar, Enter on a focused segment, Window disabled without a window list; on Wayland Full screen through the portal and Window refused by the ScreenCast fixture); global shortcuts (on X11 the saved Display chord captures, and after a settings change the new chord captures and the old one does not; on Wayland the portal owner starts); and
the recording editor: keyboard trim, play/pause and Save as new, then the Compress comparison, crop with the crop handle sliders, GIF format and a saved 320 × 180 GIF). Run it with `xvfb-run -a dbus-run-session -- /usr/bin/python3
apps/native/gtk/smoke.py --binary apps/native/gtk/target/debug/captures-gtk`.
`--only STEP` (repeatable) runs single steps. `--wayland --injector PATH` runs the same smoke on a private headless Sway
instead (the `sway` from `apps/native/build_wayland_compositor_fixture.sh` on
`PATH`, and `apps/native/wayland_drag_probe` built for its virtual pointer and
keyboard; no X display). It needs `python3-gi`, `gir1.2-atspi-2.0`, `gir1.2-gdkpixbuf-2.0`, `xdotool`,
`at-spi2-core` and `ffmpeg`, plus ImageMagick's `import` for the X11 controls-pixel comparison.
The `recording_controls` step also uses `hsetroot` and `ffprobe`. Run it on a
private Xvfb display with `-noreset`: it records changing desktop colors,
decodes the saved MP4, checks that paused frames were excluded, and exercises
child cancellation, Hide/restore, confirmations, countdown cancellation and
save-on-Quit. It does not exercise a physical microphone or a Wayland portal.
The recording editor uses `CAPTURES_FFMPEG` and
`CAPTURES_FFPROBE`, then `ffmpeg`/`ffprobe` beside its executable (a package),
then the desktop app's sidecars, then `ffmpeg` on `PATH`.
