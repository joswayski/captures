# Captures GTK4 host (Linux)

The Linux replacement for the egui/wgpu workbench: GTK4 for windows, text,
input and AT-SPI, with Captures' own token-drawn styling and controls. See
[native-rewrite.md](../../../docs/native-rewrite.md#host-direction) for the
direction and the gates it must pass. Only Preferences exists so far.

```sh
node apps/native/prepare.mjs --output apps/native/gtk/resources
cargo +1.95.0 run --manifest-path apps/native/gtk/Cargo.toml -- \
  --settings-file /tmp/captures-gtk/settings.json
```

`--appearance light|dark`, `--size WxH` and `--screenshot out.png` render one
frame and exit, for parity comparisons. Needs `libgtk-4-dev` (GTK 4.6 or newer).
