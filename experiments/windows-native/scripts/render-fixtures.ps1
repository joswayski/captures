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
  [DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr hwnd);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern bool PostMessage(IntPtr hwnd, uint message, IntPtr wparam, IntPtr lparam);
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

  static IntPtr LogicalPoint(IntPtr hwnd, int x, int y) {
    uint dpi = GetDpiForWindow(hwnd);
    int physicalX = (int)Math.Round(x * dpi / 96.0);
    int physicalY = (int)Math.Round(y * dpi / 96.0);
    return new IntPtr((physicalY << 16) | (physicalX & 0xffff));
  }

  public static void ClickLogical(IntPtr hwnd, int x, int y) {
    IntPtr point = LogicalPoint(hwnd, x, y);
    if (!PostMessage(hwnd, 0x0201, new IntPtr(1), point) || !PostMessage(hwnd, 0x0202, IntPtr.Zero, point)) {
      throw new InvalidOperationException("PostMessage could not deliver a fixture click");
    }
  }

  public static void DragLogical(IntPtr hwnd, int fromX, int fromY, int toX, int toY) {
    if (!PostMessage(hwnd, 0x0201, new IntPtr(1), LogicalPoint(hwnd, fromX, fromY))) {
      throw new InvalidOperationException("PostMessage could not begin a fixture drag");
    }
    for (int step = 1; step <= 8; step++) {
      int x = fromX + (toX - fromX) * step / 8;
      int y = fromY + (toY - fromY) * step / 8;
      if (!PostMessage(hwnd, 0x0200, new IntPtr(1), LogicalPoint(hwnd, x, y))) {
        throw new InvalidOperationException("PostMessage could not continue a fixture drag");
      }
    }
    if (!PostMessage(hwnd, 0x0202, IntPtr.Zero, LogicalPoint(hwnd, toX, toY))) {
      throw new InvalidOperationException("PostMessage could not finish a fixture drag");
    }
  }

  public static void TypeAndCommit(IntPtr hwnd, string value) {
    foreach (char unit in value) {
      if (!PostMessage(hwnd, 0x0102, new IntPtr(unit), IntPtr.Zero)) {
        throw new InvalidOperationException("PostMessage could not deliver fixture text");
      }
    }
    if (!PostMessage(hwnd, 0x0100, new IntPtr(0x0d), IntPtr.Zero)) {
      throw new InvalidOperationException("PostMessage could not commit fixture text");
    }
  }

  public static void CloseWindow(IntPtr hwnd) {
    if (!PostMessage(hwnd, 0x0010, IntPtr.Zero, IntPtr.Zero)) {
      throw new InvalidOperationException("PostMessage could not deliver WM_CLOSE");
    }
  }
}
"@

function Assert-NonBlank([Drawing.Bitmap]$bitmap, [string]$view) {
  if ($bitmap.Width -lt 40 -or $bitmap.Height -lt 40) { throw "$view rendered an implausibly small window" }
  $colors = [Collections.Generic.HashSet[int]]::new()
  # A sparse grid can miss the recording preview and text while landing only
  # on its three large neutral surfaces. Sample densely enough to distinguish
  # real route content from a blank compositor frame.
  for ($y = 0; $y -lt $bitmap.Height; $y += [Math]::Max(1, [int]($bitmap.Height / 40))) {
    for ($x = 0; $x -lt $bitmap.Width; $x += [Math]::Max(1, [int]($bitmap.Width / 40))) {
      [void]$colors.Add($bitmap.GetPixel($x, $y).ToArgb())
    }
  }
  if ($colors.Count -lt 4) { throw "$view capture is blank or nearly uniform ($($colors.Count) sampled colors)" }
}

