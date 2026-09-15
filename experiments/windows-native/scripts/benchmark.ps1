[CmdletBinding()]
param(
  [Parameter(Mandatory)][ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })][string]$TauriExecutable,
  [Parameter(Mandatory)][ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })][string]$NativeExecutable,
  [Parameter(Mandatory)][ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })][string]$EditorInput,
  [Parameter(Mandatory)][switch]$DisposableAccount,
  [string]$Output = (Join-Path $PWD 'windows-native-benchmark.json'),
  [ValidateRange(2, 100)][int]$Trials = 10,
  [ValidateRange(0, 300)][double]$SettleSeconds = 2,
  [ValidateRange(0.1, 300)][double]$IdleSeconds = 3,
  [ValidateRange(1, 300)][double]$WindowTimeoutSeconds = 30,
  [switch]$StateMicrobenchmark,
  [switch]$Force
)

$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) { throw 'PowerShell 7 or newer is required (ProcessStartInfo.ArgumentList is used).' }
if (-not $IsWindows) { throw 'This process benchmark only runs on Windows; it does not synthesize Windows results on another OS.' }
if (-not $DisposableAccount) { throw 'Acknowledge that this is a dedicated disposable Windows account or VM with -DisposableAccount.' }
. (Join-Path $PSScriptRoot 'benchmark-core.ps1')

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class CapturesBenchmarkWindows {
  public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr parameter);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr parameter);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int count);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hwnd, out RECT rect);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hwnd, ref POINT point);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hwnd, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
  [DllImport("user32.dll")] static extern IntPtr MonitorFromWindow(IntPtr hwnd, uint flags);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern bool GetMonitorInfo(IntPtr monitor, ref MONITORINFO info);
  [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
  [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
  [StructLayout(LayoutKind.Sequential)] struct MONITORINFO { public int Size; public RECT Monitor; public RECT Work; public uint Flags; }
  static IntPtr previousDpiContext;
  static bool changedDpiContext;

  public static void EnterPhysicalDpiContext() {
    // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 keeps HWND, monitor, virtual
    // desktop, client, and GDI screenshot coordinates in physical pixels.
    previousDpiContext = SetThreadDpiAwarenessContext(new IntPtr(-4));
    if (previousDpiContext == IntPtr.Zero) throw new InvalidOperationException("SetThreadDpiAwarenessContext(PER_MONITOR_AWARE_V2) failed");
    changedDpiContext = true;
  }

  public static void RestoreDpiContext() {
    if (changedDpiContext) SetThreadDpiAwarenessContext(previousDpiContext);
    changedDpiContext = false;
  }

  public static RECT PositionAndValidateClient(IntPtr hwnd, int clientWidth, int clientHeight) {
    RECT client, window;
    if (!GetClientRect(hwnd, out client) || !GetWindowRect(hwnd, out window)) throw new InvalidOperationException("Could not inspect target window dimensions");
    int outerWidth = clientWidth + (window.Right - window.Left) - (client.Right - client.Left);
    int outerHeight = clientHeight + (window.Bottom - window.Top) - (client.Bottom - client.Top);
    IntPtr monitor = MonitorFromWindow(hwnd, 2); // MONITOR_DEFAULTTONEAREST
    MONITORINFO info = new MONITORINFO(); info.Size = Marshal.SizeOf(typeof(MONITORINFO));
    if (monitor == IntPtr.Zero || !GetMonitorInfo(monitor, ref info)) throw new InvalidOperationException("GetMonitorInfo failed");
    int monitorWidth = info.Monitor.Right - info.Monitor.Left, monitorHeight = info.Monitor.Bottom - info.Monitor.Top;
    if (outerWidth > monitorWidth || outerHeight > monitorHeight) {
      throw new InvalidOperationException("requested client creates window " + outerWidth + "x" + outerHeight + " which cannot fit monitor " + monitorWidth + "x" + monitorHeight);
    }
    int x = info.Monitor.Left + (monitorWidth - outerWidth) / 2;
    int y = info.Monitor.Top + (monitorHeight - outerHeight) / 2;
    if (!SetWindowPos(hwnd, IntPtr.Zero, x, y, outerWidth, outerHeight, 0x0014)) throw new InvalidOperationException("SetWindowPos failed");
    return GetValidatedClientBounds(hwnd, clientWidth, clientHeight);
  }

  public static RECT GetValidatedClientBounds(IntPtr hwnd, int expectedWidth, int expectedHeight) {
    RECT client, window;
    POINT origin = new POINT();
    if (!GetClientRect(hwnd, out client) || !GetWindowRect(hwnd, out window) || !ClientToScreen(hwnd, ref origin)) throw new InvalidOperationException("Could not inspect target client bounds");
    int width = client.Right - client.Left, height = client.Bottom - client.Top;
    if (width != expectedWidth || height != expectedHeight) throw new InvalidOperationException("target client dimensions changed from the matched size");
    RECT capture = new RECT { Left = origin.X, Top = origin.Y, Right = origin.X + width, Bottom = origin.Y + height };
    IntPtr monitor = MonitorFromWindow(hwnd, 2);
    MONITORINFO info = new MONITORINFO(); info.Size = Marshal.SizeOf(typeof(MONITORINFO));
    if (monitor == IntPtr.Zero || !GetMonitorInfo(monitor, ref info)) throw new InvalidOperationException("GetMonitorInfo failed during capture validation");
    if (window.Left < info.Monitor.Left || window.Top < info.Monitor.Top || window.Right > info.Monitor.Right || window.Bottom > info.Monitor.Bottom ||
        capture.Left < info.Monitor.Left || capture.Top < info.Monitor.Top || capture.Right > info.Monitor.Right || capture.Bottom > info.Monitor.Bottom) {
      throw new InvalidOperationException("physical window or client rectangle is not fully inside its monitor");
    }
    int virtualLeft = GetSystemMetrics(76), virtualTop = GetSystemMetrics(77);
    int virtualRight = virtualLeft + GetSystemMetrics(78), virtualBottom = virtualTop + GetSystemMetrics(79);
    if (window.Left < virtualLeft || window.Top < virtualTop || window.Right > virtualRight || window.Bottom > virtualBottom ||
        capture.Left < virtualLeft || capture.Top < virtualTop || capture.Right > virtualRight || capture.Bottom > virtualBottom) {
      throw new InvalidOperationException("physical window or client rectangle is not fully inside the capturable virtual desktop");
    }
    return capture;
  }
}
'@

function Resolve-FullPath([string]$Path) {
  [IO.Path]::GetFullPath((Resolve-Path -LiteralPath $Path).Path)
}

$binaries = @{ tauri = Resolve-FullPath $TauriExecutable; native = Resolve-FullPath $NativeExecutable }
$inputPath = Resolve-FullPath $EditorInput
$outputPath = [IO.Path]::GetFullPath($Output)
$outputParent = [IO.Path]::GetDirectoryName($outputPath)
$artifactDirectory = Join-Path $outputParent (([IO.Path]::GetFileNameWithoutExtension($outputPath)) + '-artifacts')
if ($binaries.tauri -eq $binaries.native) { throw 'TauriExecutable and NativeExecutable must differ.' }
foreach ($entry in $binaries.GetEnumerator()) {
  if ([IO.Path]::GetExtension($entry.Value) -ne '.exe') { throw "$($entry.Key) must be an .exe, not a shortcut or wrapper: $($entry.Value)" }
}
if ((Test-Path -LiteralPath $outputPath) -and -not $Force) { throw "Output exists; use -Force to replace it: $outputPath" }
if ((Test-Path -LiteralPath $artifactDirectory) -and -not $Force) { throw "Artifact directory exists; use -Force to replace it: $artifactDirectory" }
if ($Force) {
  Remove-Item -LiteralPath $outputPath -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $artifactDirectory -Recurse -Force -ErrorAction SilentlyContinue
}

$roaming = [Environment]::GetFolderPath([Environment+SpecialFolder]::ApplicationData)
$local = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
$knownRoamingRoot = Join-Path $roaming 'github\captures'
$knownLocalRoot = Join-Path $local 'github\captures'
$knownConfigRoot = Join-Path $knownRoamingRoot 'config'
$knownDataRoot = Join-Path $knownLocalRoot 'data'
$tauriAppRoot = Join-Path $local 'io.github.joswayski.captures'
$tauriWebViewCandidates = @(
  $tauriAppRoot,
  ($binaries.tauri + '.WebView2'),
  (Join-Path ([IO.Path]::GetDirectoryName($binaries.tauri)) 'Captures.exe.WebView2')
) | Select-Object -Unique
foreach ($path in @($knownRoamingRoot, $knownLocalRoot) + $tauriWebViewCandidates) {
  if (Test-Path -LiteralPath $path) {
    throw "Refusing to touch an existing shipping config/data or Tauri WebView profile path: $path. Use a new disposable account/VM, not your normal account."
  }
}

function Get-ProcessRows {
  @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, ExecutablePath, Name)
}

