function Get-BenchmarkMedian {
  param([Parameter(Mandatory)][AllowEmptyCollection()][object[]]$Values)

  $ordered = @($Values | ForEach-Object { [double]$_ } | Sort-Object)
  if ($ordered.Count -eq 0) { return $null }
  $middle = [int][Math]::Floor($ordered.Count / 2)
  if ($ordered.Count % 2) { return $ordered[$middle] }
  ($ordered[$middle - 1] + $ordered[$middle]) / 2
}

function Get-BenchmarkTrialOrder {
  param([Parameter(Mandatory)][ValidateRange(1, [int]::MaxValue)][int]$Trial)

  if ($Trial % 2) { @('tauri', 'native') } else { @('native', 'tauri') }
}

function Get-BenchmarkTreeIds {
  param(
    [Parameter(Mandatory)][int]$RootProcessId,
    [Parameter(Mandatory)][object[]]$Rows
  )

  $children = @{}
  foreach ($row in $Rows) {
    $parentId = [int]$row.ParentProcessId
    if (-not $children.ContainsKey($parentId)) {
      $children[$parentId] = [Collections.Generic.List[int]]::new()
    }
    if ([int]$row.ProcessId -gt 0) { $children[$parentId].Add([int]$row.ProcessId) }
  }
  $ids = [Collections.Generic.HashSet[int]]::new()
  $queue = [Collections.Queue]::new()
  [void]$ids.Add($RootProcessId)
  $queue.Enqueue($RootProcessId)
  while ($queue.Count -gt 0) {
    $parentId = [int]$queue.Dequeue()
    if (-not $children.ContainsKey($parentId)) { continue }
    foreach ($childId in $children[$parentId]) {
      if ($ids.Add($childId)) { $queue.Enqueue($childId) }
    }
  }
  @($ids)
}

function Compare-BenchmarkSignatures {
  param(
    [Parameter(Mandatory)][int[]]$Previous,
    [Parameter(Mandatory)][int[]]$Current
  )

  if ($Previous.Count -ne $Current.Count -or $Current.Count -eq 0) {
    throw 'Frame signatures must have the same non-zero length.'
  }
  [double]$difference = 0
  for ($index = 0; $index -lt $Current.Count; $index++) {
    $difference += [Math]::Abs($Current[$index] - $Previous[$index])
  }
  $difference / $Current.Count
}

function Test-BenchmarkStableFrameHeuristic {
  param(
    [Parameter(Mandatory)][int[]]$Previous,
    [Parameter(Mandatory)][int[]]$Current,
    [double]$MinimumRange = 8,
    [double]$MaximumMeanDifference = 1
  )

  $range = ($Current | Measure-Object -Maximum).Maximum - ($Current | Measure-Object -Minimum).Minimum
  $difference = Compare-BenchmarkSignatures -Previous $Previous -Current $Current
  @{
    passed = $range -ge $MinimumRange -and $difference -le $MaximumMeanDifference
    sample_range = [double]$range
    mean_absolute_difference = [double]$difference
  }
}

function Get-BenchmarkSummaries {
  param([Parameter(Mandatory)][AllowEmptyCollection()][object[]]$Samples)

  $measured = @($Samples | Where-Object { -not $_.warmup })
  $fields = @(
    'first_visible_mapped_ms',
    'stable_frame_heuristic_ms',
    'private_bytes',
    'working_set_bytes',
    'idle_cpu_percent_one_core',
    'process_count'
  )
  @($measured | Group-Object scenario, app | ForEach-Object {
    $first = $_.Group[0]
    $medians = @{}
    foreach ($field in $fields) {
      $medians[$field] = Get-BenchmarkMedian -Values @($_.Group | ForEach-Object { $_[$field] })
    }
    @{
      scenario = $first.scenario
      app = $first.app
      successful_measured_trials = $_.Count
      medians = $medians
    }
  })
}

function Get-BenchmarkOutcome {
  param(
    [Parameter(Mandatory)][AllowEmptyCollection()][object[]]$Samples,
    [Parameter(Mandatory)][AllowEmptyCollection()][object[]]$Failures,
    [Parameter(Mandatory)][string[]]$Scenarios,
    [Parameter(Mandatory)][string[]]$Apps,
    [Parameter(Mandatory)][int]$ExpectedTrials
  )

  $counts = @()
  foreach ($scenario in $Scenarios) {
    foreach ($app in $Apps) {
      $measuredSuccesses = @($Samples | Where-Object { -not $_.warmup -and $_.scenario -eq $scenario -and $_.app -eq $app }).Count
      $warmupSuccesses = @($Samples | Where-Object { $_.warmup -and $_.scenario -eq $scenario -and $_.app -eq $app }).Count
      $counts += @{
        scenario = $scenario
        app = $app
        expected_measured_trials = $ExpectedTrials
        successful_measured_trials = $measuredSuccesses
        successful_warmups = $warmupSuccesses
        measured_count_matches = $measuredSuccesses -eq $ExpectedTrials
      }
    }
  }
  $measuredFailures = @($Failures | Where-Object { -not $_.warmup }).Count
  @{
    success_counts = $counts
    measured_failures = $measuredFailures
    warmup_failures = @($Failures | Where-Object { $_.warmup }).Count
    complete = $Failures.Count -eq 0 -and @($counts | Where-Object { -not $_.measured_count_matches -or $_.successful_warmups -ne 1 }).Count -eq 0
  }
}
