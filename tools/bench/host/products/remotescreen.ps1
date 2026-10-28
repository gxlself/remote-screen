function Invoke-Product {
  param([string]$Action, $Params)

  $serviceName = 'RemoteHostService'
  $displayName = 'Remote Host*'
  $installDir = 'C:\Program Files\Remote Host'
  $port = 7400

  function Get-RemoteHostVersion {
    $entries = @(Get-UninstallEntry -NameLike $displayName)
    foreach ($entry in $entries) {
      if (-not [string]::IsNullOrWhiteSpace([string]$entry.DisplayVersion)) {
        return [string]$entry.DisplayVersion
      }
    }
    $exe = Join-Path $installDir 'rs-host.exe'
    if (Test-Path -LiteralPath $exe -PathType Leaf) {
      $value = (Get-Item -LiteralPath $exe -ErrorAction SilentlyContinue).VersionInfo.FileVersion
      if (-not [string]::IsNullOrWhiteSpace([string]$value)) {
        return [string]$value
      }
    }
    return $null
  }

  function Get-RemoteHostStatus {
    $service = Get-ServiceState -Name $serviceName
    return [ordered]@{
      installed = ($service -ne 'missing')
      version = Get-RemoteHostVersion
      service = $service
    }
  }

  switch ($Action) {
    'status' {
      return Get-RemoteHostStatus
    }
    'version' {
      return @{ version = Get-RemoteHostVersion }
    }
    'install' {
      if ($null -eq $Params -or [string]::IsNullOrWhiteSpace([string]$Params.msi)) {
        throw 'install requires msi'
      }
      if ([string]::IsNullOrWhiteSpace([string]$Params.sha256)) {
        throw 'install requires sha256'
      }
      $msi = Assert-UnderBenchDir ([string]$Params.msi)
      if (-not (Test-Path -LiteralPath $msi -PathType Leaf)) {
        throw ('MSI not found: ' + $msi)
      }
      if (-not (Test-Sha256 -Path $msi -Expected ([string]$Params.sha256))) {
        throw 'MSI sha256 mismatch'
      }
      $downloads = Join-Path (Get-BenchDir) 'downloads'
      $null = New-Item -ItemType Directory -Force -Path $downloads
      $log = Join-Path $downloads 'remotescreen-install.log'
      $null = Invoke-Msi -Path $msi -LogPath $log
      return Get-RemoteHostStatus
    }
    'configure' {
      return @{ configured = $true }
    }
    'start' {
      $null = Set-ServiceRunning -Name $serviceName -Running $true
      $listening = Wait-Until -TimeoutSec 15 -Condition {
        $udp = @(Get-NetUDPEndpoint -LocalPort $port -ErrorAction SilentlyContinue)
        $tcp = @(Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue)
        ($udp.Count -gt 0 -or $tcp.Count -gt 0)
      }
      if (-not $listening) {
        throw ('Remote Host did not listen on port ' + $port)
      }
      return @{ service = 'Running'; port = $port }
    }
    'stop' {
      $null = Set-ServiceRunning -Name $serviceName -Running $false
      $stopped = Wait-Until -TimeoutSec 10 -Condition {
        $first = @(Get-Process -Name 'rs-host' -ErrorAction SilentlyContinue)
        $second = @(Get-Process -Name 'rs-host-session' -ErrorAction SilentlyContinue)
        ($first.Count -eq 0 -and $second.Count -eq 0)
      }
      $forced = $false
      if (-not $stopped) {
        Stop-Process -Name 'rs-host', 'rs-host-session' -Force -ErrorAction SilentlyContinue
        $forced = $true
      }
      return @{ service = 'Stopped'; forced = $forced }
    }
    'uninstall' {
      $entries = @(Get-UninstallEntry -NameLike $displayName)
      if ($entries.Count -eq 0) {
        return @{ uninstalled = $false; reason = 'not installed' }
      }
      $productCode = [string]$entries[0].ProductCode
      if ([string]::IsNullOrWhiteSpace($productCode)) {
        throw 'Remote Host uninstall entry has no ProductCode'
      }
      $downloads = Join-Path (Get-BenchDir) 'downloads'
      $null = New-Item -ItemType Directory -Force -Path $downloads
      $log = Join-Path $downloads 'remotescreen-uninstall.log'
      $null = Invoke-MsiUninstall -ProductCode $productCode -LogPath $log
      return @{ uninstalled = $true }
    }
    default {
      throw ('unsupported action: ' + $Action)
    }
  }
}
