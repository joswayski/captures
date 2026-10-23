$ErrorActionPreference = 'Stop'

# Match tauri-cli-v2.11.4's bundler: standard Unicode NSIS (1024-unit strings)
# and the same RunAsUser plugin. Never use an arbitrary installed NSIS build.
$scratch = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $scratch | Out-Null
$previous = $env:CAPTURES_TEST_MAKENSIS
try {
    $archive = Join-Path $scratch 'nsis.zip'
    Invoke-WebRequest 'https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.11/nsis-3.11.zip' -OutFile $archive
    if ((Get-FileHash $archive -Algorithm SHA1).Hash -ne 'EF7FF767E5CBD9EDD22ADD3A32C9B8F4500BB10D') {
        throw 'NSIS archive does not match the release bundler pin'
    }
    Expand-Archive $archive -DestinationPath $scratch
    $nsis = Join-Path $scratch 'nsis-3.11'
    $pluginDirectory = Join-Path $nsis 'Plugins/x86-unicode/additional'
    New-Item -ItemType Directory -Path $pluginDirectory -Force | Out-Null
    $plugin = Join-Path $pluginDirectory 'nsis_tauri_utils.dll'
    Invoke-WebRequest 'https://github.com/tauri-apps/nsis-tauri-utils/releases/download/nsis_tauri_utils-v0.5.3/nsis_tauri_utils.dll' -OutFile $plugin
    if ((Get-FileHash $plugin -Algorithm SHA1).Hash -ne '75197FEE3C6A814FE035788D1C34EAD39349B860') {
        throw 'RunAsUser plugin does not match the release bundler pin'
    }
    $env:CAPTURES_TEST_MAKENSIS = Join-Path $nsis 'makensis.exe'
    cargo test -p captures-app windows_nsis_preserves_restart_session_until_matching_acknowledgement -- --ignored --nocapture
    if ($LASTEXITCODE -ne 0) { throw 'Windows update session transport test failed' }
} finally {
    $env:CAPTURES_TEST_MAKENSIS = $previous
    Remove-Item -Recurse -Force $scratch
}
