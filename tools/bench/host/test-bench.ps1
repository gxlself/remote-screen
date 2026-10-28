$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
. (Join-Path $PSScriptRoot 'common.ps1')

$script:PassCount = 0
$script:FailCount = 0

function Quote-WindowsArgument {
  param([string]$Value)
  return '"' + $Value.Replace('"', '\"') + '"'
}

function Invoke-BenchChild {
  param([Parameter(Mandatory)] [string[]]$Arguments)

  $startInfo = New-Object System.Diagnostics.ProcessStartInfo
  $startInfo.FileName = 'powershell.exe'
  $argumentText = '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File ' +
    (Quote-WindowsArgument (Join-Path $PSScriptRoot 'bench.ps1'))
  foreach ($argument in $Arguments) {
    $argumentText += ' ' + (Quote-WindowsArgument $argument)
  }
  $startInfo.Arguments = $argumentText
  $startInfo.UseShellExecute = $false
  $startInfo.CreateNoWindow = $true
  $startInfo.RedirectStandardOutput = $true
  $startInfo.RedirectStandardError = $true
  $startInfo.StandardOutputEncoding = New-Object System.Text.UTF8Encoding($false)
  $startInfo.StandardErrorEncoding = New-Object System.Text.UTF8Encoding($false)
  $process = New-Object System.Diagnostics.Process
  $process.StartInfo = $startInfo
  $null = $process.Start()
  $stdout = $process.StandardOutput.ReadToEnd()
  $stderr = $process.StandardError.ReadToEnd()
  $process.WaitForExit()
  return [pscustomobject]@{
    Code = [int]$process.ExitCode
    Out = $stdout
    Err = $stderr
  }
}

function Read-ChildJson {
  param(
    [Parameter(Mandatory)] $Result,
    [bool]$RequireSuccess = $true
  )

  $text = $Result.Out.TrimEnd([char[]]"`r`n")
  if ([string]::IsNullOrEmpty($text) -or $text.Contains("`r") -or $text.Contains("`n")) {
    throw ('expected one JSON line, got stdout: ' + $Result.Out)
  }
  $value = $text | ConvertFrom-Json
  if ($RequireSuccess -and ($Result.Code -ne 0 -or -not [bool]$value.ok)) {
    throw ('child command failed: ' + $Result.Err)
  }
  return $value
}

function Require-Fields {
  param(
    [Parameter(Mandatory)] $Object,
    [Parameter(Mandatory)] [string[]]$Names
  )
  foreach ($name in $Names) {
    if ($null -eq $Object.PSObject.Properties[$name]) {
      throw ('missing field: ' + $name)
    }
  }
}

function Pass-Check {
  param([string]$Name)
  $script:PassCount++
  [Console]::Out.WriteLine('PASS ' + $Name)
}

function Fail-Check {
  param([string]$Name, [string]$Message)
  $script:FailCount++
  [Console]::Out.WriteLine('FAIL {0}: {1}' -f $Name, $Message)
}

function Run-Check {
  param([string]$Name, [scriptblock]$Body)
  try {
    & $Body
    Pass-Check $Name
  }
  catch {
    Fail-Check $Name $_.Exception.Message
  }
}

$bench = Get-BenchDir
$selftest = Join-Path $bench 'runs\selftest'
$catPath = Join-Path $selftest 'a.txt'
$samplePath = Join-Path $selftest 'hs.jsonl'

Run-Check 'hostinfo' {
  $result = Invoke-BenchChild -Arguments @('hostinfo')
  $json = Read-ChildJson $result
  Require-Fields $json @('hostname', 'os_version', 'os_caption', 'cpu', 'logical_cpus', 'gpus', 'display')
}

Run-Check 'env' {
  $result = Invoke-BenchChild -Arguments @('env')
  $json = Read-ChildJson $result
  Require-Fields $json @('bench_dir', 'rs_bench_exe', 'page', 'edge', 'ps_version')
}

Run-Check 'session' {
  $result = Invoke-BenchChild -Arguments @('session')
  $json = Read-ChildJson $result
  Require-Fields $json @('console_session_id', 'logged_in', 'locked', 'user', 'rdp')
}