function Save-View([string]$appearance, [string]$view, [string]$artifactView = $view, [bool]$inputSmoke = $false, [bool]$eraserInputSmoke = $false, [bool]$trimInputSmoke = $false, [bool]$mergeInputSmoke = $false, [bool]$sourceConsumedInputSmoke = $false, [bool]$sourceOnlyFlattenSmoke = $false) {
  $source = if ($view -eq "recording-editor") { $videoFrame } else { $ImagePath }
  Remove-Item (Join-Path $profile "render-driver.txt") -Force -ErrorAction SilentlyContinue
  $recordingReadyFile = Join-Path $profile "recording-frame-presented.txt"
  Remove-Item $recordingReadyFile -Force -ErrorAction SilentlyContinue
  $eraserAppliedFile = Join-Path $profile "editor-eraser-applied.txt"
  Remove-Item $eraserAppliedFile -Force -ErrorAction SilentlyContinue
  $trimAppliedFile = Join-Path $profile "editor-trim-applied.txt"
  Remove-Item $trimAppliedFile -Force -ErrorAction SilentlyContinue
  $mergeAppliedFile = Join-Path $profile "editor-merge-applied.txt"
  Remove-Item $mergeAppliedFile -Force -ErrorAction SilentlyContinue
  $sourceConsumedInputFile = Join-Path $profile "editor-source-consumed-input.txt"
  Remove-Item $sourceConsumedInputFile -Force -ErrorAction SilentlyContinue
  $sourceOnlyFlattenFile = Join-Path $profile "editor-flatten-source-applied.txt"
  Remove-Item $sourceOnlyFlattenFile -Force -ErrorAction SilentlyContinue
  # Start-Process flattens ArgumentList, so explicitly quote paths that may contain spaces.
  $arguments = @("--view", $view, "--appearance", $appearance, "--fixture-image", ('"{0}"' -f $source), "--fixture-video", ('"{0}"' -f $VideoPath))
  $process = Start-Process $exe -ArgumentList $arguments -PassThru
  $viewFailure = $null
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
    Add-Content $driverLog "$appearance,$artifactView,$driver"
    Write-Host "$appearance-$artifactView D3D driver: $driver"
    $bounds = [CapturesFixtureNative]::PositionAndValidateWindow($handle)
    Add-Content $boundsLog "$appearance,$artifactView,$bounds"
    $recordingReady = $true
    if ($view -eq "recording-editor") {
      $recordingReady = $false
      for ($attempt = 0; $attempt -lt 300 -and !$recordingReady; $attempt++) {
        if ($process.HasExited) { throw "$view exited while waiting for decoded source content (exit $($process.ExitCode))" }
        $recordingReady = Test-Path $recordingReadyFile
        if (!$recordingReady) { Start-Sleep -Milliseconds 100 }
      }
      if ($recordingReady) {
        Write-Host "$appearance-$view decoded paused frame presented; playback timing is not exercised"
      }
    } else {
      Start-Sleep -Milliseconds 350
    }
    if ($inputSmoke) {
      # Exercise the real HWND routes rather than preparing the resulting Frame:
      # replace the canvas width through WM_CHAR, choose Star through two clicks,
      # and draw an asymmetric shape through the native pointer path.
      [CapturesFixtureNative]::ClickLogical($handle, 110, 26)
      [CapturesFixtureNative]::TypeAndCommit($handle, "777")
      [CapturesFixtureNative]::ClickLogical($handle, 28, 232)
      [CapturesFixtureNative]::ClickLogical($handle, 296, 311)
      [CapturesFixtureNative]::DragLogical($handle, 250, 190, 430, 330)
      Start-Sleep -Milliseconds 1000
    }
    if ($eraserInputSmoke) {
      # Select Eraser, choose its Erase mode, and paint through the real rail,
      # property, capture, pointer-move, and pointer-up routes.
      [CapturesFixtureNative]::ClickLogical($handle, 28, 396)
      [CapturesFixtureNative]::ClickLogical($handle, 930, 220)
      [CapturesFixtureNative]::DragLogical($handle, 350, 250, 550, 380)
      for ($attempt = 0; $attempt -lt 100 -and !(Test-Path $eraserAppliedFile); $attempt++) {
        if ($process.HasExited) { throw "$view exited before applying the eraser input smoke" }
        Start-Sleep -Milliseconds 50
      }
      if (!(Test-Path $eraserAppliedFile) -or (Get-Content $eraserAppliedFile -Raw).Trim() -ne "erase:alpha-changed") {
        throw "$appearance-$artifactView did not select Erase and change image alpha pixels"
      }
      Start-Sleep -Milliseconds 500
    }
    if ($trimInputSmoke) {
      # Hide the locked original layer, then invoke Trim edges through its real
      # source-eye and toolbar hit targets. The marker includes resulting size.
      [CapturesFixtureNative]::ClickLogical($handle, 1018, 180)
      [CapturesFixtureNative]::ClickLogical($handle, 290, 26)
      for ($attempt = 0; $attempt -lt 100 -and !(Test-Path $trimAppliedFile); $attempt++) {
        if ($process.HasExited) { throw "$view exited before applying the trim input smoke" }
        Start-Sleep -Milliseconds 50
      }
      if (!(Test-Path $trimAppliedFile) -or (Get-Content $trimAppliedFile -Raw).Trim() -ne "trim:421x261") {
        throw "$appearance-$artifactView did not hide the source and trim to the imported layer's 421x261 bounds"
      }
      Start-Sleep -Milliseconds 500
    }
    if ($mergeInputSmoke) {
      # Invoke the already-open Combine menu's Merge down action through the
      # real HWND route. The app marker verifies that the pair became one image.
      [CapturesFixtureNative]::ClickLogical($handle, 900, 155)
      for ($attempt = 0; $attempt -lt 100 -and !(Test-Path $mergeAppliedFile); $attempt++) {
        if ($process.HasExited) { throw "$view exited before applying the merge input smoke" }
        Start-Sleep -Milliseconds 50
      }
      if (!(Test-Path $mergeAppliedFile) -or (Get-Content $mergeAppliedFile -Raw).Trim() -ne "merge-down:one-image-layer") {
        throw "$appearance-$artifactView did not merge the selected pair into one image layer"
      }
      Start-Sleep -Milliseconds 500
    }
    if ($sourceConsumedInputSmoke) {
      # Merge visible consumed the source row before launch. Click the displayed
      # opacity decrement using the shared source-aware inspector geometry.
      [CapturesFixtureNative]::ClickLogical($handle, 990, 280)
      for ($attempt = 0; $attempt -lt 100 -and !(Test-Path $sourceConsumedInputFile); $attempt++) {
        if ($process.HasExited) { throw "$view exited before applying the source-consumed property input smoke" }
        Start-Sleep -Milliseconds 50
      }
      if (!(Test-Path $sourceConsumedInputFile) -or (Get-Content $sourceConsumedInputFile -Raw).Trim() -ne "opacity:229") {
        throw "$appearance-$artifactView did not route the source-consumed inspector click to opacity"
      }
      Start-Sleep -Milliseconds 500
    }
    if ($sourceOnlyFlattenSmoke) {
      # No annotation is selected: open Combine from the source+background
      # document, then invoke its eligible Flatten image action.
      [CapturesFixtureNative]::ClickLogical($handle, 1030, 82)
      [CapturesFixtureNative]::ClickLogical($handle, 900, 220)
      for ($attempt = 0; $attempt -lt 100 -and !(Test-Path $sourceOnlyFlattenFile); $attempt++) {
        if ($process.HasExited) { throw "$view exited before applying source-only flatten" }
        Start-Sleep -Milliseconds 50
      }
      if (!(Test-Path $sourceOnlyFlattenFile) -or (Get-Content $sourceOnlyFlattenFile -Raw).Trim() -ne "flatten:source-only-background") {
        throw "$appearance-$artifactView could not flatten an eligible source+background without a selected annotation"
      }
      Start-Sleep -Milliseconds 500
    }
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
    $path = Join-Path $out "$appearance-$artifactView.png"
    try {
      # Persist diagnostics before assertions so every captured failure remains reviewable.
      $bitmap.Save($path, [Drawing.Imaging.ImageFormat]::Png)
      Assert-NonBlank $bitmap "$appearance-$view"
      if (!$recordingReady) {
        throw "$appearance-$view did not present a decoded source frame within 30 seconds; saved diagnostic $path"
      }
    } finally { $bitmap.Dispose() }
    Write-Host $path
  } catch {
    $viewFailure = $_
  } finally {
    try {
      if (!$process.HasExited) {
        try {
          Stop-Process -Id $process.Id -Force -ErrorAction Stop
        } catch {
          # Exiting between HasExited and Stop-Process is already success.
          if (!$process.HasExited) { throw }
        }
      }
      if (!$process.WaitForExit(10000)) {
        throw "$appearance-$artifactView process $($process.Id) did not terminate within 10 seconds"
      }
    } catch {
      if ($null -eq $viewFailure) {
        $viewFailure = $_
      } else {
        Write-Error "$appearance-$artifactView also failed during process teardown: $($_.Exception.Message)" -ErrorAction Continue
      }
    }
  }
  if ($null -ne $viewFailure) {
    throw $viewFailure
  }
}

