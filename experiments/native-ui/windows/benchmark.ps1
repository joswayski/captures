[CmdletBinding()]
param(
  [Parameter(Mandatory)][ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })][string]$TauriExecutable,
  [Parameter(Mandatory)][ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })][string]$NativeExecutable,
  [Parameter(Mandatory)][ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })][string]$EditorInput,
  [Parameter(Mandatory)][switch]$DisposableAccount,
  [string]$Output = (Join-Path $PWD 'windows-benchmark.json'),
  [ValidateRange(2, 100)][int]$Trials = 10,
  [ValidateRange(0, 300)][double]$SettleSeconds = 2,
  [ValidateRange(0.1, 300)][double]$IdleSeconds = 3,
  [ValidateRange(1, 300)][double]$WindowTimeoutSeconds = 30,
  [switch]$Force
)

$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) { throw 'PowerShell 7 or newer is required (ProcessStartInfo.ArgumentList is used).' }
if (-not $IsWindows) { throw 'This benchmark only runs on Windows.' }
if (-not $DisposableAccount) { throw 'Acknowledge that this is a dedicated disposable Windows account or VM with -DisposableAccount.' }

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class BenchmarkWindows {
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
}
'@

function Resolve-FullPath([string]$Path) {
  [IO.Path]::GetFullPath((Resolve-Path -LiteralPath $Path).Path)
}

$binaries = @{ tauri = Resolve-FullPath $TauriExecutable; native = Resolve-FullPath $NativeExecutable }
$inputPath = Resolve-FullPath $EditorInput
$outputPath = [IO.Path]::GetFullPath($Output)
$artifactDirectory = Join-Path ([IO.Path]::GetDirectoryName($outputPath)) (([IO.Path]::GetFileNameWithoutExtension($outputPath)) + '-artifacts')
if ($binaries.tauri -eq $binaries.native) { throw 'TauriExecutable and NativeExecutable must differ.' }
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
  @{ process_id = $ProcessId; start_time_utc = $process.StartTime.ToUniversalTime().Ticks }
}

function Test-Identity($Identity) {
  try { (Get-Process -Id $Identity.process_id -ErrorAction Stop).StartTime.ToUniversalTime().Ticks -eq $Identity.start_time_utc } catch { $false }
}

function Get-TreeIds([int]$RootProcessId, $Rows) {
  $children = @{}
  foreach ($row in $Rows) {
    $parentId = [int]$row.ParentProcessId
    if (-not $children.ContainsKey($parentId)) { $children[$parentId] = [Collections.Generic.List[int]]::new() }
    if ([int]$row.ProcessId -gt 0) { $children[$parentId].Add([int]$row.ProcessId) }
  }
  $ids = [Collections.Generic.HashSet[int]]::new()
  $queue = [Collections.Queue]::new()
  [void]$ids.Add($RootProcessId); $queue.Enqueue($RootProcessId)
  while ($queue.Count -gt 0) {
    $parentId = [int]$queue.Dequeue()
    if ($children.ContainsKey($parentId)) {
      foreach ($childId in $children[$parentId]) { if ($ids.Add($childId)) { $queue.Enqueue($childId) } }
    }
  }
  @($ids)
}

