param(
  [Parameter(Mandatory=$true)][string]$ImagePath,
  [Parameter(Mandatory=$true)][string]$VideoPath
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
$exe = Join-Path $root "experiments/windows-native/target/x86_64-pc-windows-msvc/release/captures-windows-native.exe"
$out = Join-Path $root "experiments/windows-native/runtime-screenshots"
if (!(Test-Path $exe)) { & "$PSScriptRoot/build.ps1" }
if (!(Test-Path $exe)) { throw "Windows executable was not produced at $exe" }

if (!(Test-Path $ImagePath)) { throw "Shared image fixture does not exist: $ImagePath" }
if (!(Test-Path $VideoPath)) { throw "Shared video fixture does not exist: $VideoPath" }
$ImagePath = (Resolve-Path $ImagePath).Path
$VideoPath = (Resolve-Path $VideoPath).Path

$profile = Join-Path ([IO.Path]::GetTempPath()) ("captures-windows-fixtures-" + [guid]::NewGuid())
$videoFrame = Join-Path $profile "video-frame.png"
New-Item -ItemType Directory -Force $out, $profile | Out-Null
$driverLog = Join-Path $out "render-drivers.txt"
Set-Content $driverLog "appearance,view,d3d_driver"

ffmpeg -v error -y -ss 0.2 -i $VideoPath -frames:v 1 $videoFrame
if ($LASTEXITCODE -ne 0 -or !(Test-Path $videoFrame)) {
  throw "ffmpeg could not extract a frame from the shared video fixture"
}

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class CapturesFixtureNative {
  public delegate bool EnumCallback(IntPtr hwnd, IntPtr state);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumCallback callback, IntPtr state);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr hwnd, StringBuilder name, int length);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
  public struct RECT { public int Left, Top, Right, Bottom; }

  public static IntPtr FindWindowForProcess(uint expectedPid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((hwnd, state) => {
      uint pid;
      GetWindowThreadProcessId(hwnd, out pid);
      var name = new StringBuilder(128);
      GetClassName(hwnd, name, name.Capacity);
      if (pid == expectedPid && IsWindowVisible(hwnd) && name.ToString() == "CapturesWindowsNativeWindow") {
        found = hwnd;
        return false;
      }
      return true;
    }, IntPtr.Zero);
    return found;
  }
}
"@

function Assert-NonBlank([Drawing.Bitmap]$bitmap, [string]$view) {
  if ($bitmap.Width -lt 40 -or $bitmap.Height -lt 40) { throw "$view rendered an implausibly small window" }
  $colors = [Collections.Generic.HashSet[int]]::new()
  for ($y = 0; $y -lt $bitmap.Height; $y += [Math]::Max(1, [int]($bitmap.Height / 12))) {
    for ($x = 0; $x -lt $bitmap.Width; $x += [Math]::Max(1, [int]($bitmap.Width / 12))) {
      [void]$colors.Add($bitmap.GetPixel($x, $y).ToArgb())
    }
  }
  if ($colors.Count -lt 4) { throw "$view capture is blank or nearly uniform ($($colors.Count) sampled colors)" }
}

function Save-View([string]$appearance, [string]$view) {
  $source = if ($view -eq "recording-editor") { $videoFrame } else { $ImagePath }
  Remove-Item (Join-Path $profile "render-driver.txt") -Force -ErrorAction SilentlyContinue
  # Start-Process flattens ArgumentList, so explicitly quote paths that may contain spaces.
  $arguments = @("--view", $view, "--appearance", $appearance, "--fixture-image", ('"{0}"' -f $source), "--fixture-video", ('"{0}"' -f $VideoPath))
  $process = Start-Process $exe -ArgumentList $arguments -PassThru
  try {
    $handle = [IntPtr]::Zero
    for ($attempt = 0; $attempt -lt 300 -and $handle -eq [IntPtr]::Zero; $attempt++) {
      if ($process.HasExited) { throw "$view exited before creating a window (exit $($process.ExitCode))" }
      Start-Sleep -Milliseconds 100
      $handle = [CapturesFixtureNative]::FindWindowForProcess([uint32]$process.Id)
    }
    if ($handle -eq [IntPtr]::Zero) { throw "PID $($process.Id) did not create the Captures window for $view" }
    $driverFile = Join-Path $profile "render-driver.txt"
    if (!(Test-Path $driverFile)) { throw "$view did not report its D3D driver" }
    $driver = (Get-Content $driverFile -Raw).Trim()
    if ($driver -notin @("hardware", "warp")) { throw "$view reported unknown D3D driver '$driver'" }
    Add-Content $driverLog "$appearance,$view,$driver"
    Write-Host "$appearance-$view D3D driver: $driver"
    Start-Sleep -Milliseconds 350
    $rect = New-Object CapturesFixtureNative+RECT
    if (![CapturesFixtureNative]::GetWindowRect($handle, [ref]$rect)) { throw "GetWindowRect failed for $view" }
    $bitmap = New-Object Drawing.Bitmap ($rect.Right-$rect.Left), ($rect.Bottom-$rect.Top)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
      # This intentionally throws on hosted Windows sessions without a capturable desktop.
      $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
    } finally { $graphics.Dispose() }
    Assert-NonBlank $bitmap "$appearance-$view"
    $path = Join-Path $out "$appearance-$view.png"
    $bitmap.Save($path, [Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()
    Write-Host $path
  } finally {
    Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
  }
}

$previousData = $env:CAPTURES_WINDOWS_NATIVE_DATA
try {
  $env:CAPTURES_WINDOWS_NATIVE_DATA = $profile
  foreach ($appearance in @("light", "dark")) {
    foreach ($view in @("menu", "editor", "editor-shapes", "editor-export", "recording-selector", "recording-hud", "recording-editor", "preview", "history", "preferences", "delete-confirmation")) {
      Save-View $appearance $view
    }
  }
} finally {
  $env:CAPTURES_WINDOWS_NATIVE_DATA = $previousData
  Remove-Item -Recurse -Force $profile -ErrorAction SilentlyContinue
}
