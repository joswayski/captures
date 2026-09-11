# Windows native experiment tooling

These scripts package and measure the experimental GTK3 + Cairo native UI. They do not produce an official Captures release.

## Build a portable directory

Use an **MSYS2 MINGW64** shell with Rust 1.94 and the GNU target. Install the
runtime and build dependencies first; the build script never installs packages:

```bash
pacman -S --needed mingw-w64-x86_64-toolchain mingw-w64-x86_64-gtk3 \
  mingw-w64-x86_64-librsvg mingw-w64-x86_64-ffmpeg mingw-w64-x86_64-python \
  mingw-w64-x86_64-pkgconf mingw-w64-x86_64-nasm
rustup target add x86_64-pc-windows-gnu --toolchain 1.94.0
```

```bash
cd experiments/native-ui
./windows/build.sh
# optional output directory:
./windows/build.sh /c/path/to/captures-native-portable
```

The locked release build uses `--no-default-features --features native --bin captures-windows-native`.
Extract the entire portable folder, quit the shipping Captures app to avoid shortcut
conflicts, and launch `bin/captures-windows-native.exe` (or `Captures.cmd`). Do not
move the executable away from its DLLs. The executable configures the relocated
GTK paths and a private temporary pixbuf loader cache before GTK starts, including
when launched at login. It cleans up that cache on normal exit.

The package requires and includes FFmpeg, ffprobe, ffplay, all recursively imported
non-system DLLs, SVG/image loader plugins and their DLL dependencies, GTK schemas,
icons and font configuration. Output directories must not already exist; failed
packages are retained for inspection. `files.json` records every packaged file's
SHA-256 and size. Windows system DLLs are never copied.

This is an unsigned **private-test folder**, not an installer or official release.
The prefix's available license texts are copied under `share/licenses`; this is
not a complete redistribution/source-offer audit. Review all dependency licenses,
MSYS2 package source recipes and corresponding-source obligations before public
distribution, especially for FFmpeg. No packaged updater or file associations are
registered. Runtime dependency inspection is not Windows hardware verification.

The app uses `%LOCALAPPDATA%\captures-windows-native`, or an absolute
`CAPTURES_NATIVE_DATA` override, rather than shipping Tauri settings/history.
Use `--capture`, `--record`, `--gif`, `--preferences`, `--history`, `--canvas`,
`--open "C:\path\image.png"`, or `--previews "C:\path\image.png"`.
Closing a window leaves the tray app running; **Quit Captures** exits it.
Test on copies: the image editor's **Save** overwrites the opened source file,
whereas export creates a new file. The [parity report](../../../docs/windows-native-implementation.md)
lists functionality inherited from Linux and remaining replacement blockers.

## Compare production Tauri and native binaries

**Use a dedicated disposable Windows account or disposable VM. Never run this benchmark in an account containing a real Captures installation or data.** PowerShell environment overrides do not isolate the shipping Tauri app: on Windows, `directories::ProjectDirs("io", "github", "captures")` obtains Windows Known Folders directly. Shipping settings and local data are therefore under:

- `%APPDATA%\github\captures\config\settings.json`
- `%LOCALAPPDATA%\github\captures\data`

Tauri/WebView also uses a Local AppData profile based on the app identifier (`io.github.joswayski.captures`). The benchmark refuses to run if any shipping config/data directory, identifier profile, conventional executable-adjacent WebView2 profile, or Captures process already exists. It does not rename or modify the Tauri binary.

Run with **PowerShell 7 (`pwsh`)** on the same otherwise-idle Windows machine. `-DisposableAccount` is a mandatory acknowledgement:

```powershell
Set-ExecutionPolicy -Scope Process Bypass
pwsh experiments/native-ui/windows/benchmark.ps1 `
  -DisposableAccount `
  -TauriExecutable 'C:\builds\Captures.exe' `
  -NativeExecutable 'C:\builds\native\bin\captures-windows-native.exe' `
  -EditorInput 'C:\fixtures\editor input.png' `
  -Trials 10 -Output '.\windows-benchmark.json'
```

The portable folder includes `benchmark.ps1` at its root; use that path instead
of `experiments/native-ui/windows/benchmark.ps1` when testing without a checkout.

Supply executable paths (`.exe`), not shortcuts or wrappers. Portable native
runtime configuration is automatic. Native storage alone is explicitly redirected
with `CAPTURES_NATIVE_DATA`. Both apps receive light-appearance fixtures, the same
temporary output directory, and the same editor input; the shipping fixture marks
onboarding complete. The script creates Tauri fixtures only at the real KnownFolder
paths after proving they are absent, and removes only the exact newly created app
directories. Unknown or pre-existing paths are never cleaned up. Profiles are reused
between trials within this run: these are warm-launch tests, not fresh-profile or
cold-boot measurements. Do not start other applications while it runs.

Preferences client areas are set to 980×720 and editor client areas to 1280×760. The script locates visible top-level windows with `EnumWindows`, the launched root PID, and the expected title; it does not depend on `MainWindowTitle` or repeatedly query WMI while timing. Startup timing means **observed visible target window**, not UI readiness or completed rendering.

After settling, each trial captures the visible desktop region occupied by the window client area. This avoids unreliable `PrintWindow` WebView captures, but focus denial, overlap, notifications, or other occlusion can contaminate a PNG; inspect the images. JSON is updated after every trial and retains raw successful samples, failures, executable/input/PNG SHA-256 hashes, and process-tree CPU/memory including WebView children. A failed trial is recorded but excluded from benchmark samples. Existing output is refused unless `-Force` is explicit. OS caches are not flushed and background activity remains a confounder.
