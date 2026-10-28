param(
  [Parameter(Mandatory)] [string]$Names,
  [Parameter(Mandatory)] [ValidateRange(1, 7200)] [int]$Seconds,
  [Parameter(Mandatory)] [string]$Out,
  [string]$RemoteAddress = "",
  [string]$StopFile = "",
  [int]$IntervalMs = 1000
)

$ErrorActionPreference = 'Stop'

if ($IntervalMs -lt 1) {
  throw 'IntervalMs must be positive'
}

$nameList = @(
  $Names.Split(',') |
    ForEach-Object { $_.Trim() } |
    Where-Object { $_.Length -gt 0 }
)
if ($nameList.Count -eq 0) {
  throw 'Names must contain at least one process name'
}

$outFull = [System.IO.Path]::GetFullPath($Out)
$outDir = [System.IO.Path]::GetDirectoryName($outFull)
if ([string]::IsNullOrEmpty($outDir)) {
  $outDir = (Get-Location).Path
}
$null = [System.IO.Directory]::CreateDirectory($outDir)
if (Test-Path -LiteralPath $outFull) {
  Remove-Item -LiteralPath $outFull -Force
}
$Out = $outFull

if ([string]::IsNullOrEmpty($StopFile)) {
  $StopFile = Join-Path $outDir 'sample.stop'
}

$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

function Write-JsonLine {
  param([object]$Value)
  $line = $Value | ConvertTo-Json -Compress -Depth 4
  [System.IO.File]::AppendAllText($Out, $line + "`n", $utf8NoBom)
}

