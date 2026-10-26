# Captures patch to global-hotkey 0.8.0

This is the crates.io 0.8.0 source, manifests, examples, README and licenses.
The root Tauri workspace and the isolated native wgpu workspace both use it.
No dependency version or public API changes are included.

The only source change is in `src/platform_impl/x11/mod.rs`: flush queued
`XUngrabKey` requests after unregistering and after rolling back a failed grab.
The polling event loop does not flush output. Without this fix, Escape remains
grabbed by the X server even after the library reports successful unregister,
until an unrelated later registration flushes the connection.

The same fix is proposed upstream in
https://github.com/tauri-apps/global-hotkey/pull/197 (still unmerged when vendored).
Remove this copy and both Cargo patches when a released version includes it.

Regression check from the repository root, on a disposable X11 display:

```sh
env -u WAYLAND_DISPLAY CAPTURES_TEST_PRIVATE_X11=1 xvfb-run -a \
  cargo test -p captures-app --test escape_hotkey -- --ignored
```

The test uses two independent clients and requires the second client to acquire
Escape after unregister, without any further request from the first client.
It fails against the unmodified crates.io release.
