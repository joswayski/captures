$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
Push-Location $root
try {
  cargo build --manifest-path experiments/windows-native/Cargo.toml --release --target x86_64-pc-windows-msvc
  if ($LASTEXITCODE -ne 0) { throw "MSVC release build failed with exit code $LASTEXITCODE" }
  Write-Host "Built experiments/windows-native/target/x86_64-pc-windows-msvc/release/captures-windows-native.exe"
} finally { Pop-Location }