function Get-Identity([int]$ProcessId) {
  $process = Get-Process -Id $ProcessId -ErrorAction Stop
  @{ process_id = $ProcessId; start_time_utc_ticks = $process.StartTime.ToUniversalTime().Ticks }
}

function Test-Identity($Identity) {
  try { (Get-Process -Id $Identity.process_id -ErrorAction Stop).StartTime.ToUniversalTime().Ticks -eq $Identity.start_time_utc_ticks } catch { $false }
}

function Get-TreeIds([int]$RootProcessId, $Rows) {
  @(Get-BenchmarkTreeIds -RootProcessId $RootProcessId -Rows @($Rows))
}

function Get-TreeSnapshot([int[]]$ProcessIds, $Rows) {
  $rowById = @{}
  foreach ($row in $Rows) { $rowById[[int]$row.ProcessId] = $row }
  $processes = @($ProcessIds | ForEach-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
  if ($processes.Count -eq 0) { throw 'The owned process tree disappeared during measurement.' }
  $details = @($processes | ForEach-Object {
    $row = $rowById[[int]$_.Id]
    @{
      process_id = [int]$_.Id
      parent_process_id = if ($row) { [int]$row.ParentProcessId } else { $null }
      name = if ($row) { $row.Name } else { $_.ProcessName }
      executable_path = if ($row) { $row.ExecutablePath } else { $null }
      private_bytes = [int64]$_.PrivateMemorySize64
      working_set_bytes = [int64]$_.WorkingSet64
      cpu_seconds = [double]$_.CPU
    }
  })
  @{
    private_bytes = [int64](($details | Measure-Object private_bytes -Sum).Sum)
    working_set_bytes = [int64](($details | Measure-Object working_set_bytes -Sum).Sum)
    cpu_seconds = [double](($details | Measure-Object cpu_seconds -Sum).Sum)
    process_count = $details.Count
    processes = $details
  }
}

function Stop-OwnedTree($RootIdentity) {
  if (-not (Test-Identity $RootIdentity)) { return }
  $ids = @(Get-TreeIds $RootIdentity.process_id (Get-ProcessRows))
  $identities = @($ids | ForEach-Object { try { Get-Identity $_ } catch { } })
  [array]::Reverse($identities)
  foreach ($identity in $identities) {
    if (Test-Identity $identity) { Stop-Process -Id $identity.process_id -Force -ErrorAction SilentlyContinue }
  }
}

function Find-TargetWindow([int]$RootProcessId, [string]$TitlePattern) {
  $script:foundWindow = [IntPtr]::Zero
  $script:foundTitle = $null
  $callback = [CapturesBenchmarkWindows+EnumWindowsProc]{
    param([IntPtr]$handle, [IntPtr]$parameter)
    [uint32]$windowProcessId = 0
    [void][CapturesBenchmarkWindows]::GetWindowThreadProcessId($handle, [ref]$windowProcessId)
    if ($windowProcessId -eq $RootProcessId -and [CapturesBenchmarkWindows]::IsWindowVisible($handle) -and -not [CapturesBenchmarkWindows]::IsIconic($handle)) {
      $text = [Text.StringBuilder]::new(512)
      [void][CapturesBenchmarkWindows]::GetWindowText($handle, $text, $text.Capacity)
      if ($text.ToString() -match $TitlePattern) {
        $script:foundWindow = $handle
        $script:foundTitle = $text.ToString()
        return $false
      }
    }
    $true
  }
  [void][CapturesBenchmarkWindows]::EnumWindows($callback, [IntPtr]::Zero)
  if ($script:foundWindow -ne [IntPtr]::Zero) { @{ handle = $script:foundWindow; title = $script:foundTitle } }
}

function Set-ClientSizeAndPosition([IntPtr]$Handle, [int]$Width, [int]$Height) {
  [void][CapturesBenchmarkWindows]::PositionAndValidateClient($Handle, $Width, $Height)
  $deadline = [DateTime]::UtcNow.AddSeconds(1)
  do {
    try {
      $bounds = [CapturesBenchmarkWindows]::GetValidatedClientBounds($Handle, $Width, $Height)
      return @{ left = $bounds.Left; top = $bounds.Top; right = $bounds.Right; bottom = $bounds.Bottom }
    } catch { }
    Start-Sleep -Milliseconds 10
  } while ([DateTime]::UtcNow -lt $deadline)
  throw 'The window did not retain the matched client size and fully on-screen placement.'
}

function Get-ClientBitmap([IntPtr]$Handle, [int]$Width, [int]$Height) {
  $bounds = [CapturesBenchmarkWindows]::GetValidatedClientBounds($Handle, $Width, $Height)
  $bitmap = [Drawing.Bitmap]::new($Width, $Height)
  $graphics = [Drawing.Graphics]::FromImage($bitmap)
  try { $graphics.CopyFromScreen($bounds.Left, $bounds.Top, 0, 0, [Drawing.Size]::new($Width, $Height)) }
  finally { $graphics.Dispose() }
  $bitmap
}

function Get-BitmapSignature([Drawing.Bitmap]$Bitmap) {
  $sample = [Drawing.Bitmap]::new(16, 12)
  $graphics = [Drawing.Graphics]::FromImage($sample)
  try { $graphics.DrawImage($Bitmap, 0, 0, 16, 12) }
  finally { $graphics.Dispose() }
  try {
    [Collections.Generic.List[int]]$values = @()
    for ($y = 0; $y -lt 12; $y++) {
      for ($x = 0; $x -lt 16; $x++) {
        $pixel = $sample.GetPixel($x, $y)
        $values.Add($pixel.R); $values.Add($pixel.G); $values.Add($pixel.B)
      }
    }
    @($values)
  } finally { $sample.Dispose() }
}

function Wait-StableFrameHeuristic([IntPtr]$Handle, [int]$Width, [int]$Height, [Diagnostics.Stopwatch]$Watch, [DateTime]$Deadline) {
  $previous = $null
  do {
    $bitmap = Get-ClientBitmap $Handle $Width $Height
    try { $current = @(Get-BitmapSignature $bitmap) } finally { $bitmap.Dispose() }
    if ($previous) {
      $state = Test-BenchmarkStableFrameHeuristic -Previous $previous -Current $current
      if ($state.passed) {
        return @{
          milliseconds = $Watch.Elapsed.TotalMilliseconds
          sampled_color_range = $state.sample_range
          mean_absolute_frame_difference = $state.mean_absolute_difference
        }
      }
    }
    $previous = $current
    Start-Sleep -Milliseconds 50
  } while ([DateTime]::UtcNow -lt $Deadline)
  throw 'The target window mapped but did not produce a stable, non-uniform client frame before the timeout.'
}

function Save-WindowScreenshot([IntPtr]$Handle, [int]$Width, [int]$Height, [string]$Path) {
  [void][CapturesBenchmarkWindows]::SetForegroundWindow($Handle)
  Start-Sleep -Milliseconds 250
  $bitmap = Get-ClientBitmap $Handle $Width $Height
  try { $bitmap.Save($Path, [Drawing.Imaging.ImageFormat]::Png) }
  finally { $bitmap.Dispose() }
}

$fixtureOutput = Join-Path ([IO.Path]::GetTempPath()) ('captures-benchmark-output-' + [guid]::NewGuid())
$nativeData = Join-Path ([IO.Path]::GetTempPath()) ('captures-windows-native-benchmark-' + [guid]::NewGuid())
$createdKnownRoots = @()
$script:launchedTauri = $false
$samples = [Collections.Generic.List[object]]::new()
$failures = [Collections.Generic.List[object]]::new()
$scenarios = @('preferences', 'editor')
$apps = @('tauri', 'native')
$script:physicalDpiContextEntered = $false
$environment = @{
  os = [Environment]::OSVersion.VersionString
  machine = $env:PROCESSOR_IDENTIFIER
  logical_processors = [Environment]::ProcessorCount
  powershell = $PSVersionTable.PSVersion.ToString()
  computer_name = $env:COMPUTERNAME
  video_controllers = @(Get-CimInstance Win32_VideoController | ForEach-Object { @{ name = $_.Name; driver_version = $_.DriverVersion } })
}

function Write-Result {
  $outcome = Get-BenchmarkOutcome -Samples @($samples) -Failures @($failures) -Scenarios $scenarios -Apps $apps -ExpectedTrials $Trials
  $result = @{
    recorded_at_utc = [DateTime]::UtcNow.ToString('o')
    executables = @{
      tauri = @{ path = $binaries.tauri; sha256 = (Get-FileHash -LiteralPath $binaries.tauri -Algorithm SHA256).Hash; bytes = (Get-Item -LiteralPath $binaries.tauri).Length }
      native = @{ path = $binaries.native; sha256 = (Get-FileHash -LiteralPath $binaries.native -Algorithm SHA256).Hash; bytes = (Get-Item -LiteralPath $binaries.native).Length }
    }
    editor_input = @{ path = $inputPath; sha256 = (Get-FileHash -LiteralPath $inputPath -Algorithm SHA256).Hash; bytes = (Get-Item -LiteralPath $inputPath).Length }
    benchmark_scripts = @{
      runner_sha256 = (Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash
      core_sha256 = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'benchmark-core.ps1') -Algorithm SHA256).Hash
    }
    environment = $environment
    configuration = @{
      trials = $Trials
      settle_seconds = $SettleSeconds
      idle_seconds = $IdleSeconds
      window_timeout_seconds = $WindowTimeoutSeconds
      excluded_warmups_per_scenario_app = 1
      preferences_client_size = @{ width = 980; height = 720 }
      editor_client_size = @{ width = 1280; height = 760 }
      ordering = 'Tauri first on odd trials, native first on even trials, independently within each scenario'
      profiles = 'one isolated profile per app, reused for warm launches across this run'
      display = 'existing display modes only; windows are centered and must fit fully within one physical monitor and the virtual desktop'
      tauri_arguments = @{ preferences = @(); editor = @('<EditorInput>') }
      native_arguments = @{ preferences = @('--view', 'preferences', '--appearance', 'light'); editor = @('--view', 'editor', '--appearance', 'light', '--fixture-image', '<EditorInput>') }
    }
    caveats = @(
      'first_visible_mapped_ms observes the first visible non-minimized target HWND; it does not claim rendered content.',
      'stable_frame_heuristic_ms is not content-ready latency. It is only the time when, after matched resizing, two 16x12 client-area samples become non-uniform and differ by at most one channel value on average.',
      'A stable wallpaper, empty chrome, error page, or wrong application content can pass the frame heuristic. Screenshots must be inspected; the benchmark never certifies semantic readiness.',
      'Screenshots copy the desktop client-area region after foregrounding; overlap, notifications, focus denial, HDR, and occlusion can affect readiness and PNGs, so inspect every image.',
      'Private bytes and working set are whole process-tree sums, including WebView and FFmpeg children when present; working set can double-count shared pages.',
      'CPU is whole process-tree CPU-time delta as percent of one logical core. A process-tree change during the idle interval invalidates that trial.',
      'Profiles and OS caches are warm after the first launch; caches are not flushed and background load remains a confounder.'
    )
    outcome = $outcome
    summaries = @(Get-BenchmarkSummaries -Samples @($samples))
    samples = @($samples)
    failures = @($failures)
  }
  $result | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $outputPath -Encoding utf8
}