Run-Check 'mkdir' {
  $result = Invoke-BenchChild -Arguments @('mkdir', '-Path', $selftest)
  $json = Read-ChildJson $result
  Require-Fields $json @('path')
  if (-not (Test-Path -LiteralPath $selftest -PathType Container)) {
    throw 'selftest directory was not created'
  }
}

Run-Check 'mkdir-outside' {
  $result = Invoke-BenchChild -Arguments @('mkdir', '-Path', 'C:\Windows\Temp\x')
  $json = Read-ChildJson $result -RequireSuccess $false
  if ($result.Code -ne 1 -or [bool]$json.ok) {
    throw 'outside path was accepted'
  }
}

Run-Check 'cat' {
  [System.IO.File]::WriteAllText($catPath, "x`ny", (New-Object System.Text.UTF8Encoding($false)))
  $result = Invoke-BenchChild -Arguments @('cat', '-Path', $catPath)
  if ($result.Code -ne 0) {
    throw ('cat failed: ' + $result.Err)
  }
  $expected = [System.Text.Encoding]::UTF8.GetString([System.IO.File]::ReadAllBytes($catPath))
  if ($result.Out -cne $expected) {
    throw 'cat bytes differ'
  }
}

Run-Check 'inventory' {
  $result = Invoke-BenchChild -Arguments @('inventory')
  $json = Read-ChildJson $result
  Require-Fields $json @('services', 'programs', 'firewall_rules', 'drivers')
  foreach ($name in @('services', 'programs', 'firewall_rules', 'drivers')) {
    if (@($json.$name).Count -eq 0) {
      throw ($name + ' is empty')
    }
  }
}

Run-Check 'sample' {
  $result = Invoke-BenchChild -Arguments @(
    'sample-start', '-Names', 'explorer', '-Seconds', '30',
    '-RemoteAddress', '127.0.0.1', '-Out', $samplePath
  )
  $null = Read-ChildJson $result
  Start-Sleep -Seconds 3
  $stopResult = Invoke-BenchChild -Arguments @('sample-stop', '-Out', $samplePath)
  $stopJson = Read-ChildJson $stopResult
  if ([int]$stopJson.lines -lt 3) {
    throw ('expected at least 3 lines, got ' + $stopJson.lines)
  }
  $lines = [System.IO.File]::ReadAllLines($samplePath)
  $last = $lines[$lines.Count - 1] | ConvertFrom-Json
  if ([string]$last.reason -ne 'stop-file') {
    throw ('unexpected stop reason: ' + $last.reason)
  }
}

Run-Check 'product-nosuch' {
  $result = Invoke-BenchChild -Arguments @('product', '-Id', 'nosuch', '-Action', 'status')
  $json = Read-ChildJson $result -RequireSuccess $false
  if ($result.Code -ne 1 -or [bool]$json.ok) {
    throw 'missing product did not fail'
  }
}

try {
  $sessionResult = Invoke-BenchChild -Arguments @('session')
  $session = Read-ChildJson $sessionResult
  if ([bool]$session.logged_in -and -not [bool]$session.locked) {
    try {
      $pageResult = Invoke-BenchChild -Arguments @('page-start', '-Scene', 'calib', '-Seconds', '5')
      $page = Read-ChildJson $pageResult
      if ([int]$page.pid -le 0) {
        throw 'page did not return a pid'
      }
      $stopResult = Invoke-BenchChild -Arguments @('page-stop')
      $stop = Read-ChildJson $stopResult
      if ([int]$stop.killed -lt 1) {
        throw 'page-stop did not kill Edge'
      }
      Pass-Check 'page'
    }
    catch {
      Fail-Check 'page' $_.Exception.Message
    }
  }
  else {
    Fail-Check 'page' 'console session not available'
  }
}
catch {
  Fail-Check 'page' $_.Exception.Message
}

if (Test-Path -LiteralPath $selftest) {
  Remove-Item -LiteralPath $selftest -Recurse -Force
}

[Console]::Out.WriteLine('PASS {0} / FAIL {1}' -f $script:PassCount, $script:FailCount)
if ($script:FailCount -gt 0) {
  exit 1
}
exit 0
