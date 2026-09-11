param(
  [string]$Dist,
  [string]$MediaDir = $env:CAPTURES_GPUI_MEDIA_DIR
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$platform = Split-Path $PSScriptRoot -Parent
$experiment = Split-Path $platform -Parent
if (-not $Dist) { $Dist = Join-Path $platform 'dist\windows-x86_64-msvc' }
$target = 'x86_64-pc-windows-msvc'

foreach ($tool in @('ffmpeg', 'ffprobe', 'ffplay')) {
  $found = Get-Command "$tool.exe" -ErrorAction SilentlyContinue
  if (-not $found) { throw "Missing runtime media tool on PATH: $tool.exe" }
  & $found.Source -version | Select-Object -First 1
}
if (-not $env:GPUI_FXC_PATH -or -not (Test-Path $env:GPUI_FXC_PATH)) {
  throw 'Run prepare-msvc.ps1 before building; GPUI 0.2.2 release builds require GPUI_FXC_PATH.'
}
if (-not $env:CAPTURES_GPUI_MSVC_PREFIX) { throw 'Missing CAPTURES_GPUI_MSVC_PREFIX.' }

cargo test --manifest-path (Join-Path $experiment 'Cargo.toml') --locked --release --target $target --no-run
if ($LASTEXITCODE -ne 0) { throw 'cargo test compilation failed' }
# Windows resolves native DLLs beside the test executable, not from pkg-config's
# link-search paths. Keep this scoped to build output rather than rewriting PATH.
$testDirectory = Join-Path $experiment "target\$target\release\deps"
Copy-Item (Join-Path $env:CAPTURES_GPUI_MSVC_PREFIX 'bin\*.dll') $testDirectory
cargo test --manifest-path (Join-Path $experiment 'Cargo.toml') --locked --release --target $target
if ($LASTEXITCODE -ne 0) { throw 'cargo test failed' }
cargo build --manifest-path (Join-Path $experiment 'Cargo.toml') --locked --release --target $target
if ($LASTEXITCODE -ne 0) { throw 'cargo release build failed' }

$binary = Join-Path $experiment "target\$target\release\captures-gpui.exe"
$imports = (& dumpbin.exe /DEPENDENTS $binary | Out-String)
if ($LASTEXITCODE -ne 0) { throw 'dumpbin dependency inspection failed' }
if ($imports -match '(?im)^\s*(libgcc|libstdc\+\+|libwinpthread|msys-|cygwin).*\.dll\s*$') {
  throw 'The MSVC executable imports a MinGW/MSYS runtime DLL.'
}

Remove-Item $Dist -Recurse -Force -ErrorAction SilentlyContinue
New-Item $Dist -ItemType Directory | Out-Null
Copy-Item $binary $Dist
Copy-Item (Join-Path $env:CAPTURES_GPUI_MSVC_PREFIX 'bin\*.dll') $Dist
Copy-Item (Join-Path $platform 'README.md') (Join-Path $Dist 'PRIVATE-TEST-NOTES.md')
if ($MediaDir) {
  foreach ($tool in @('ffmpeg', 'ffprobe', 'ffplay')) {
    $source = Join-Path $MediaDir "$tool.exe"
    if (-not (Test-Path $source)) { throw "Missing bundled media executable: $source" }
    Copy-Item $source $Dist
  }
}

$gnuDll = Get-ChildItem $Dist -Filter '*.dll' | Where-Object { $_.Name -match '^(libgcc|libstdc\+\+|libwinpthread|msys-|cygwin)' }
if ($gnuDll) { throw "Refusing to package non-MSVC runtime DLL: $($gnuDll.Name -join ', ')" }
& (Join-Path $Dist 'captures-gpui.exe') --help | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Packaged CLI startup smoke check failed.' }

$zip = "$Dist.zip"
Remove-Item $zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path "$Dist\*" -DestinationPath $zip
Write-Host "Built private-test MSVC package: $zip"
