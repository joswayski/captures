$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'benchmark-core.ps1')

function Assert-Equal($Expected, $Actual, [string]$Message) {
  if ($Expected -ne $Actual) { throw "$Message (expected $Expected, got $Actual)" }
}

Assert-Equal 7 (Get-BenchmarkMedian -Values @(9, 1, 7)) 'odd median must sort samples'
Assert-Equal 5 (Get-BenchmarkMedian -Values @(8, 2, 6, 4)) 'even median must average the middle samples'
Assert-Equal $null (Get-BenchmarkMedian -Values @()) 'empty samples must have no median'
Assert-Equal 0 (@(Get-BenchmarkSummaries -Samples @()).Count) 'empty samples must produce no summaries'
Assert-Equal 'tauri,native' ((Get-BenchmarkTrialOrder -Trial 1) -join ',') 'odd trials must start with Tauri'
Assert-Equal 'native,tauri' ((Get-BenchmarkTrialOrder -Trial 2) -join ',') 'even trials must start with native'

$rows = @(
  [pscustomobject]@{ ProcessId = 10; ParentProcessId = 1 },
  [pscustomobject]@{ ProcessId = 11; ParentProcessId = 10 },
  [pscustomobject]@{ ProcessId = 12; ParentProcessId = 11 },
  [pscustomobject]@{ ProcessId = 20; ParentProcessId = 1 }
)
Assert-Equal '10,11,12' ((Get-BenchmarkTreeIds -RootProcessId 10 -Rows $rows | Sort-Object) -join ',') 'tree traversal must include descendants and exclude siblings'

$blank = Test-BenchmarkStableFrameHeuristic -Previous @(20, 20, 20, 20) -Current @(20, 20, 20, 20)
Assert-Equal $false $blank.passed 'a stable blank frame does not pass the heuristic'
$moving = Test-BenchmarkStableFrameHeuristic -Previous @(0, 40, 80, 120) -Current @(20, 60, 100, 140)
Assert-Equal $false $moving.passed 'a changing non-blank frame does not pass the heuristic'
$wrongContent = Test-BenchmarkStableFrameHeuristic -Previous @(0, 40, 80, 120) -Current @(0, 40, 80, 120)
Assert-Equal $true $wrongContent.passed 'stable wrong content or an error frame passes the heuristic without certifying readiness'

$samples = @(
  @{ scenario = 'editor'; app = 'native'; warmup = $true; first_visible_mapped_ms = 1000; stable_frame_heuristic_ms = 2000; private_bytes = 9000; working_set_bytes = 4500; idle_cpu_percent_one_core = 30; process_count = 9 },
  @{ scenario = 'editor'; app = 'native'; warmup = $false; first_visible_mapped_ms = 9; stable_frame_heuristic_ms = 19; private_bytes = 90; working_set_bytes = 45; idle_cpu_percent_one_core = 3; process_count = 3 },
  @{ scenario = 'editor'; app = 'native'; warmup = $false; first_visible_mapped_ms = 1; stable_frame_heuristic_ms = 11; private_bytes = 10; working_set_bytes = 25; idle_cpu_percent_one_core = 1; process_count = 1 }
)
$summary = @(Get-BenchmarkSummaries -Samples $samples)[0]
Assert-Equal 5 $summary.medians.first_visible_mapped_ms 'summary must report startup median'
Assert-Equal 50 $summary.medians.private_bytes 'summary must report memory median'
Assert-Equal 2 $summary.medians.process_count 'summary must report process-tree size median'

$failures = @(@{ scenario = 'preferences'; app = 'tauri'; warmup = $false })
$outcome = Get-BenchmarkOutcome -Samples $samples -Failures $failures -Scenarios @('editor', 'preferences') -Apps @('tauri', 'native') -ExpectedTrials 2
Assert-Equal $false $outcome.complete 'a measured failure or mismatched success count must fail the benchmark'
Assert-Equal 1 $outcome.measured_failures 'measured failures must be reported separately from warmups'
$nativeEditor = @($outcome.success_counts | Where-Object { $_.scenario -eq 'editor' -and $_.app -eq 'native' })[0]
Assert-Equal $true $nativeEditor.measured_count_matches 'warmups must not affect measured success counts'

$complete = Get-BenchmarkOutcome -Samples $samples -Failures @() -Scenarios @('editor') -Apps @('native') -ExpectedTrials 2
Assert-Equal $true $complete.complete 'all measured trials plus the excluded warmup complete the run'
$noWarmup = Get-BenchmarkOutcome -Samples @($samples | Where-Object { -not $_.warmup }) -Failures @(@{warmup=$true}) -Scenarios @('editor') -Apps @('native') -ExpectedTrials 2
Assert-Equal $false $noWarmup.complete 'a failed warmup cannot silently turn the first cold launch into a warm measurement'

Write-Host 'benchmark-core tests passed'
