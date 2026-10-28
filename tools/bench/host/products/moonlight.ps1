# Sunshine host actions used by the Moonlight benchmark adapter.
# This file is dot-sourced by bench.ps1; it intentionally defines one entry point.
function Invoke-Product {
  param([string]$Action, $Params)

  $ServiceName = 'SunshineService'
  $InstallDir = "$env:ProgramFiles\Sunshine"
  $SunshineExe = Join-Path $InstallDir 'sunshine.exe'
  $UninstallName = 'Sunshine*'
  $TcpPort = 47989
  $WebUri = 'https://localhost:47990'

  $readStatus = {
    $installed = Test-Path -LiteralPath $SunshineExe -PathType Leaf
    $version = $null
    $entries = @(Get-UninstallEntry -NameLike $UninstallName)
    if ($entries.Count -gt 0 -and -not [string]::IsNullOrEmpty([string]$entries[0].DisplayVersion)) {
      $version = [string]$entries[0].DisplayVersion
    }
    elseif ($installed) {
      try {
        $version = [string](Get-Item -LiteralPath $SunshineExe -ErrorAction Stop).VersionInfo.ProductVersion
      }
      catch {
        $version = $null
      }
    }

    $paired = $null
    $statePath = Join-Path $InstallDir 'config\sunshine_state.json'
    if (Test-Path -LiteralPath $statePath -PathType Leaf) {
      try {
        $state = Get-Content -LiteralPath $statePath -Raw -ErrorAction Stop | ConvertFrom-Json
        if ($null -ne $state.root -and $null -ne $state.root.named_devices) {
          $devices = $state.root.named_devices
          if ($devices -is [System.Collections.IDictionary]) {
            $paired = @($devices.Keys).Count
          }
          elseif ($devices -is [System.Array]) {
            $paired = @($devices).Count
          }
          else {
            $paired = @($devices.PSObject.Properties).Count
          }
        }
      }
      catch {
        $paired = $null
      }
    }

    return [ordered]@{
      installed = [bool]$installed
      version = $version
      service = Get-ServiceState -Name $ServiceName
      paired_clients = $paired
    }
  }

  $stopSunshine = {
    $null = Set-ServiceRunning -Name $ServiceName -Running $false
    $processes = @(Get-Process -Name 'sunshine' -ErrorAction SilentlyContinue)
    foreach ($process in $processes) {
      Stop-Process -Id ([int]$process.Id) -ErrorAction SilentlyContinue
    }
    $ended = Wait-Until -TimeoutSec 5 -Condition {
      @(Get-Process -Name 'sunshine' -ErrorAction SilentlyContinue).Count -eq 0
    }
    $forced = $false
    if (-not $ended) {
      $forced = $true
      foreach ($process in @(Get-Process -Name 'sunshine' -ErrorAction SilentlyContinue)) {
        Stop-Process -Id ([int]$process.Id) -Force -ErrorAction SilentlyContinue
      }
    }
    return [ordered]@{ forced = [bool]$forced }
  }

  switch ($Action) {
    'status' {
      return (& $readStatus)
    }
    'version' {
      $status = & $readStatus
      return [ordered]@{ version = $status.version }
    }
    'install' {
      if ($null -eq $Params -or [string]::IsNullOrEmpty([string]$Params.msi) -or
          [string]::IsNullOrEmpty([string]$Params.sha256)) {
        throw 'install requires msi and sha256'
      }
      $msi = [string]$Params.msi
      $sha256 = [string]$Params.sha256
      if (-not (Test-Path -LiteralPath $msi -PathType Leaf)) {
        throw ('MSI not found: ' + $msi)
      }
      if (-not (Test-Sha256 -Path $msi -Expected $sha256)) {
        throw 'Sunshine MSI sha256 mismatch'
      }
      $logPath = Join-Path (Get-BenchDir) 'downloads\sunshine-install.log'
      $null = New-Item -ItemType Directory -Path (Split-Path -Parent $logPath) -Force
      $null = Invoke-Msi -Path $msi -LogPath $logPath
      $ready = Wait-Until -TimeoutSec 60 -Condition {
        (Get-ServiceState -Name $ServiceName) -ne 'missing'
      }
      if (-not $ready) {
        throw 'Sunshine service did not appear after installation'
      }
      return (& $readStatus)
    }
    'configure' {
      if ($null -eq $Params -or [string]::IsNullOrEmpty([string]$Params.user) -or
          [string]::IsNullOrEmpty([string]$Params.password)) {
        throw 'configure requires user and password'
      }
      if (-not (Test-Path -LiteralPath $SunshineExe -PathType Leaf)) {
        throw 'Sunshine executable not found'
      }
      $exitCode = 1
      Push-Location -LiteralPath $InstallDir
      try {
        & '.\sunshine.exe' '--creds' ([string]$Params.user) ([string]$Params.password) *> $null
        $exitCode = [int]$LASTEXITCODE
      }
      finally {
        Pop-Location
      }
      if ($exitCode -ne 0) {
        throw 'Sunshine credential setup failed'
      }
      return [ordered]@{ configured = $true }
    }
    'start' {
      $null = Set-ServiceRunning -Name $ServiceName -Running $true
      $ready = Wait-Until -TimeoutSec 30 -Condition {
        $client = New-Object System.Net.Sockets.TcpClient
        try {
          $client.Connect('127.0.0.1', $TcpPort)
          return $true
        }
        catch {
          return $false
        }
        finally {
          $client.Dispose()
        }
      }
      if (-not $ready) {
        throw ('Sunshine did not listen on TCP ' + $TcpPort)
      }
      return [ordered]@{ started = $true; port = $TcpPort }
    }
    'stop' {
      return (& $stopSunshine)
    }
    'pin' {
      if ($null -eq $Params -or [string]::IsNullOrEmpty([string]$Params.pin) -or
          [string]::IsNullOrEmpty([string]$Params.name) -or
          [string]::IsNullOrEmpty([string]$Params.user) -or
          [string]::IsNullOrEmpty([string]$Params.password)) {
        throw 'pin requires pin, name, user, and password'
      }
      $body = [ordered]@{ pin = [string]$Params.pin; name = [string]$Params.name } |
        ConvertTo-Json -Compress
      $credentials = '{0}:{1}' -f [string]$Params.user, [string]$Params.password
      $credentialBytes = [System.Text.Encoding]::ASCII.GetBytes($credentials)
      $authorization = 'Basic ' + [Convert]::ToBase64String($credentialBytes)
      $headers = @{ Authorization = $authorization }
      $oldCallback = [System.Net.ServicePointManager]::ServerCertificateValidationCallback
      $oldProtocol = [System.Net.ServicePointManager]::SecurityProtocol
      try {
        [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.SecurityProtocolType]::Tls12
        [System.Net.ServicePointManager]::ServerCertificateValidationCallback = { $true }
        $reply = Invoke-RestMethod -Method Post -Uri ($WebUri + '/api/pin') -Headers $headers -ContentType 'application/json' -Body $body -ErrorAction Stop
        return [ordered]@{ status = $reply.status }
      }
      finally {
        [System.Net.ServicePointManager]::ServerCertificateValidationCallback = $oldCallback
        [System.Net.ServicePointManager]::SecurityProtocol = $oldProtocol
      }
    }
    'uninstall' {
      $null = & $stopSunshine
      $entries = @(Get-UninstallEntry -NameLike $UninstallName)
      if ($entries.Count -eq 0) {
        throw 'Sunshine uninstall entry not found'
      }
      $entry = $entries[0]
      $logPath = Join-Path (Get-BenchDir) 'downloads\sunshine-uninstall.log'
      $null = New-Item -ItemType Directory -Path (Split-Path -Parent $logPath) -Force
      $null = Invoke-MsiUninstall -ProductCode ([string]$entry.ProductCode) -LogPath $logPath

      $vigem = @(
        Get-CimInstance -ClassName Win32_SystemDriver -ErrorAction SilentlyContinue |
          Where-Object { [string]$_.Name -like 'ViGEmBus*' }
      ).Count -gt 0
      $leftovers = @()
      if (Test-Path -LiteralPath $InstallDir) {
        $leftovers += $InstallDir
      }
      return [ordered]@{
        uninstalled = -not (Test-Path -LiteralPath $SunshineExe -PathType Leaf)
        vigem_present = [bool]$vigem
        leftover_dirs = @($leftovers)
      }
    }
    default {
      throw ('unsupported action: ' + $Action)
    }
  }
}
