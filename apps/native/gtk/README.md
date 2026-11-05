# Captures GTK4 host (Linux)

The Linux replacement for the egui/wgpu workbench: GTK4 for windows, text,
input and AT-SPI, with Captures' own token-drawn styling and controls. See
[native-rewrite.md](../../../docs/native-rewrite.md#host-direction) for the
direction and the gates it must pass. Preferences (General and Capture
cards) and Capture History exist so far.

```sh
node apps/native/prepare.mjs --output apps/native/gtk/resources
cargo +1.95.0 run --manifest-path apps/native/gtk/Cargo.toml -- \
  --settings-file /tmp/captures-gtk/settings.json
```

`--appearance light|dark`, `--size WxH` and `--screenshot out.png` render one
frame and exit, for parity comparisons. `--scene history --history-root DIR`
opens Capture History; `python3 apps/native/history_fixture.py DIR --count 1000`
writes a disposable 1000-entry History. `--exercise` and `--quit-after SECONDS`
emit the same `ready` and `scripted-action` JSON lines as the wgpu workbench
for `profile.py`-style measurement. Needs `libgtk-4-dev` (GTK 4.6 or newer).

History is one virtualized `GtkListView` of card rows. GtkListView keeps up
to 200 rows alive before GTK 4.12, so only rows near the viewport hold cards
(from a shared pool) and thumbnail textures, decoded off the UI thread at card
size from `preview.png`, never the full capture.

`smoke.py` drives real widgets over AT-SPI (History filter and two-step
Delete with 1000 entries, a Preferences switch's role, checked state and
save). Run it with `xvfb-run -a dbus-run-session -- /usr/bin/python3
apps/native/gtk/smoke.py --binary apps/native/gtk/target/debug/captures-gtk`;
it needs `python3-gi`, `gir1.2-atspi-2.0` and `at-spi2-core`.