function Run-Trial([string]$App, [string]$Scenario, [int]$Index, [bool]$Warmup) {
  $process = $null
  $rootIdentity = $null
  $window = $null
  try {
    $arguments = if ($App -eq 'tauri') {
      if ($Scenario -eq 'editor') { @($inputPath) } else { @() }
    } elseif ($Scenario -eq 'editor') {
      @('--view', 'editor', '--appearance', 'light', '--fixture-image', $inputPath)
    } else {
      @('--view', 'preferences', '--appearance', 'light')
    }
    $renderDriverPath = Join-Path $nativeData 'render-driver.txt'
    if ($App -eq 'native') { Remove-Item -LiteralPath $renderDriverPath -Force -ErrorAction SilentlyContinue }
    $titlePattern = if ($App -eq 'native') { '^Captures \[[^\]]+\]$' } elseif ($Scenario -eq 'editor') { '^(Captures Screenshot Editor|Captures — Image editor)$' } else { '^Captures Preferences$' }
    $size = if ($Scenario -eq 'editor') { @(1280, 760) } else { @(980, 720) }
    $startInfo = [Diagnostics.ProcessStartInfo]::new($binaries[$App])
    $startInfo.UseShellExecute = $false
    foreach ($argument in $arguments) { [void]$startInfo.ArgumentList.Add($argument) }
    if ($App -eq 'native') { $startInfo.Environment['CAPTURES_WINDOWS_NATIVE_DATA'] = $nativeData }
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $process = [Diagnostics.Process]::Start($startInfo)
    if ($App -eq 'tauri') { $script:launchedTauri = $true }
    $rootIdentity = Get-Identity $process.Id
    $deadline = [DateTime]::UtcNow.AddSeconds($WindowTimeoutSeconds)
    do {
      if ($process.HasExited) { throw "$App exited before showing $Scenario (exit $($process.ExitCode))." }
      $window = Find-TargetWindow $process.Id $titlePattern
      if (-not $window) { Start-Sleep -Milliseconds 10 }
    } while (-not $window -and [DateTime]::UtcNow -lt $deadline)
    if (-not $window) { throw "$App did not show a visible $Scenario target window within $WindowTimeoutSeconds seconds." }
    $mappedMilliseconds = $watch.Elapsed.TotalMilliseconds
    $clientBounds = Set-ClientSizeAndPosition $window.handle $size[0] $size[1]
    $stableFrame = Wait-StableFrameHeuristic $window.handle $size[0] $size[1] $watch $deadline
    $renderDriver = $null
    if ($App -eq 'native') {
      if (-not (Test-Path -LiteralPath $renderDriverPath -PathType Leaf)) { throw 'Native content became visible without the expected DirectComposition render-driver marker.' }
      $renderDriver = (Get-Content -LiteralPath $renderDriverPath -Raw).Trim()
      if ($renderDriver -notin @('hardware', 'warp')) { throw "Native render-driver marker was invalid: $renderDriver" }
    }
    Start-Sleep -Milliseconds ([int]($SettleSeconds * 1000))
    $screenshotName = if ($Warmup) { 'warmup-{0}-{1}.png' -f $Scenario, $App } else { '{0}-{1}-{2:D2}.png' -f $Scenario, $App, $Index }
    $screenshotPath = Join-Path $artifactDirectory $screenshotName
    Save-WindowScreenshot $window.handle $size[0] $size[1] $screenshotPath

    $rowsBefore = Get-ProcessRows
    $treeBefore = @(Get-TreeIds $process.Id $rowsBefore | Sort-Object)
    $identitiesBefore = @($treeBefore | ForEach-Object { Get-Identity $_ })
    $before = Get-TreeSnapshot $treeBefore $rowsBefore
    $cpuWatch = [Diagnostics.Stopwatch]::StartNew()
    Start-Sleep -Milliseconds ([int]($IdleSeconds * 1000))
    $rowsAfter = Get-ProcessRows
    $treeAfter = @(Get-TreeIds $process.Id $rowsAfter | Sort-Object)
    if ((Compare-Object $treeBefore $treeAfter) -or @($identitiesBefore | Where-Object { -not (Test-Identity $_) }).Count) { throw 'Process tree changed during the CPU sample; excluding this trial.' }
    $after = Get-TreeSnapshot $treeAfter $rowsAfter
    $samples.Add(@{
      app = $App
      scenario = $Scenario
      trial = $Index
      warmup = $Warmup
      included_in_medians = -not $Warmup
      arguments = $arguments
      first_visible_mapped_ms = $mappedMilliseconds
      stable_frame_heuristic_ms = $stableFrame.milliseconds
      stable_frame_heuristic_probe = @{ sampled_color_range = $stableFrame.sampled_color_range; mean_absolute_frame_difference = $stableFrame.mean_absolute_frame_difference }
      native_directcomposition_driver = $renderDriver
      window_title = $window.title
      client_width = $size[0]
      client_height = $size[1]
      physical_client_bounds = $clientBounds
      private_bytes = $after.private_bytes
      working_set_bytes = $after.working_set_bytes
      process_count = $after.process_count
      idle_cpu_percent_one_core = 100 * ($after.cpu_seconds - $before.cpu_seconds) / $cpuWatch.Elapsed.TotalSeconds
      process_tree = $after.processes
      screenshot = @{ path = $screenshotPath; sha256 = (Get-FileHash -LiteralPath $screenshotPath -Algorithm SHA256).Hash; bytes = (Get-Item -LiteralPath $screenshotPath).Length }
    })
  } catch {
    $failure = @{
      app = $App
      scenario = $Scenario
      trial = $Index
      warmup = $Warmup
      included_in_medians = $false
      recorded_at_utc = [DateTime]::UtcNow.ToString('o')
      error = $_.Exception.Message
    }
    if ($window) {
      $failurePath = Join-Path $artifactDirectory $(if ($Warmup) { 'failure-warmup-{0}-{1}.png' -f $Scenario, $App } else { 'failure-{0}-{1}-{2:D2}.png' -f $Scenario, $App, $Index })
      try {
        Save-WindowScreenshot $window.handle $size[0] $size[1] $failurePath
        $failure.screenshot = @{ path = $failurePath; sha256 = (Get-FileHash -LiteralPath $failurePath -Algorithm SHA256).Hash; bytes = (Get-Item -LiteralPath $failurePath).Length }
      } catch { $failure.screenshot_error = $_.Exception.Message }
    }
    $failures.Add($failure)
    Write-Warning $failure.error
  } finally {
    if ($rootIdentity) { Stop-OwnedTree $rootIdentity }
  }
}