function Wait-FixtureWindow([Diagnostics.Process]$process, [string]$phase) {
  $handle = [IntPtr]::Zero
  for ($attempt = 0; $attempt -lt 300 -and $handle -eq [IntPtr]::Zero; $attempt++) {
    if ($process.HasExited) { throw "draft $phase exited before creating a window (exit $($process.ExitCode))" }
    Start-Sleep -Milliseconds 100
    $handle = [CapturesFixtureNative]::FindWindowForProcess([uint32]$process.Id)
  }
  if ($handle -eq [IntPtr]::Zero) { throw "draft $phase PID $($process.Id) did not create the Captures window" }
  return $handle
}

function Wait-FixtureMarker([Diagnostics.Process]$process, [string]$path, [string]$expected, [string]$phase) {
  for ($attempt = 0; $attempt -lt 200 -and !(Test-Path $path); $attempt++) {
    if ($process.HasExited) { throw "draft $phase exited before writing $(Split-Path -Leaf $path) (exit $($process.ExitCode))" }
    Start-Sleep -Milliseconds 50
  }
  if (!(Test-Path $path)) { throw "draft $phase did not write $(Split-Path -Leaf $path)" }
  $actual = (Get-Content $path -Raw).Trim()
  if ($actual -ne $expected) { throw "draft $phase marker was '$actual', expected '$expected'" }
}

