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
restores the two newest entries (or `--artifact ID`) as floating previews,
`--scene feedback` opens Send Feedback (a fixture whose Send stays off unless
`--feedback-endpoint URL` is given; loopback HTTP is allowed for tests, and
`https://captur.es/api/feedback` sends real feedback), and
`--find QUERY` opens Preferences Find with a typed query; `python3 apps/native/history_fixture.py DIR --count 1000`
writes a disposable 1000-entry History (`--video` adds one real FFmpeg
recording and prints its id). `--exercise` and `--quit-after SECONDS`
emit the same `ready` and `scripted-action` JSON lines as the wgpu workbench
for `profile.py`-style measurement. Needs `libgtk-4-dev` (GTK 4.6 or newer).
The default `x11` feature places the preview stack on X11 (work-area corner,
keep-above, every workspace); `--no-default-features` builds without it, and
Wayland compositors place the stack themselves.
Preferences' Updates card checks only the native Preview channel, and only in
a build with `CAPTURES_NATIVE_PREVIEW_VERSION` and
`CAPTURES_NATIVE_PREVIEW_PUBLIC_KEY` set; development builds never make an
update request, and the GTK host never installs.

History is one virtualized `GtkListView` of card rows. GtkListView keeps up
to 200 rows alive before GTK 4.12, so only rows near the viewport hold cards
(from a shared pool) and thumbnail textures, decoded off the UI thread at card
size from `preview.png`, never the full capture.

`smoke.py` drives real widgets over AT-SPI (History filter and two-step
Delete with 1000 entries, Preferences (a switch's role, checked state and
save, Interface theme, accent chips, a custom accent, the preview corner, a
select and the shortcut recorder), and the editor opened from History: an xdotool drag draws a rectangle,
then Undo, Redo and Save, the Text tool types and commits a label, the Eraser's slider, check box and Wand click (one undo step), shape and text Properties (shadow, opacity drag as one undo step, fill, Bold, alignment), a drawing default fill reaching a new rectangle, Add images through the GTK chooser, the Canvas toolbar (width, Trim edges, transparent background), Export settings (Compress, 50%, JPEG saved and measured) Change… save location, layer rename plus drag reorder with the real pointer, the ⋯ layer settings card (opacity, Send to back, Duplicate, Flatten image), and the Compress before/after comparison (split, Hide, Show); Restore into previews: Copy, Close and Delete; Send Feedback as a fixture and against a loopback service (keyboard entry, one send, the cooldown); and
the recording editor: keyboard trim, play/pause and Save as new, then the Compress comparison, crop, GIF format and a saved 320 × 180 GIF). Run it with `xvfb-run -a dbus-run-session -- /usr/bin/python3
apps/native/gtk/smoke.py --binary apps/native/gtk/target/debug/captures-gtk`;
it needs `python3-gi`, `gir1.2-atspi-2.0`, `gir1.2-gdkpixbuf-2.0`, `xdotool`,
`at-spi2-core` and `ffmpeg`. The recording editor uses `CAPTURES_FFMPEG` and
`CAPTURES_FFPROBE`, then the desktop app's sidecars, then `ffmpeg` on `PATH`.
