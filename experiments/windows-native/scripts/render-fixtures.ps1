$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
$exe = Join-Path $root "experiments/windows-native/target/x86_64-pc-windows-msvc/release/captures-windows-native.exe"
$out = Join-Path $root "experiments/windows-native/runtime-screenshots"
if (!(Test-Path $exe)) { & "$PSScriptRoot/build.ps1" }
New-Item -ItemType Directory -Force $out | Out-Null

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class CapturesFixtureNative {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

function Save-View([string]$view) {
  $process = Start-Process $exe -ArgumentList "--view", $view -PassThru
  try {
    $handle = [IntPtr]::Zero
    for ($attempt = 0; $attempt -lt 50 -and $handle -eq [IntPtr]::Zero; $attempt++) {
      Start-Sleep -Milliseconds 100
      $handle = [CapturesFixtureNative]::FindWindow("CapturesWindowsNativeWindow", "Captures")
    }
    if ($handle -eq [IntPtr]::Zero) { throw "Captures window did not appear for $view" }
    [CapturesFixtureNative]::SetForegroundWindow($handle) | Out-Null
    Start-Sleep -Milliseconds 250
    $rect = New-Object CapturesFixtureNative+RECT
    [CapturesFixtureNative]::GetWindowRect($handle, [ref]$rect) | Out-Null
    $bitmap = New-Object Drawing.Bitmap ($rect.Right-$rect.Left), ($rect.Bottom-$rect.Top)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try { $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size) }
    finally { $graphics.Dispose() }
    $path = Join-Path $out "$view.png"
    $bitmap.Save($path, [Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()
    Write-Host $path
  } finally { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
}

@("menu", "editor", "recording-selector", "recording-hud", "recording-editor", "preview", "history", "preferences", "delete-confirmation") |
  ForEach-Object { Save-View $_ }
