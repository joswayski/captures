[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,
    [string]$OutputDirectory = 'native-dist'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$output = if ([IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory
} else {
    Join-Path $root $OutputDirectory
}
$binary = Join-Path $root 'experiments/windows-native/target/x86_64-pc-windows-msvc/release/captures-windows-native.exe'
$ffmpeg = Join-Path $root 'apps/desktop/src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe'
$ffprobe = Join-Path $root 'apps/desktop/src-tauri/binaries/ffprobe-x86_64-pc-windows-msvc.exe'
$compliance = Join-Path $root 'target/ffmpeg-dist'
$noticeDirectory = Join-Path $root 'apps/desktop/src-tauri/ffmpeg'
$installer = Join-Path $output 'Captures-Windows-x64-setup.exe'
$makensis = Get-Command makensis -ErrorAction SilentlyContinue

if (-not $makensis) { throw 'makensis was not found. Install NSIS (for example: choco install nsis).' }
foreach ($path in @($binary, $ffmpeg, $ffprobe, $compliance, $noticeDirectory)) {
    if (-not (Test-Path -LiteralPath $path)) { throw "Required packaging input is missing: $path" }
}
$complianceFiles = @(Get-ChildItem -LiteralPath $compliance -File)
if ($complianceFiles.Count -eq 0) { throw "FFmpeg compliance distribution is empty: $compliance" }
foreach ($pattern in @('*.tar.xz', '*.tar.xz.asc', '*-BUILD_CONFIG.txt', '*-COPYING.LGPLv2.1', '*-NOTICE.md')) {
    if (-not ($complianceFiles | Where-Object Name -Like $pattern)) {
        throw "Required FFmpeg source/compliance input is missing: $pattern"
    }
}

New-Item -ItemType Directory -Force -Path $output | Out-Null
& $makensis.Source /V2 "/DPRODUCT_VERSION=$Version" "/DOUTPUT_FILE=$installer" `
    "/DAPP_BINARY=$binary" "/DFFMPEG_BINARY=$ffmpeg" "/DFFPROBE_BINARY=$ffprobe" `
    "/DFFMPEG_NOTICES=$noticeDirectory" "/DFFMPEG_DIST=$compliance" "/DREPO_ROOT=$root" `
    (Join-Path $PSScriptRoot 'native-windows.nsi')
if ($LASTEXITCODE -ne 0) { throw "makensis failed with exit code $LASTEXITCODE" }
if (-not (Test-Path -LiteralPath $installer)) { throw "Installer was not created: $installer" }
Write-Output $installer
