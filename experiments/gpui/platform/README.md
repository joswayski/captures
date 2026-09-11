# GPUI native build checks

These helpers build the experimental `captures-gpui` application directly on each target operating system. They are for private testing and CI verification: they do not install, sign for distribution, publish, update, or create a release.

The checks deliberately distinguish three levels of confidence:

1. `cargo test` and a release build verify Rust and native dependency linkage.
2. The package helpers verify the private-test layout and platform metadata.
3. Linux CI starts a real GPUI window under Xvfb and waits for X11 to report it. macOS and Windows hosted runners only exercise `--help`; that proves the process starts, but **does not** prove that a window rendered or that capture permissions work.

The application invokes `ffmpeg`, `ffprobe`, and `ffplay` at runtime. The helpers fail early unless all three are on `PATH`; they do not redistribute Homebrew, apt, Chocolatey, or other third-party builds by default. Set `CAPTURES_GPUI_MEDIA_DIR` to a reviewed set of all three executables to include them in a package. Linux and Windows place them beside the app executable; macOS places them in `Contents/Resources/bin`. These layouts match the app's native resolver and no helper mutates the app process's `PATH`.

The optional media directory must contain self-contained/static builds: the helpers copy only those three executables, not their dependent DLLs or dylibs. Do not use package-manager binaries for downloadable bundles unless their runtime dependencies are separately collected, reviewed, and signed where required. CI intentionally leaves media tools unbundled.

## Linux x64

Install the GPUI X11/Wayland, Cairo, audio, PipeWire, Vulkan, FFmpeg, Xvfb, and X11 test dependencies listed in `.github/workflows/gpui.yml`, then run:

```sh
bash experiments/gpui/platform/linux/build-package.sh
xvfb-run -a bash experiments/gpui/platform/linux/smoke-render.sh experiments/gpui/platform/dist/linux-x86_64/captures-gpui
```

The package is a tarball containing the dynamically linked executable. It expects compatible system libraries and media tools on the test machine unless `CAPTURES_GPUI_MEDIA_DIR` is supplied; it is not an installer.

## macOS 13+

Use full Xcode, not Command Line Tools alone. GPUI 0.2.2 invokes both `xcrun -sdk macosx metal` and `xcrun -sdk macosx metallib`. Cairo must be visible through `pkg-config`, and the shared macOS recording backend requires Swift from Xcode with `MACOSX_DEPLOYMENT_TARGET=13.0`.

```sh
brew install cairo ffmpeg pkg-config
bash experiments/gpui/platform/macos/build-package.sh aarch64-apple-darwin
# On an Intel Mac:
bash experiments/gpui/platform/macos/build-package.sh x86_64-apple-darwin
```

The result is an ad-hoc-signed `Captures GPUI Experiment.app` zip with the stable, isolated bundle identifier `io.github.joswayski.captures.gpui-experiment`. Its usage strings cover screen capture, system audio, and microphone access, which the experiment actually uses. It intentionally has no camera usage key. The Rust executable is the bundle executable; no shell launcher rewrites `PATH`.

The current app accepts media paths through its command line and `--open`, but it does not yet handle macOS `kAEOpenDocuments` events. Consequently the plist does **not** advertise document types: adding associations now would make Finder's Open With UI promise routing that does not exist. Add associations only alongside a tested native open-event adapter.

## Windows x64 MSVC

Run from a Visual Studio developer PowerShell with the Windows SDK installed:

```powershell
python -m pip install gvsbuild==2026.8.0
./experiments/gpui/platform/windows/prepare-msvc.ps1
./experiments/gpui/platform/windows/build-package.ps1
```

`prepare-msvc.ps1` builds Cairo and its dependencies with Visual Studio via gvsbuild when they are not cached, exports its `.pc`, `.lib`, and DLL prefix, discovers the installed Windows SDK's x64 `fxc.exe`, and sets `GPUI_FXC_PATH`. GPUI 0.2.2 needs FXC for release HLSL compilation; its default `windows-manifest` feature embeds PerMonitorV2 DPI awareness.

The package helper rejects GNU runtime imports and copies only the DLLs from the gvsbuild **MSVC** prefix. Never put MSYS2/MinGW Cairo DLLs beside the MSVC binary. The zip is a private-test portable directory, not an installer.

## Upstream references

- [GPUI 0.2.2 published build script](https://docs.rs/crate/gpui/0.2.2/source/build.rs)
- [GPUI 0.2.2 crate metadata](https://crates.io/crates/gpui/0.2.2)
- [gvsbuild 2026.8.0](https://github.com/wingtk/gvsbuild/tree/2026.8.0)
- [Microsoft: FXC is supplied in Windows SDK architecture directories](https://learn.microsoft.com/en-us/windows/win32/directx-sdk--august-2009-)
