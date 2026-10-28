# Shared functions for the Windows benchmark host scripts.
# Product scripts are dot-sourced by bench.ps1 and must only define
# function Invoke-Product { param([string]$Action, $Params) ... }.
# Supported actions are status, version, install, configure, start, stop,
# and uninstall. The moonlight product may also support pin. A product
# function must return one hashtable, write no stdout, and contain ASCII only.

function Get-BenchDir {
  $value = $env:RS_BENCH_DIR
  if ([string]::IsNullOrWhiteSpace($value)) {
    $value = 'C:\rs-bench'
  }
  return [System.IO.Path]::GetFullPath($value)
}

function Write-Log {
  param([string]$Message)
  [Console]::Error.WriteLine('[rs-bench] ' + $Message)
}

function Write-Result {
  param([object]$Object)

  if ($null -eq $Object) {
    $Object = [ordered]@{}
  }
  $hasOk = $false
  if ($Object -is [System.Collections.IDictionary]) {
    $hasOk = $Object.Contains('ok')
    if (-not $hasOk) {
      $Object['ok'] = $true
    }
  }
  else {
    $property = $Object.PSObject.Properties['ok']
    if ($null -eq $property) {
      $Object | Add-Member -MemberType NoteProperty -Name ok -Value $true
    }
  }
  $json = $Object | ConvertTo-Json -Compress -Depth 6
  [Console]::Out.Write($json + "`n")
}

function Read-BenchParams {
  if (-not [Console]::IsInputRedirected) {
    return $null
  }
  $text = [Console]::In.ReadToEnd()
  if ([string]::IsNullOrWhiteSpace($text)) {
    return $null
  }
  return $text | ConvertFrom-Json
}

function Assert-UnderBenchDir {
  param([string]$Path)

  $full = [System.IO.Path]::GetFullPath($Path)
  $root = [System.IO.Path]::GetFullPath((Get-BenchDir)).TrimEnd([char[]]'\/')
  $prefix = $root + [System.IO.Path]::DirectorySeparatorChar
  if (-not $full.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw ('path outside bench dir: ' + $full)
  }
  return $full
}

function Test-Sha256 {
  param(
    [Parameter(Mandatory)] [string]$Path,
    [Parameter(Mandatory)] [string]$Expected
  )
  $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256 -ErrorAction Stop).Hash
  return $actual.Equals($Expected, [System.StringComparison]::OrdinalIgnoreCase)
}

function Invoke-Msi {
  param(
    [Parameter(Mandatory)] [string]$Path,
    [Parameter(Mandatory)] [string]$LogPath,
    [hashtable]$Properties = @{}
  )

  $arguments = @('/i', ('"' + $Path + '"'), '/qn', '/norestart', '/L*v', ('"' + $LogPath + '"'))
  foreach ($key in $Properties.Keys) {
    $arguments += ([string]$key + '=' + [string]$Properties[$key])
  }
  $process = Start-Process -FilePath 'msiexec.exe' -ArgumentList ($arguments -join ' ') -Wait -PassThru -WindowStyle Hidden
  $code = [int]$process.ExitCode
  if ($code -eq 0 -or $code -eq 3010) {
    return $code
  }
  $tail = @()
  if (Test-Path -LiteralPath $LogPath) {
    $tail = @(Get-Content -LiteralPath $LogPath -Tail 20 -ErrorAction SilentlyContinue)
  }
  throw ('msiexec exit code {0}; log tail: {1}' -f $code, ($tail -join "`n"))
}

function Invoke-MsiUninstall {
  param(
    [Parameter(Mandatory)] [string]$ProductCode,
    [Parameter(Mandatory)] [string]$LogPath
  )

  $arguments = @('/x', $ProductCode, '/qn', '/norestart', '/L*v', ('"' + $LogPath + '"'))
  $process = Start-Process -FilePath 'msiexec.exe' -ArgumentList ($arguments -join ' ') -Wait -PassThru -WindowStyle Hidden
  $code = [int]$process.ExitCode
  if ($code -eq 0 -or $code -eq 3010) {
    return $code
  }
  $tail = @()
  if (Test-Path -LiteralPath $LogPath) {
    $tail = @(Get-Content -LiteralPath $LogPath -Tail 20 -ErrorAction SilentlyContinue)
  }
  throw ('msiexec exit code {0}; log tail: {1}' -f $code, ($tail -join "`n"))
}

function Get-UninstallEntry {
  param([Parameter(Mandatory)] [string]$NameLike)

  $roots = @(
    'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
    'HKLM:\Software\Wow6432Node\Microsoft\Windows\CurrentVersion\Uninstall',
    'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
  )
  $entries = @()
  foreach ($root in $roots) {
    if (-not (Test-Path -LiteralPath $root)) {
      continue
    }
    foreach ($key in @(Get-ChildItem -LiteralPath $root -ErrorAction SilentlyContinue)) {
      $value = Get-ItemProperty -LiteralPath $key.PSPath -ErrorAction SilentlyContinue
      if ($null -eq $value -or [string]::IsNullOrEmpty([string]$value.DisplayName)) {
        continue
      }
      if (-not ([string]$value.DisplayName -like $NameLike)) {
        continue
      }
      $entries += [ordered]@{
        DisplayName = [string]$value.DisplayName
        DisplayVersion = [string]$value.DisplayVersion
        UninstallString = [string]$value.UninstallString
        QuietUninstallString = [string]$value.QuietUninstallString
        ProductCode = [string]$key.PSChildName
      }
    }
  }
  return @($entries)
}

