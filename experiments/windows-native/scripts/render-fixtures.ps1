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
$boundsLog = Join-Path $out "capture-bounds.csv"
Set-Content $boundsLog "appearance,view,virtual_bounds,monitor_bounds,window_bounds"

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
  [DllImport("user32.dll")] static extern IntPtr MonitorFromWindow(IntPtr hwnd, uint flags);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern bool GetMonitorInfo(IntPtr monitor, ref MONITORINFO info);
  [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr hwnd, IntPtr insertAfter, int x, int y, int width, int height, uint flags);
  [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern bool EnumDisplaySettings(string device, int mode, ref DEVMODE settings);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int ChangeDisplaySettings(ref DEVMODE settings, uint flags);
  [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
  public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)]
  struct MONITORINFO { public int Size; public RECT Monitor; public RECT Work; public uint Flags; }
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
  struct DEVMODE {
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string DeviceName;
    public ushort SpecVersion, DriverVersion, Size, DriverExtra;
    public uint Fields;
    public int PositionX, PositionY;
    public uint DisplayOrientation, DisplayFixedOutput;
    public short Color, Duplex, YResolution, TTOption, Collate;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string FormName;
    public ushort LogPixels;
    public uint BitsPerPel, PelsWidth, PelsHeight, DisplayFlags, DisplayFrequency;
    public uint ICMMethod, ICMIntent, MediaType, DitherType, Reserved1, Reserved2;
    public uint PanningWidth, PanningHeight;
  }
  static DEVMODE originalMode;
  static bool changedMode;
  static IntPtr previousDpiContext;
  static bool changedDpiContext;

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

  public static string EnsureCaptureDisplay(int minimumWidth, int minimumHeight) {
    originalMode = new DEVMODE();
    originalMode.Size = (ushort)Marshal.SizeOf(typeof(DEVMODE));
    if (!EnumDisplaySettings(null, -1, ref originalMode)) throw new InvalidOperationException("EnumDisplaySettings could not read the current mode");
    if (originalMode.PelsWidth >= minimumWidth && originalMode.PelsHeight >= minimumHeight) {
      return originalMode.PelsWidth + "x" + originalMode.PelsHeight + " (unchanged)";
    }
    DEVMODE best = new DEVMODE();
    ulong bestArea = ulong.MaxValue;
    for (int index = 0; ; index++) {
      DEVMODE candidate = new DEVMODE();
      candidate.Size = (ushort)Marshal.SizeOf(typeof(DEVMODE));
      if (!EnumDisplaySettings(null, index, ref candidate)) break;
      if (candidate.PelsWidth < minimumWidth || candidate.PelsHeight < minimumHeight) continue;
      ulong area = (ulong)candidate.PelsWidth * candidate.PelsHeight;
      if (area < bestArea) { best = candidate; bestArea = area; }
    }
    if (bestArea == ulong.MaxValue) {
      return originalMode.PelsWidth + "x" + originalMode.PelsHeight + " (no larger mode exposed)";
    }
    // Temporary for this process/session: never update the user's persisted display mode.
    int result = ChangeDisplaySettings(ref best, 0);
    if (result != 0) throw new InvalidOperationException("ChangeDisplaySettings failed with code " + result);
    changedMode = true;
    return best.PelsWidth + "x" + best.PelsHeight + " (temporary)";
  }

  public static void EnterPhysicalDpiContext() {
    // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2. This keeps HWND, monitor,
    // virtual-screen, and GDI capture coordinates in the same physical space.
    previousDpiContext = SetThreadDpiAwarenessContext(new IntPtr(-4));
    if (previousDpiContext == IntPtr.Zero) throw new InvalidOperationException("SetThreadDpiAwarenessContext(PER_MONITOR_AWARE_V2) failed");
    changedDpiContext = true;
  }

  public static void RestoreDpiContext() {
    if (changedDpiContext) SetThreadDpiAwarenessContext(previousDpiContext);
    changedDpiContext = false;
  }

  public static void RestoreDisplay() {
    if (changedMode) ChangeDisplaySettings(ref originalMode, 0);
    changedMode = false;
  }

  public static string PositionAndValidateWindow(IntPtr hwnd) {
    RECT window;
    if (!GetWindowRect(hwnd, out window)) throw new InvalidOperationException("GetWindowRect failed");
    int width = window.Right - window.Left, height = window.Bottom - window.Top;
    IntPtr monitor = MonitorFromWindow(hwnd, 2); // MONITOR_DEFAULTTONEAREST
    MONITORINFO info = new MONITORINFO();
    info.Size = Marshal.SizeOf(typeof(MONITORINFO));
    if (monitor == IntPtr.Zero || !GetMonitorInfo(monitor, ref info)) throw new InvalidOperationException("GetMonitorInfo failed");
    int monitorWidth = info.Monitor.Right - info.Monitor.Left;
    int monitorHeight = info.Monitor.Bottom - info.Monitor.Top;
    if (width > monitorWidth || height > monitorHeight) {
      throw new InvalidOperationException("window " + width + "x" + height + " exceeds capturable monitor " + monitorWidth + "x" + monitorHeight);
    }
    int x = info.Monitor.Left + (monitorWidth - width) / 2;
    int y = info.Monitor.Top + (monitorHeight - height) / 2;
    if (!SetWindowPos(hwnd, IntPtr.Zero, x, y, width, height, 0x0014)) throw new InvalidOperationException("SetWindowPos failed");
    if (!GetWindowRect(hwnd, out window)) throw new InvalidOperationException("GetWindowRect failed after placement");
    monitor = MonitorFromWindow(hwnd, 2);
    info = new MONITORINFO(); info.Size = Marshal.SizeOf(typeof(MONITORINFO));
    if (!GetMonitorInfo(monitor, ref info)) throw new InvalidOperationException("GetMonitorInfo failed after placement");
    if (window.Left < info.Monitor.Left || window.Top < info.Monitor.Top || window.Right > info.Monitor.Right || window.Bottom > info.Monitor.Bottom) {
      throw new InvalidOperationException("physical window rectangle is not fully inside its monitor after placement");
    }
    int virtualLeft = GetSystemMetrics(76), virtualTop = GetSystemMetrics(77);
    int virtualRight = virtualLeft + GetSystemMetrics(78), virtualBottom = virtualTop + GetSystemMetrics(79);
    if (window.Left < virtualLeft || window.Top < virtualTop || window.Right > virtualRight || window.Bottom > virtualBottom) {
      throw new InvalidOperationException("physical window rectangle is not fully inside the capturable virtual desktop");
    }
    return string.Format("\"{0}:{1}:{2}:{3}\",\"{4}:{5}:{6}:{7}\",\"{8}:{9}:{10}:{11}\"",
      virtualLeft, virtualTop, virtualRight, virtualBottom,
      info.Monitor.Left, info.Monitor.Top, info.Monitor.Right, info.Monitor.Bottom,
      window.Left, window.Top, window.Right, window.Bottom);
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
    $bounds = [CapturesFixtureNative]::PositionAndValidateWindow($handle)
    Add-Content $boundsLog "$appearance,$view,$bounds"
    Start-Sleep -Milliseconds 350
    # Revalidate after composition settles; never capture an off-screen partial window.
    [void][CapturesFixtureNative]::PositionAndValidateWindow($handle)
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
  # Protect both temporary thread DPI state and the first display-mode side effect.
  [CapturesFixtureNative]::EnterPhysicalDpiContext()
  $displayMode = [CapturesFixtureNative]::EnsureCaptureDisplay(1600, 1000)
  Write-Host "Fixture display mode: $displayMode"
  Start-Sleep -Milliseconds 500
  $env:CAPTURES_WINDOWS_NATIVE_DATA = $profile
  foreach ($appearance in @("light", "dark")) {
    foreach ($view in @("menu", "editor", "editor-shapes", "editor-export", "recording-selector", "recording-hud", "recording-editor", "preview", "history", "preferences", "delete-confirmation")) {
      Save-View $appearance $view
    }
  }
} finally {
  $env:CAPTURES_WINDOWS_NATIVE_DATA = $previousData
  Remove-Item -Recurse -Force $profile -ErrorAction SilentlyContinue
  [CapturesFixtureNative]::RestoreDisplay()
  [CapturesFixtureNative]::RestoreDpiContext()
}
