param(
  [Parameter(Position = 0, Mandatory = $true)] [string]$Command,
  [string]$Path, [string]$Scene, [int]$Seconds = 60, [int]$Calib = 4, [int]$Delay = 0,
  [string]$Names, [string]$RemoteAddress, [string]$Out, [string]$Id, [string]$Action
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
. (Join-Path $PSScriptRoot 'common.ps1')

function Get-HostInfoResult {
  $os = Get-CimInstance -ClassName Win32_OperatingSystem -ErrorAction Stop
  $processor = @(Get-CimInstance -ClassName Win32_Processor -ErrorAction Stop) | Select-Object -First 1
  $computer = Get-CimInstance -ClassName Win32_ComputerSystem -ErrorAction Stop
  $controllers = @(Get-CimInstance -ClassName Win32_VideoController -ErrorAction SilentlyContinue)
  $gpus = @(
    $controllers |
      ForEach-Object { [string]$_.Name } |
      Where-Object { -not [string]::IsNullOrEmpty($_) }
  )
  $display = $null
  foreach ($controller in $controllers) {
    if ($null -ne $controller.CurrentHorizontalResolution -and
        $null -ne $controller.CurrentVerticalResolution -and
        [int]$controller.CurrentHorizontalResolution -gt 0 -and
        [int]$controller.CurrentVerticalResolution -gt 0) {
      $refresh = 0
      if ($null -ne $controller.CurrentRefreshRate) {
        $refresh = [int]$controller.CurrentRefreshRate
      }
      $display = '{0}x{1}@{2}' -f [int]$controller.CurrentHorizontalResolution,
        [int]$controller.CurrentVerticalResolution, $refresh
      break
    }
  }
  return [ordered]@{
    hostname = [string]$env:COMPUTERNAME
    os_version = [string]$os.Version
    os_caption = [string]$os.Caption
    cpu = [string]$processor.Name
    logical_cpus = [int]$computer.NumberOfLogicalProcessors
    gpus = @($gpus)
    display = $display
  }
}

function Get-SessionResult {
  $consoleId = Get-ConsoleSession
  $explorer = @(Get-Process -Name explorer -ErrorAction SilentlyContinue)
  $logonUi = @(Get-Process -Name LogonUI -ErrorAction SilentlyContinue)
  $loggedIn = @($explorer | Where-Object { [int]$_.SessionId -eq $consoleId }).Count -gt 0
  $locked = @($logonUi | Where-Object { [int]$_.SessionId -eq $consoleId }).Count -gt 0
  $rdp = @(
    $explorer | Where-Object {
      [int]$_.SessionId -ne $consoleId -and [int]$_.SessionId -ne 0
    }
  ).Count -gt 0
  $computer = Get-CimInstance -ClassName Win32_ComputerSystem -ErrorAction Stop
  return [ordered]@{
    console_session_id = $consoleId
    logged_in = [bool]$loggedIn
    locked = [bool]$locked
    user = [string]$computer.UserName
    rdp = [bool]$rdp
  }
}

function Get-InventoryResult {
  $services = @(
    Get-Service -ErrorAction SilentlyContinue |
      ForEach-Object { [string]$_.Name } |
      Where-Object { $_ } |
      Sort-Object -Unique
  )
  $programs = @(
    Get-UninstallEntry -NameLike '*' |
      ForEach-Object { '{0} {1}' -f $_.DisplayName, $_.DisplayVersion } |
      Where-Object { $_ } |
      Sort-Object -Unique
  )
  $firewall = @()
  try {
    $firewall = @(
      Get-NetFirewallRule -ErrorAction Stop |
        ForEach-Object { [string]$_.DisplayName } |
        Where-Object { $_ } |
        Sort-Object -Unique
    )
  }
  catch {
    $firewall = @()
  }
  $drivers = @(
    Get-CimInstance -ClassName Win32_SystemDriver -ErrorAction SilentlyContinue |
      ForEach-Object { [string]$_.Name } |
      Where-Object { $_ } |
      Sort-Object -Unique
  )
  return [ordered]@{
    services = @($services)
    programs = @($programs)
    firewall_rules = @($firewall)
    drivers = @($drivers)
  }
}

function Stop-PageProcesses {
  $profile = Join-Path (Get-BenchDir) 'edge-profile'
  $processes = @(Get-ProcessByCommandLine -Name 'msedge.exe' -Contains $profile)
  $killed = 0
  foreach ($process in $processes) {
    try {
      Stop-Process -Id ([int]$process.ProcessId) -Force -ErrorAction Stop
      $killed++
    }
    catch {
      if ($_.Exception.Message -notmatch '(?i)(cannot find|no process)') {
        throw
      }
    }
  }
  $null = Stop-Interactive -Name 'page'
  return $killed
}

function Start-Page {
  if (@('static', 'scroll', 'video', 'drag', 'calib') -notcontains $Scene) {
    throw ('invalid scene: ' + $Scene)
  }
  if ($Seconds -lt 1 -or $Seconds -gt 3600) {
    throw 'Seconds must be between 1 and 3600'
  }
  if ($Calib -lt 1 -or $Calib -gt 30) {
    throw 'Calib must be between 1 and 30'
  }
  if ($Delay -lt 0 -or $Delay -gt 30) {
    throw 'Delay must be between 0 and 30'
  }
  $bench = Get-BenchDir
  $page = Join-Path $bench 'page\bench.html'
  if (-not (Test-Path -LiteralPath $page -PathType Leaf)) {
    throw ('page not found: ' + $page)
  }
  $edge = Find-Edge
  if ([string]::IsNullOrEmpty($edge)) {
    throw 'Microsoft Edge was not found'
  }

  $null = Stop-PageProcesses
  $profile = Join-Path $bench 'edge-profile'
  if (Test-Path -LiteralPath $profile) {
    Remove-Item -LiteralPath $profile -Recurse -Force
  }
  $null = New-Item -ItemType Directory -Path $profile -Force
  $pageFull = [System.IO.Path]::GetFullPath($page)
  $url = ('file:///' + $pageFull.Replace('\', '/') + '#scene={0}&seconds={1}&calib={2}&delay={3}') -f $Scene, $Seconds, $Calib, $Delay
  $commandLine = 'start "" "{0}" --kiosk "{1}" --edge-kiosk-type=fullscreen --no-first-run --no-default-browser-check --disable-features=Translate --user-data-dir="{2}"' -f $edge, $url, $profile
  $null = Start-Interactive -Name 'page' -CommandLine $commandLine
  $started = Wait-Until -TimeoutSec 15 -Condition {
    @(Get-ProcessByCommandLine -Name 'msedge.exe' -Contains $profile).Count -gt 0
  }
  if (-not $started) {
    throw 'edge did not start (is the console session logged in and unlocked?)'
  }
  $process = @(Get-ProcessByCommandLine -Name 'msedge.exe' -Contains $profile) | Select-Object -First 1
  return [ordered]@{ pid = [int]$process.ProcessId; url = $url }
}

function Start-Sample {
  if ([string]::IsNullOrEmpty($Names) -or $Names -notmatch '^[A-Za-z0-9._,-]+$') {
    throw 'Names contains invalid characters'
  }
  if ([string]::IsNullOrEmpty($RemoteAddress) -or $RemoteAddress -notmatch '^[0-9A-Fa-f:.]+$') {
    throw 'RemoteAddress contains invalid characters'
  }
  if ($Seconds -lt 1 -or $Seconds -gt 7200) {
    throw 'Seconds must be between 1 and 7200'
  }
  $fullOut = Assert-UnderBenchDir $Out
  $outDir = [System.IO.Path]::GetDirectoryName($fullOut)
  if ([string]::IsNullOrEmpty($outDir)) {
    throw 'Out must name a file'
  }
  $null = New-Item -ItemType Directory -Path $outDir -Force
  $stopFile = Join-Path $outDir 'sample.stop'
  if (Test-Path -LiteralPath $stopFile) {
    Remove-Item -LiteralPath $stopFile -Force
  }
  if (Test-Path -LiteralPath $fullOut) {
    Remove-Item -LiteralPath $fullOut -Force
  }
  $sample = Join-Path (Get-BenchDir) 'host\sample.ps1'
  if (-not (Test-Path -LiteralPath $sample -PathType Leaf)) {
    throw ('sample script not found: ' + $sample)
  }
  $commandLine = 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File "{0}" -Names "{1}" -Seconds {2} -RemoteAddress "{3}" -Out "{4}"' -f $sample, $Names, $Seconds, $RemoteAddress, $fullOut
  $sampleProcessId = Start-Detached -CommandLine $commandLine
  $pidPath = Join-Path $outDir 'sample.pid'
  [System.IO.File]::WriteAllText($pidPath, [string]$sampleProcessId, (New-Object System.Text.ASCIIEncoding))
  $started = Wait-Until -TimeoutSec 8 -Condition {
    if (-not (Test-Path -LiteralPath $fullOut -PathType Leaf)) {
      return $false
    }
    return [System.IO.File]::ReadAllLines($fullOut).Count -ge 1
  }
  if (-not $started) {
    throw 'sampler did not start'
  }
  return [ordered]@{ pid = $sampleProcessId }
}

function Stop-Sample {
  $fullOut = Assert-UnderBenchDir $Out
  $outDir = [System.IO.Path]::GetDirectoryName($fullOut)
  if ([string]::IsNullOrEmpty($outDir)) {
    throw 'Out must name a file'
  }
  $null = New-Item -ItemType Directory -Path $outDir -Force
  $stopFile = Join-Path $outDir 'sample.stop'
  [System.IO.File]::WriteAllText($stopFile, '', (New-Object System.Text.ASCIIEncoding))
  $ended = Wait-Until -TimeoutSec 5 -Condition {
    if (-not (Test-Path -LiteralPath $fullOut -PathType Leaf)) {
      return $false
    }
    $lines = [System.IO.File]::ReadAllLines($fullOut)
    if ($lines.Count -eq 0) {
      return $false
    }
    return $lines[$lines.Count - 1].IndexOf('"type":"end"', [System.StringComparison]::OrdinalIgnoreCase) -ge 0
  }
  $forced = $false
  if (-not $ended) {
    $pidPath = Join-Path $outDir 'sample.pid'
    if (Test-Path -LiteralPath $pidPath) {
      $pidText = [System.IO.File]::ReadAllText($pidPath).Trim()
      $samplePid = 0
      if ([int]::TryParse($pidText, [ref]$samplePid) -and $samplePid -gt 0) {
        Stop-Process -Id $samplePid -Force -ErrorAction SilentlyContinue
      }
    }
    $forced = $true
  }
  $lineCount = 0
  if (Test-Path -LiteralPath $fullOut -PathType Leaf) {
    $lineCount = [System.IO.File]::ReadAllLines($fullOut).Count
  }
  $result = [ordered]@{ lines = $lineCount }
  if ($forced) {
    $result.forced = $true
  }
  return $result
}

try {
  switch ($Command) {
    'hostinfo' {
      Write-Result (Get-HostInfoResult)
    }
    'session' {
      Write-Result (Get-SessionResult)
    }
    'env' {
      $bench = Get-BenchDir
      Write-Result ([ordered]@{
        bench_dir = $bench
        rs_bench_exe = Test-Path -LiteralPath (Join-Path $bench 'bin\rs-bench.exe') -PathType Leaf
        page = Test-Path -LiteralPath (Join-Path $bench 'page\bench.html') -PathType Leaf
        edge = Find-Edge
        ps_version = [string]$PSVersionTable.PSVersion
      })
    }
    'inventory' {
      Write-Result (Get-InventoryResult)
    }
    'mkdir' {
      $full = Assert-UnderBenchDir $Path
      $null = New-Item -ItemType Directory -Path $full -Force
      Write-Result ([ordered]@{ path = $full })
    }
    'cat' {
      $full = Assert-UnderBenchDir $Path
      $bytes = [System.IO.File]::ReadAllBytes($full)
      $stream = [Console]::OpenStandardOutput()
      $stream.Write($bytes, 0, $bytes.Length)
      $stream.Flush()
    }
    'page-start' {
      Write-Result (Start-Page)
    }
    'page-stop' {
      Write-Result ([ordered]@{ killed = Stop-PageProcesses })
    }
    'sample-start' {
      Write-Result (Start-Sample)
    }
    'sample-stop' {
      Write-Result (Stop-Sample)
    }
    'product' {
      if ([string]::IsNullOrEmpty($Id) -or $Id -notmatch '^[a-z]+$') {
        throw 'invalid product id'
      }
      $productPath = Join-Path (Get-BenchDir) ('host\products\' + $Id + '.ps1')
      if (-not (Test-Path -LiteralPath $productPath -PathType Leaf)) {
        throw ('product script not found: ' + $Id)
      }
      . $productPath
      $parameters = Read-BenchParams
      $productResult = @(Invoke-Product -Action $Action -Params $parameters)
      if ($productResult.Count -eq 0) {
        throw 'product script returned no result'
      }
      $productResult = $productResult[$productResult.Count - 1]
      $isHashtable = $productResult -is [System.Collections.IDictionary]
      $isObject = $productResult -is [PSCustomObject]
      if (-not $isHashtable -and -not $isObject) {
        throw 'product script returned no result'
      }
      Write-Result $productResult
    }
    default {
      throw ('unknown command: ' + $Command)
    }
  }
}
catch {
  Write-Result ([ordered]@{ ok = $false; error = $_.Exception.Message })
  exit 1
}