function Start-Detached {
  param([Parameter(Mandatory)] [string]$CommandLine)

  $result = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $CommandLine }
  if ([int]$result.ReturnValue -ne 0) {
    throw ('Win32_Process.Create failed with return code {0}' -f $result.ReturnValue)
  }
  return [int]$result.ProcessId
}

function Start-Interactive {
  param(
    [Parameter(Mandatory)] [string]$Name,
    [Parameter(Mandatory)] [string]$CommandLine
  )

  $tasksDir = Join-Path (Get-BenchDir) 'host\tasks'
  $null = New-Item -ItemType Directory -Path $tasksDir -Force
  $cmdPath = Join-Path $tasksDir ($Name + '.cmd')
  $content = "@echo off`r`n$CommandLine`r`n"
  [System.IO.File]::WriteAllText($cmdPath, $content, (New-Object System.Text.ASCIIEncoding))
  $taskName = 'rs-bench\' + $Name

  $createOutput = @(& schtasks.exe /create /tn $taskName /tr $cmdPath /sc once /st 00:00 /it /f 2>&1)
  $createCode = $LASTEXITCODE
  if ($createCode -ne 0) {
    Write-Log ($createOutput -join "`n")
    throw ('schtasks create failed with exit code {0}' -f $createCode)
  }
  $runOutput = @(& schtasks.exe /run /tn $taskName 2>&1)
  $runCode = $LASTEXITCODE
  if ($runCode -ne 0) {
    Write-Log ($runOutput -join "`n")
    throw ('schtasks run failed with exit code {0}' -f $runCode)
  }
  return $cmdPath
}

function Stop-Interactive {
  param([Parameter(Mandatory)] [string]$Name)

  $taskName = 'rs-bench\' + $Name
  $output = @(& schtasks.exe /delete /tn $taskName /f 2>&1)
  $code = $LASTEXITCODE
  if ($code -eq 0) {
    return $true
  }
  $message = ($output -join "`n")
  if ($message -match '(?i)(cannot find|does not exist|could not be found|no such)') {
    return $false
  }
  Write-Log $message
  throw ('schtasks delete failed with exit code {0}' -f $code)
}

function Wait-Until {
  param(
    [Parameter(Mandatory)] [int]$TimeoutSec,
    [Parameter(Mandatory)] [scriptblock]$Condition,
    [int]$IntervalMs = 200
  )

  $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSec)
  do {
    $value = & $Condition
    if ($value) {
      return $true
    }
    if ([DateTime]::UtcNow -ge $deadline) {
      break
    }
    Start-Sleep -Milliseconds $IntervalMs
  } while ($true)
  return $false
}

function Get-ServiceState {
  param([Parameter(Mandatory)] [string]$Name)

  try {
    $service = Get-Service -Name $Name -ErrorAction Stop
    return $service.Status.ToString()
  }
  catch [System.InvalidOperationException] {
    return 'missing'
  }
  catch [Microsoft.PowerShell.Commands.ServiceCommandException] {
    return 'missing'
  }
}

function Set-ServiceRunning {
  param(
    [Parameter(Mandatory)] [string]$Name,
    [Parameter(Mandatory)] [bool]$Running,
    [int]$TimeoutSec = 30
  )

  $state = Get-ServiceState -Name $Name
  if ($state -eq 'missing') {
    throw ('service not found: ' + $Name)
  }
  if ($Running) {
    if ($state -ne 'Running') {
      Start-Service -Name $Name -ErrorAction Stop
    }
    $wanted = 'Running'
  }
  else {
    if ($state -ne 'Stopped') {
      Stop-Service -Name $Name -Force -ErrorAction Stop
    }
    $wanted = 'Stopped'
  }
  $ready = Wait-Until -TimeoutSec $TimeoutSec -Condition { (Get-ServiceState -Name $Name) -eq $wanted }
  if (-not $ready) {
    throw ('service {0} did not reach {1}' -f $Name, $wanted)
  }
  return $wanted
}

function Find-Edge {
  $roots = @(${env:ProgramFiles(x86)}, $env:ProgramFiles)
  foreach ($root in $roots) {
    if ([string]::IsNullOrWhiteSpace($root)) {
      continue
    }
    $path = Join-Path $root 'Microsoft\Edge\Application\msedge.exe'
    if (Test-Path -LiteralPath $path -PathType Leaf) {
      return [System.IO.Path]::GetFullPath($path)
    }
  }
  return $null
}

function Get-ConsoleSession {
  if (-not ('RsBench.NativeMethods' -as [type])) {
    $source = @'
using System;
using System.Runtime.InteropServices;
namespace RsBench {
  public static class NativeMethods {
    [DllImport("kernel32.dll")]
    public static extern uint WTSGetActiveConsoleSessionId();
  }
}
'@
    $null = Add-Type -TypeDefinition $source -ErrorAction Stop
  }
  return [int][RsBench.NativeMethods]::WTSGetActiveConsoleSessionId()
}

function Get-ProcessByCommandLine {
  param(
    [Parameter(Mandatory)] [string]$Name,
    [Parameter(Mandatory)] [string]$Contains
  )

  $escapedName = $Name.Replace("'", "''")
  $processes = @(Get-CimInstance -ClassName Win32_Process -Filter ("Name = '{0}'" -f $escapedName) -ErrorAction SilentlyContinue)
  return @(
    $processes | Where-Object {
      $null -ne $_.CommandLine -and ([string]$_.CommandLine).IndexOf($Contains, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
    }
  )
}