function Unix-Microseconds {
  return [int64]([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() * 1000)
}

$logicalCpus = [int](Get-CimInstance -ClassName 'Win32_ComputerSystem' -ErrorAction Stop).NumberOfLogicalProcessors
if ($logicalCpus -lt 1) {
  $logicalCpus = 1
}

$nicAlias = $null
if (-not [string]::IsNullOrEmpty($RemoteAddress)) {
  $route = Find-NetRoute -RemoteIPAddress $RemoteAddress -ErrorAction Stop |
    Where-Object { $_.InterfaceAlias } |
    Select-Object -First 1
  if ($null -ne $route) {
    $nicAlias = [string]$route.InterfaceAlias
  }
}
else {
  $route = Get-NetRoute -DestinationPrefix '0.0.0.0/0' -ErrorAction Stop |
    Where-Object { $_.InterfaceAlias } |
    Sort-Object -Property @{ Expression = {
      [int64]$_.RouteMetric + [int64]$_.InterfaceMetric
    } } |
    Select-Object -First 1
  if ($null -ne $route) {
    $nicAlias = [string]$route.InterfaceAlias
  }
}

$gpuUnavailable = $false
try {
  $null = Get-CimInstance -ClassName 'Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine' -ErrorAction Stop |
    Select-Object -First 1
}
catch {
  $gpuUnavailable = $true
}

$startLine = [ordered]@{
  type = 'start'
  schema = 1
  t_us = Unix-Microseconds
  names = @($nameList)
  nic = $nicAlias
  logical_cpus = $logicalCpus
}
if ($gpuUnavailable) {
  $startLine.gpu_unavailable = $true
}
Write-JsonLine $startLine

$clockFrequency = [double][System.Diagnostics.Stopwatch]::Frequency
$clockStart = [System.Diagnostics.Stopwatch]::GetTimestamp()
$clockDeadline = $clockStart + [int64]($Seconds * $clockFrequency)
$sampleIndex = 0

while ($true) {
  $target = $clockStart + [int64](($sampleIndex * $IntervalMs * $clockFrequency) / 1000.0)
  if ($target -gt $clockDeadline) {
    $target = $clockDeadline
  }
  do {
    $now = [System.Diagnostics.Stopwatch]::GetTimestamp()
    $remaining = $target - $now
    if ($remaining -le 0) {
      break
    }
    $sleepMs = [int][Math]::Ceiling(($remaining * 1000.0) / $clockFrequency)
    if ($sleepMs -lt 1) {
      $sleepMs = 1
    }
    Start-Sleep -Milliseconds $sleepMs
  } while ($true)

  if (Test-Path -LiteralPath $StopFile) {
    Write-JsonLine ([ordered]@{ type = 'end'; t_us = Unix-Microseconds; reason = 'stop-file' })
    Remove-Item -LiteralPath $StopFile -Force -ErrorAction SilentlyContinue
    exit 0
  }
  if ([System.Diagnostics.Stopwatch]::GetTimestamp() -ge $clockDeadline) {
    Write-JsonLine ([ordered]@{ type = 'end'; t_us = Unix-Microseconds; reason = 'seconds' })
    exit 0
  }

  $pids = @()
  try {
    $pids = @(
      $nameList |
        ForEach-Object {
          Get-Process -Name $_ -ErrorAction SilentlyContinue |
          ForEach-Object { [int]$_.Id }
        } |
        Sort-Object -Unique
    )
  }
  catch {
    $pids = @()
  }

  $cpuPct = $null
  try {
    $cpuRows = Get-CimInstance -ClassName 'Win32_PerfFormattedData_PerfProc_Process' -ErrorAction Stop
    $cpuSum = 0.0
    foreach ($row in $cpuRows) {
      if ($pids -contains [int]$row.IDProcess -and $null -ne $row.PercentProcessorTime) {
        $cpuSum += [double]$row.PercentProcessorTime
      }
    }
    $cpuPct = [Math]::Round($cpuSum / $logicalCpus, 2)
  }
  catch {
    $cpuPct = $null
  }

  $cpuTotalPct = $null
  try {
    $totalRow = Get-CimInstance -ClassName 'Win32_PerfFormattedData_PerfOS_Processor' -ErrorAction Stop |
      Where-Object { $_.Name -eq '_Total' } |
      Select-Object -First 1
    if ($null -ne $totalRow -and $null -ne $totalRow.PercentProcessorTime) {
      $cpuTotalPct = [Math]::Round([double]$totalRow.PercentProcessorTime, 2)
    }
  }
  catch {
    $cpuTotalPct = $null
  }

  $gpu = @{}
  $gpuTotal = @{}
  if ($gpuUnavailable) {
    $gpu = @{}
    $gpuTotal = @{}
  }
  else {
    try {
      $gpuRows = Get-CimInstance -ClassName 'Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine' -ErrorAction Stop
      foreach ($row in $gpuRows) {
        $match = [regex]::Match([string]$row.Name, '^pid_(\d+)_.*_engtype_(.+)$')
        if (-not $match.Success -or $null -eq $row.UtilizationPercentage) {
          continue
        }
        $pid = [int]$match.Groups[1].Value
        $engine = $match.Groups[2].Value
        $value = [double]$row.UtilizationPercentage
        if ($gpuTotal.ContainsKey($engine)) {
          $gpuTotal[$engine] = [double]$gpuTotal[$engine] + $value
        }
        else {
          $gpuTotal[$engine] = $value
        }
        if ($pids -contains $pid) {
          if ($gpu.ContainsKey($engine)) {
            $gpu[$engine] = [double]$gpu[$engine] + $value
          }
          else {
            $gpu[$engine] = $value
          }
        }
      }
      $roundedGpu = @{}
      foreach ($key in $gpu.Keys) {
        $roundedGpu[$key] = [Math]::Round([double]$gpu[$key], 2)
      }
      $gpu = $roundedGpu
      $roundedGpuTotal = @{}
      foreach ($key in $gpuTotal.Keys) {
        $roundedGpuTotal[$key] = [Math]::Round([double]$gpuTotal[$key], 2)
      }
      $gpuTotal = $roundedGpuTotal
    }
    catch {
      $gpu = $null
      $gpuTotal = $null
    }
  }

  $nicTxBytes = $null
  $nicRxBytes = $null
  if (-not [string]::IsNullOrEmpty($nicAlias)) {
    try {
      $nicStats = Get-NetAdapterStatistics -Name $nicAlias -ErrorAction Stop |
        Select-Object -First 1
      if ($null -ne $nicStats) {
        $nicTxBytes = [int64]$nicStats.SentBytes
        $nicRxBytes = [int64]$nicStats.ReceivedBytes
      }
    }
    catch {
      $nicTxBytes = $null
      $nicRxBytes = $null
    }
  }

  Write-JsonLine ([ordered]@{
    type = 'sample'
    t_us = Unix-Microseconds
    pids = @($pids)
    cpu_pct = $cpuPct
    cpu_total_pct = $cpuTotalPct
    gpu = $gpu
    gpu_total = $gpuTotal
    nic_tx_bytes = $nicTxBytes
    nic_rx_bytes = $nicRxBytes
  })
  $sampleIndex++
}
