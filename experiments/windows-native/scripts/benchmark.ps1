$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
Push-Location $root
try {
  cargo bench --manifest-path experiments/windows-native/Cargo.toml --bench state
  Measure-Command { cargo build --manifest-path experiments/windows-native/Cargo.toml --release --target x86_64-pc-windows-msvc } |
    Select-Object TotalMilliseconds
} finally { Pop-Location }
