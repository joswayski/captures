$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
Push-Location $root
try {
  cargo bench --manifest-path experiments/windows-native/Cargo.toml --bench state
  if ($LASTEXITCODE -ne 0) { throw "state benchmark failed with exit code $LASTEXITCODE" }
  $script:buildExitCode = 0
  $timing = Measure-Command {
    cargo build --manifest-path experiments/windows-native/Cargo.toml --release --target x86_64-pc-windows-msvc
    $script:buildExitCode = $LASTEXITCODE
  }
  if ($script:buildExitCode -ne 0) { throw "timed MSVC build failed with exit code $script:buildExitCode" }
  $timing | Select-Object TotalMilliseconds
} finally { Pop-Location }