function Close-DraftFixture([Diagnostics.Process]$process, [IntPtr]$handle, [string]$marker, [string]$phase) {
  [CapturesFixtureNative]::CloseWindow($handle)
  Wait-FixtureMarker $process $marker "flush:exact" $phase
  if (!$process.WaitForExit(10000)) {
    throw "draft $phase process $($process.Id) did not terminate within 10 seconds after WM_CLOSE"
  }
  if ($process.ExitCode -ne 0) { throw "draft $phase process exited $($process.ExitCode)" }
}

function Save-DraftRestartFixture() {
  $source = Join-Path $profile "draft-source-sentinel.png"
  Copy-Item $ImagePath $source
  $sourceHash = (Get-FileHash $source -Algorithm SHA256).Hash
  $editedMarker = Join-Path $profile "editor-draft-edited.txt"
  $createFlushMarker = Join-Path $profile "editor-draft-create-flushed.txt"
  $restoredMarker = Join-Path $profile "editor-draft-restored.txt"
  $restoreFlushMarker = Join-Path $profile "editor-draft-restore-flushed.txt"
  Remove-Item $editedMarker, $createFlushMarker, $restoredMarker, $restoreFlushMarker -Force -ErrorAction SilentlyContinue

  $createArguments = @("--view", "editor-draft-create", "--appearance", "light", "--fixture-image", ('"{0}"' -f $source), "--fixture-video", ('"{0}"' -f $VideoPath))
  $create = Start-Process $exe -ArgumentList $createArguments -PassThru
  try {
    $createHandle = Wait-FixtureWindow $create "create"
    [void][CapturesFixtureNative]::PositionAndValidateWindow($createHandle)
    [CapturesFixtureNative]::ClickLogical($createHandle, 28, 232)
    [CapturesFixtureNative]::ClickLogical($createHandle, 296, 311)
    [CapturesFixtureNative]::DragLogical($createHandle, 250, 190, 430, 330)
    Wait-FixtureMarker $create $editedMarker "edit:polygon-10" "create edit"
    Close-DraftFixture $create $createHandle $createFlushMarker "create close"
  } finally {
    if (!$create.HasExited) { Stop-Process -Id $create.Id -Force }
  }
  if ((Get-FileHash $source -Algorithm SHA256).Hash -ne $sourceHash) {
    throw "draft create/close rewrote the source sentinel"
  }

  Remove-Item (Join-Path $profile "render-driver.txt") -Force -ErrorAction SilentlyContinue
  $restoreArguments = @("--view", "editor-draft-restore", "--appearance", "light", "--fixture-image", ('"{0}"' -f $source), "--fixture-video", ('"{0}"' -f $VideoPath))
  $restore = Start-Process $exe -ArgumentList $restoreArguments -PassThru
  try {
    $restoreHandle = Wait-FixtureWindow $restore "restore"
    Wait-FixtureMarker $restore $restoredMarker "restore:polygon-10:editable-undo" "restore"
    $bounds = [CapturesFixtureNative]::PositionAndValidateWindow($restoreHandle)
    Add-Content $boundsLog "light,editor-draft-restored,$bounds"
    $driver = (Get-Content (Join-Path $profile "render-driver.txt") -Raw).Trim()
    if ($driver -notin @("hardware", "warp")) { throw "draft restore reported unknown D3D driver '$driver'" }
    Add-Content $driverLog "light,editor-draft-restored,$driver"
    Start-Sleep -Milliseconds 350
    $rect = New-Object CapturesFixtureNative+RECT
    if (![CapturesFixtureNative]::GetWindowRect($restoreHandle, [ref]$rect)) { throw "GetWindowRect failed for restored draft" }
    $bitmap = New-Object Drawing.Bitmap ($rect.Right-$rect.Left), ($rect.Bottom-$rect.Top)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
      $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
    } finally { $graphics.Dispose() }
    try {
      $path = Join-Path $out "light-editor-draft-restored.png"
      $bitmap.Save($path, [Drawing.Imaging.ImageFormat]::Png)
      Assert-NonBlank $bitmap "editor-draft-restored"
      Write-Host $path
    } finally { $bitmap.Dispose() }
    Close-DraftFixture $restore $restoreHandle $restoreFlushMarker "restore close"
  } finally {
    if (!$restore.HasExited) { Stop-Process -Id $restore.Id -Force }
  }
  if ((Get-FileHash $source -Algorithm SHA256).Hash -ne $sourceHash) {
    throw "draft restore/edit/close rewrote the source sentinel"
  }
  Write-Host "draft restart fixture restored editable polygon and preserved source SHA256 $sourceHash"
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
    foreach ($view in @("menu", "editor", "editor-image", "editor-shapes", "editor-export", "editor-properties", "editor-line", "editor-merge", "editor-merge-visible", "editor-flatten-source", "editor-eraser", "editor-trim", "recording-selector", "recording-hud", "recording-editor", "preview", "history", "preferences", "preferences-capture", "preferences-recording", "preferences-appearance", "feedback", "delete-confirmation")) {
      Save-View $appearance $view
    }
    Save-View $appearance "editor" "editor-input-smoke" $true
    Save-View $appearance "editor" "editor-eraser-input-smoke" $false $true
    Save-View $appearance "editor-trim" "editor-trim-input-smoke" $false $false $true
    Save-View $appearance "editor-merge" "editor-merge-input-smoke" $false $false $false $true
    Save-View $appearance "editor-merge-visible" "editor-source-consumed-input-smoke" $false $false $false $false $true
    Save-View $appearance "editor-flatten-source" "editor-source-only-flatten-smoke" $false $false $false $false $false $true
  }
  Save-DraftRestartFixture
} finally {
  $env:CAPTURES_WINDOWS_NATIVE_DATA = $previousData
  Remove-Item -Recurse -Force $profile -ErrorAction SilentlyContinue
  [CapturesFixtureNative]::RestoreDisplay()
  [CapturesFixtureNative]::RestoreDpiContext()
}
