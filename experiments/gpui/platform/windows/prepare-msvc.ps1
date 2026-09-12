param(
  [string]$BuildRoot = 'C:\gtk-build'
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$prefix = Join-Path $BuildRoot 'gtk\x64\release'
$cairoPc = Join-Path $prefix 'lib\pkgconfig\cairo.pc'
if (-not (Test-Path $cairoPc)) {
  $installed = python -m pip show gvsbuild 2>$null
  if (-not ($installed -match '^Version: 2026\.8\.0$')) {
    throw 'Install the pinned MSVC builder first: python -m pip install gvsbuild==2026.8.0'
  }
  gvsbuild build --build-dir $BuildRoot --platform x64 --configuration release cairo
}

$fxc = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin" -Filter fxc.exe -Recurse -File |
  Where-Object { $_.Directory.Name -eq 'x64' } |
  Sort-Object { [version]$_.Directory.Parent.Name } -Descending |
  Select-Object -First 1
if (-not $fxc) { throw 'No x64 fxc.exe was found in the installed Windows SDK.' }

$pkgconf = @('pkgconf.exe', 'pkg-config.exe') |
  ForEach-Object { Join-Path $prefix "bin\$_" } |
  Where-Object { Test-Path $_ } |
  Select-Object -First 1
if (-not $pkgconf) { throw "gvsbuild did not provide pkgconf in $prefix\bin" }
if (-not (Test-Path (Join-Path $prefix 'lib\cairo.lib'))) { throw 'MSVC cairo.lib is missing.' }
if (-not (Get-ChildItem (Join-Path $prefix 'bin') -Filter 'cairo*.dll' -File)) { throw 'MSVC Cairo DLL is missing.' }

$values = @{
  GPUI_FXC_PATH = $fxc.FullName
  PKG_CONFIG = $pkgconf
  PKG_CONFIG_PATH = (Join-Path $prefix 'lib\pkgconfig')
  CAPTURES_GPUI_MSVC_PREFIX = $prefix
}
foreach ($entry in $values.GetEnumerator()) {
  Set-Item "Env:$($entry.Key)" $entry.Value
  if ($env:GITHUB_ENV) { "$($entry.Key)=$($entry.Value)" | Out-File $env:GITHUB_ENV -Append -Encoding utf8 }
}

& $pkgconf --atleast-version=1.16 cairo
if ($LASTEXITCODE -ne 0) { throw 'pkgconf could not resolve Cairo from the gvsbuild MSVC prefix.' }
Write-Host "Prepared MSVC Cairo at $prefix"
Write-Host "GPUI_FXC_PATH=$($fxc.FullName)"
