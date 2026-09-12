$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
Push-Location $root
try {
  cargo fmt --manifest-path experiments/windows-native/Cargo.toml -- --check
  cargo test --manifest-path experiments/windows-native/Cargo.toml
  cargo clippy --manifest-path experiments/windows-native/Cargo.toml --all-targets -- -D warnings
  cargo check --manifest-path experiments/windows-native/Cargo.toml --target x86_64-pc-windows-msvc
} finally { Pop-Location }