try {
  [CapturesBenchmarkWindows]::EnterPhysicalDpiContext()
  $script:physicalDpiContextEntered = $true
  $runningRows = Get-ProcessRows
  foreach ($row in $runningRows) {
    $pathMatches = $row.ExecutablePath -and ($binaries.Values -contains [IO.Path]::GetFullPath($row.ExecutablePath))
    $nameMatches = $row.Name -match '(?i)^captures(?:\.exe)?$|captures-windows-native'
    if ($pathMatches -or $nameMatches) { throw "Refusing to run while a Captures process exists (PID $($row.ProcessId), $($row.Name)). Close it and use a disposable account/VM." }
  }

  if ($StateMicrobenchmark) {
    $manifest = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\Cargo.toml'))
    if (-not (Test-Path -LiteralPath $manifest)) { throw 'The optional state microbenchmark requires a repository checkout next to this script.' }
    cargo bench --manifest-path $manifest --bench state
    if ($LASTEXITCODE -ne 0) { throw "state microbenchmark failed with exit code $LASTEXITCODE" }
  }

  New-Item -ItemType Directory -Path $knownConfigRoot | Out-Null
  $createdKnownRoots += $knownRoamingRoot
  New-Item -ItemType Directory -Path $knownDataRoot | Out-Null
  $createdKnownRoots += $knownLocalRoot
  New-Item -ItemType Directory -Path $outputParent -Force | Out-Null
  New-Item -ItemType Directory -Path $fixtureOutput, $nativeData, $artifactDirectory -Force | Out-Null
  @{
    settings_schema_version = 5
    appearance = 'light'
    output_directory = $fixtureOutput
    region_shortcut = 'Super+Shift+S'
    window_shortcut = 'Alt+PrintScreen'
    display_shortcut = 'Shift+PrintScreen'
    launch_at_login = $false
    onboarding_completed = $true
    show_mini_previews = $false
    include_mini_previews_in_captures = $false
    include_recording_controls_in_captures = $false
  } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $knownConfigRoot 'settings.json') -Encoding utf8
  @{
    appearance = 'light'
    output_directory = $fixtureOutput
    show_mini_previews = $false
    include_mini_previews_in_captures = $false
    include_recording_controls_in_captures = $false
  } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $nativeData 'settings.json') -Encoding utf8

  Write-Result
  foreach ($scenario in $scenarios) {
    foreach ($app in $apps) {
      Write-Host "$scenario warmup $app (excluded)"
      Run-Trial $app $scenario 0 $true
      Write-Result
    }
    for ($index = 1; $index -le $Trials; $index++) {
      foreach ($app in (Get-BenchmarkTrialOrder -Trial $index)) {
        Write-Host "$scenario $index/$Trials $app"
        Run-Trial $app $scenario $index $false
        Write-Result
      }
    }
  }
  Write-Result
  $outcome = Get-BenchmarkOutcome -Samples @($samples) -Failures @($failures) -Scenarios $scenarios -Apps $apps -ExpectedTrials $Trials
  Write-Host "Raw samples, medians, failures, hashes, process trees, and screenshots written to $outputPath"
  if (-not $outcome.complete) {
    throw "Measured benchmark incomplete: $($outcome.measured_failures) failed trial(s); inspect outcome.success_counts and failures in $outputPath"
  }
} finally {
  Remove-Item -LiteralPath $nativeData, $fixtureOutput -Recurse -Force -ErrorAction SilentlyContinue
  foreach ($path in $createdKnownRoots) { Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction SilentlyContinue }
  if ($script:launchedTauri) {
    foreach ($path in $tauriWebViewCandidates) { Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction SilentlyContinue }
  }
  if ($script:physicalDpiContextEntered) { [CapturesBenchmarkWindows]::RestoreDpiContext() }
}