function Get-Metrics([int[]]$ProcessIds) {
  $processes = @($ProcessIds | Where-Object { $_ -gt 0 } | ForEach-Object { Get-Process -Id $_ -ErrorAction Stop })
  if ($processes.Count -eq 0) { throw 'The owned process tree disappeared during measurement.' }
  @{
    private_bytes = [int64](($processes | Measure-Object PrivateMemorySize64 -Sum).Sum)
    working_set_bytes = [int64](($processes | Measure-Object WorkingSet64 -Sum).Sum)
    cpu_seconds = [double](($processes | Measure-Object CPU -Sum).Sum)
    process_count = $processes.Count
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
  $callback = [BenchmarkWindows+EnumWindowsProc]{
    param([IntPtr]$handle, [IntPtr]$parameter)
    [uint32]$windowProcessId = 0
    [void][BenchmarkWindows]::GetWindowThreadProcessId($handle, [ref]$windowProcessId)
    if ($windowProcessId -eq $RootProcessId -and [BenchmarkWindows]::IsWindowVisible($handle) -and -not [BenchmarkWindows]::IsIconic($handle)) {
      $text = [Text.StringBuilder]::new(512)
      [void][BenchmarkWindows]::GetWindowText($handle, $text, $text.Capacity)
      if ($text.ToString() -match $TitlePattern) { $script:foundWindow = $handle; $script:foundTitle = $text.ToString(); return $false }
    }
    $true
  }
  [void][BenchmarkWindows]::EnumWindows($callback, [IntPtr]::Zero)
  if ($script:foundWindow -ne [IntPtr]::Zero) { @{ handle = $script:foundWindow; title = $script:foundTitle } }
}

function Set-ClientSize([IntPtr]$Handle, [int]$Width, [int]$Height) {
  $client = [BenchmarkWindows+RECT]::new(); $outer = [BenchmarkWindows+RECT]::new()
  if (-not [BenchmarkWindows]::GetClientRect($Handle, [ref]$client) -or -not [BenchmarkWindows]::GetWindowRect($Handle, [ref]$outer)) { throw 'Could not inspect target window dimensions.' }
  $extraWidth = ($outer.Right - $outer.Left) - ($client.Right - $client.Left)
  $extraHeight = ($outer.Bottom - $outer.Top) - ($client.Bottom - $client.Top)
  if (-not [BenchmarkWindows]::SetWindowPos($Handle, [IntPtr]::Zero, 0, 0, $Width + $extraWidth, $Height + $extraHeight, 0x0002 -bor 0x0004 -bor 0x0010)) { throw 'Could not set target client dimensions.' }
  Start-Sleep -Milliseconds 100
  if (-not [BenchmarkWindows]::GetClientRect($Handle, [ref]$client) -or ($client.Right - $client.Left) -ne $Width -or ($client.Bottom - $client.Top) -ne $Height) { throw 'The window refused the matched client dimensions.' }
}

function Save-WindowScreenshot([IntPtr]$Handle, [string]$Path) {
  [void][BenchmarkWindows]::SetForegroundWindow($Handle)
  Start-Sleep -Milliseconds 250
  $client = [BenchmarkWindows+RECT]::new(); $origin = [BenchmarkWindows+POINT]::new()
  if (-not [BenchmarkWindows]::GetClientRect($Handle, [ref]$client) -or -not [BenchmarkWindows]::ClientToScreen($Handle, [ref]$origin)) { throw 'Could not locate target client area for screenshot.' }
  $width = $client.Right - $client.Left; $height = $client.Bottom - $client.Top
  $bitmap = [Drawing.Bitmap]::new($width, $height)
  try {
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try { $graphics.CopyFromScreen($origin.X, $origin.Y, 0, 0, [Drawing.Size]::new($width, $height)) }
    finally { $graphics.Dispose() }
    $bitmap.Save($Path, [Drawing.Imaging.ImageFormat]::Png)
  } finally { $bitmap.Dispose() }
}

# This fixture is intentionally written to the real KnownFolder-backed shipping path.
# Environment overrides do not redirect directories::ProjectDirs on Windows.
$fixtureOutput = Join-Path ([IO.Path]::GetTempPath()) ('captures-benchmark-output-' + [guid]::NewGuid())
$nativeData = Join-Path ([IO.Path]::GetTempPath()) ('captures-native-benchmark-' + [guid]::NewGuid())
$createdKnownRoots = @()
$script:launchedTauri = $false
$samples = [Collections.Generic.List[object]]::new()
$failures = [Collections.Generic.List[object]]::new()

function Write-Result {
  $result = @{
    recorded_at_utc = [DateTime]::UtcNow.ToString('o')
    executables = @{
      tauri = @{ path = $binaries.tauri; sha256 = (Get-FileHash -LiteralPath $binaries.tauri).Hash; bytes = (Get-Item -LiteralPath $binaries.tauri).Length }
      native = @{ path = $binaries.native; sha256 = (Get-FileHash -LiteralPath $binaries.native).Hash; bytes = (Get-Item -LiteralPath $binaries.native).Length }
    }
    editor_input = @{ path = $inputPath; sha256 = (Get-FileHash -LiteralPath $inputPath).Hash; bytes = (Get-Item -LiteralPath $inputPath).Length }
    environment = @{ os = [Environment]::OSVersion.VersionString; machine = $env:PROCESSOR_IDENTIFIER; logical_processors = [Environment]::ProcessorCount; powershell = $PSVersionTable.PSVersion.ToString() }
    configuration = @{ trials = $Trials; settle_seconds = $SettleSeconds; idle_seconds = $IdleSeconds; window_timeout_seconds = $WindowTimeoutSeconds; preferences_client_size = '980x720'; editor_client_size = '1280x760'; ordering = 'alternating app order within each scenario' }
    caveats = @(
      'Startup timing is time to the observed visible target window, not UI readiness or completed rendering.',
      'Screenshots copy the desktop client-area region after foregrounding; overlapping windows, notifications, focus denial, and occlusion can affect the PNG.',
      'Private bytes and working set are process-tree sums; working set can double-count shared pages.',
      'CPU is process-tree CPU-time delta as percent of one logical core and includes WebView child processes.',
      'OS caches are not flushed and background load remains a confounder.'
    )
    samples = @($samples)
    failures = @($failures)
  }
  $result | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $outputPath -Encoding utf8
}

function Run-Trial([string]$App, [string]$Scenario, [int]$Index) {
  $process = $null; $rootIdentity = $null
  try {
    $arguments = if ($Scenario -eq 'editor') { @($inputPath) } elseif ($App -eq 'native') { @('--preferences') } else { @() }
    $titlePattern = if ($Scenario -eq 'editor') { '^(Captures Screenshot Editor|Captures — Image editor)$' } else { '^Captures Preferences$' }
    $size = if ($Scenario -eq 'editor') { @(1280, 760) } else { @(980, 720) }
    $startInfo = [Diagnostics.ProcessStartInfo]::new($binaries[$App])
    $startInfo.UseShellExecute = $false
    foreach ($argument in $arguments) { [void]$startInfo.ArgumentList.Add($argument) }
    if ($App -eq 'native') { $startInfo.Environment['CAPTURES_NATIVE_DATA'] = $nativeData }
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $process = [Diagnostics.Process]::Start($startInfo)
    if ($App -eq 'tauri') { $script:launchedTauri = $true }
    $rootIdentity = Get-Identity $process.Id
    $deadline = [DateTime]::UtcNow.AddSeconds($WindowTimeoutSeconds); $window = $null
    do {
      if ($process.HasExited) { throw "$App exited before showing $Scenario (exit $($process.ExitCode))." }
      $window = Find-TargetWindow $process.Id $titlePattern
      if (-not $window) { Start-Sleep -Milliseconds 10 }
    } while (-not $window -and [DateTime]::UtcNow -lt $deadline)
    if (-not $window) { throw "$App did not show a visible $Scenario target window within $WindowTimeoutSeconds seconds." }
    $visibleMilliseconds = $watch.Elapsed.TotalMilliseconds
    Set-ClientSize $window.handle $size[0] $size[1]
    Start-Sleep -Milliseconds ([int]($SettleSeconds * 1000))
    $screenshotName = '{0}-{1}-{2:D2}.png' -f $Scenario, $App, $Index
    $screenshotPath = Join-Path $artifactDirectory $screenshotName
    Save-WindowScreenshot $window.handle $screenshotPath
    $rowsBefore = Get-ProcessRows
    $treeBefore = @(Get-TreeIds $process.Id $rowsBefore)
    $identitiesBefore = @($treeBefore | ForEach-Object { Get-Identity $_ })
    $before = Get-Metrics $treeBefore
    $cpuWatch = [Diagnostics.Stopwatch]::StartNew()
    Start-Sleep -Milliseconds ([int]($IdleSeconds * 1000))
    $rowsAfter = Get-ProcessRows
    $treeAfter = @(Get-TreeIds $process.Id $rowsAfter)
    if ((Compare-Object $treeBefore $treeAfter) -or @($identitiesBefore | Where-Object { -not (Test-Identity $_) }).Count) { throw 'Process tree changed during the CPU sample; excluding this trial.' }
    $after = Get-Metrics $treeAfter
    $samples.Add(@{
      app = $App; scenario = $Scenario; trial = $Index
      startup_observed_visible_target_window_ms = $visibleMilliseconds; window_title = $window.title
      client_width = $size[0]; client_height = $size[1]
      private_bytes = $after.private_bytes; working_set_bytes = $after.working_set_bytes; process_count = $after.process_count
      idle_cpu_percent_one_core = 100 * ($after.cpu_seconds - $before.cpu_seconds) / $cpuWatch.Elapsed.TotalSeconds
      process_ids_before = $treeBefore; process_ids_after = $treeAfter
      screenshot = @{ path = $screenshotPath; sha256 = (Get-FileHash -LiteralPath $screenshotPath).Hash; bytes = (Get-Item -LiteralPath $screenshotPath).Length }
    })
  } finally {
    if ($rootIdentity) { Stop-OwnedTree $rootIdentity }
  }
}

try {
  $runningRows = Get-ProcessRows
  foreach ($row in $runningRows) {
    $pathMatches = $row.ExecutablePath -and ($binaries.Values -contains [IO.Path]::GetFullPath($row.ExecutablePath))
    $nameMatches = $row.Name -match '(?i)^captures(?:\.exe)?$|captures-windows-native'
    if ($pathMatches -or $nameMatches) { throw "Refusing to run while a Captures process exists (PID $($row.ProcessId), $($row.Name)). Close it and use a disposable account/VM." }
  }

  New-Item -ItemType Directory -Path $knownConfigRoot | Out-Null; $createdKnownRoots += $knownRoamingRoot
  New-Item -ItemType Directory -Path $knownDataRoot | Out-Null; $createdKnownRoots += $knownLocalRoot
  New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($outputPath)) -Force | Out-Null
  New-Item -ItemType Directory -Path $fixtureOutput, $nativeData, $artifactDirectory -Force | Out-Null
  @{
    settings_schema_version = 5; appearance = 'light'; output_directory = $fixtureOutput
    region_shortcut = 'Super+Shift+S'; window_shortcut = 'Alt+PrintScreen'; display_shortcut = 'Shift+PrintScreen'
    launch_at_login = $false; onboarding_completed = $true
  } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $knownConfigRoot 'settings.json') -Encoding utf8
  @{
    appearance = 'light'; output_directory = $fixtureOutput
  } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $nativeData 'settings.json') -Encoding utf8

  Write-Result
  foreach ($scenario in @('preferences', 'editor')) {
    for ($index = 1; $index -le $Trials; $index++) {
      $order = if ($index % 2) { @('tauri', 'native') } else { @('native', 'tauri') }
      foreach ($app in $order) {
        Write-Host "$scenario $index/$Trials $app"
        try { Run-Trial $app $scenario $index }
        catch { $failures.Add(@{ app = $app; scenario = $scenario; trial = $index; recorded_at_utc = [DateTime]::UtcNow.ToString('o'); error = $_.Exception.Message }); Write-Warning $_.Exception.Message }
        Write-Result
      }
    }
  }
  Write-Host "Raw samples, failures, and hashes written to $outputPath"
} finally {
  Remove-Item -LiteralPath $nativeData, $fixtureOutput -Recurse -Force -ErrorAction SilentlyContinue
  # Preflight proved these exact KnownFolder/profile directories did not exist.
  foreach ($path in $createdKnownRoots) { Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction SilentlyContinue }
  if ($script:launchedTauri) {
    foreach ($path in $tauriWebViewCandidates) { Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction SilentlyContinue }
  }
}
