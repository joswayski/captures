$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
Push-Location $root
try {
  cargo fmt --manifest-path experiments/windows-native/Cargo.toml -- --check
  if ($LASTEXITCODE -ne 0) { throw "cargo fmt failed with exit code $LASTEXITCODE" }
  cargo test --manifest-path experiments/windows-native/Cargo.toml
  if ($LASTEXITCODE -ne 0) { throw "cargo test failed with exit code $LASTEXITCODE" }
  cargo clippy --manifest-path experiments/windows-native/Cargo.toml --all-targets -- -D warnings
  if ($LASTEXITCODE -ne 0) { throw "cargo clippy failed with exit code $LASTEXITCODE" }
  cargo check --manifest-path experiments/windows-native/Cargo.toml --target x86_64-pc-windows-msvc
  if ($LASTEXITCODE -ne 0) { throw "MSVC cargo check failed with exit code $LASTEXITCODE" }
} finally { Pop-Location }
