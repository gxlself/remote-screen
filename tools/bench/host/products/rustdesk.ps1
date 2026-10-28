function Invoke-Product {
  param([string]$Action, $Params)

  $serviceName = 'RustDesk'
  $exe = "$env:ProgramFiles\RustDesk\rustdesk.exe"
  $displayName = 'RustDesk*'
  $port = 21118
  $configCandidates = @(
    'C:\Windows\ServiceProfiles\LocalService\AppData\Roaming\RustDesk\config\RustDesk2.toml',
    'C:\Windows\System32\config\systemprofile\AppData\Roaming\RustDesk\config\RustDesk2.toml'
  )

  switch ($Action) {
    'status' {
      $entry = @(Get-UninstallEntry -NameLike $displayName) | Select-Object -First 1
      $service = Get-ServiceState -Name $serviceName
      $exeExists = Test-Path -LiteralPath $exe -PathType Leaf
      $installed = ($null -ne $entry) -or $exeExists -or ($service -ne 'missing')
      $version = $null
      if ($null -ne $entry) {
        $version = [string]$entry.DisplayVersion
      }
      return [ordered]@{
        installed = [bool]$installed
        version = $version
        service = $service
        exe = [bool]$exeExists
      }
    }
    'version' {
      $status = Invoke-Product -Action 'status' -Params $null
      return [ordered]@{ version = $status.version }
    }
    'install' {
      if ($null -eq $Params -or [string]::IsNullOrWhiteSpace([string]$Params.msi)) {
        throw 'install requires msi'
      }
      $msi = Assert-UnderBenchDir ([string]$Params.msi)
      if (-not (Test-Path -LiteralPath $msi -PathType Leaf)) {
        throw ('msi not found: ' + $msi)
      }
      $expected = [string]$Params.sha256
      if ([string]::IsNullOrWhiteSpace($expected)) {
        throw 'install requires sha256'
      }
      if (-not (Test-Sha256 -Path $msi -Expected $expected)) {
        throw 'sha256 mismatch for RustDesk MSI'
      }
      $logPath = Join-Path (Get-BenchDir) 'downloads\rustdesk-install.log'
      $null = Invoke-Msi -Path $msi -LogPath $logPath
      $present = Wait-Until -TimeoutSec 60 -Condition {
        (Get-ServiceState -Name $serviceName) -ne 'missing'
      }
      if (-not $present) {
        throw 'RustDesk service did not appear after install'
      }
      return Invoke-Product -Action 'status' -Params $null
    }
    'configure' {
      if ($null -eq $Params -or [string]::IsNullOrWhiteSpace([string]$Params.password)) {
        throw 'configure requires password'
      }

      $options = @{}
      if ($null -ne $Params.options) {
        if ($Params.options -is [System.Collections.IDictionary]) {
          foreach ($key in $Params.options.Keys) {
            $options[[string]$key] = [string]$Params.options[$key]
          }
        }
        else {
          foreach ($property in $Params.options.PSObject.Properties) {
            $options[[string]$property.Name] = [string]$property.Value
          }
        }
      }

      $cliOk = $true
      $readback = @{}
      foreach ($key in $options.Keys) {
        $value = [string]$options[$key]
        $setOutput = @(& $exe '--option' $key $value 2>&1)
        $setCode = $LASTEXITCODE
        $setText = ($setOutput | ForEach-Object { [string]$_ }) -join "`n"
        if ($setCode -ne 0 -or $setText -match '(?i)usage') {
          $cliOk = $false
          break
        }

        $getOutput = @(& $exe '--option' $key 2>&1)
        $getCode = $LASTEXITCODE
        $getText = ($getOutput | ForEach-Object { [string]$_ }) -join "`n"
        if ($getCode -ne 0 -or $getText -match '(?i)usage' -or
            ($getText.Trim() -ne $value -and $getText -notmatch [regex]::Escape($value))) {
          $cliOk = $false
          break
        }
        $readback[$key] = $getText.Trim()
      }

      $method = 'cli'
      if (-not $cliOk) {
        $null = Set-ServiceRunning -Name $serviceName -Running $false
        $configPath = $null
        foreach ($candidate in $configCandidates) {
          if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            $configPath = $candidate
            break
          }
        }
        if ($null -eq $configPath) {
          $configPath = $configCandidates[0]
        }
        $configDir = Split-Path -Parent $configPath
        $null = New-Item -ItemType Directory -Path $configDir -Force

        $lines = @()
        if (Test-Path -LiteralPath $configPath -PathType Leaf) {
          $lines = @([System.IO.File]::ReadAllLines($configPath, [System.Text.Encoding]::UTF8))
        }
        $newLines = New-Object 'System.Collections.Generic.List[string]'
        $inOptions = $false
        $seen = @{}
        foreach ($line in $lines) {
          $section = [regex]::Match([string]$line, '^\s*\[([^\]]+)\]\s*$')
          if ($section.Success) {
            if ($inOptions) {
              foreach ($key in $options.Keys) {
                if (-not $seen.ContainsKey($key)) {
                  $escaped = ([string]$options[$key]).Replace("'", "''")
                  $null = $newLines.Add("$key = '$escaped'")
                  $seen[$key] = $true
                }
              }
            }
            $inOptions = $section.Groups[1].Value -eq 'options'
            $null = $newLines.Add([string]$line)
            continue
          }
          if ($inOptions) {
            $assignment = [regex]::Match([string]$line, '^\s*([A-Za-z0-9_-]+)\s*=')
            if ($assignment.Success -and $options.ContainsKey($assignment.Groups[1].Value)) {
              $key = $assignment.Groups[1].Value
              $escaped = ([string]$options[$key]).Replace("'", "''")
              $null = $newLines.Add("$key = '$escaped'")
              $seen[$key] = $true
              continue
            }
          }
          $null = $newLines.Add([string]$line)
        }
        if ($inOptions) {
          foreach ($key in $options.Keys) {
            if (-not $seen.ContainsKey($key)) {
              $escaped = ([string]$options[$key]).Replace("'", "''")
              $null = $newLines.Add("$key = '$escaped'")
              $seen[$key] = $true
            }
          }
        }
        else {
          if ($newLines.Count -gt 0) {
            $null = $newLines.Add('')
          }
          $null = $newLines.Add('[options]')
          foreach ($key in $options.Keys) {
            $escaped = ([string]$options[$key]).Replace("'", "''")
            $null = $newLines.Add("$key = '$escaped'")
            $seen[$key] = $true
          }
        }
        $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
        $toml = $newLines -join [Environment]::NewLine
        [System.IO.File]::WriteAllText($configPath, $toml + [Environment]::NewLine, $utf8NoBom)
        $null = Set-ServiceRunning -Name $serviceName -Running $true
        $method = 'toml'
        $readback = @{}
        foreach ($key in $options.Keys) {
          $readback[$key] = [string]$options[$key]
        }
      }

      $passwordOutput = @(& $exe '--password' ([string]$Params.password) 2>&1)
      $passwordCode = $LASTEXITCODE
      if ($passwordCode -ne 0) {
        throw 'RustDesk password configuration failed'
      }
      $null = Set-ServiceRunning -Name $serviceName -Running $false
      $null = Set-ServiceRunning -Name $serviceName -Running $true
      return [ordered]@{
        configured = $true
        method = $method
        options = $readback
      }
    }
    'start' {
      $null = Set-ServiceRunning -Name $serviceName -Running $true
      $ready = Wait-Until -TimeoutSec 20 -Condition {
        try {
          @(Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction Stop).Count -gt 0
        }
        catch {
          $false
        }
      }
      if (-not $ready) {
        throw 'RustDesk direct access port 21118 did not start listening'
      }
      return Invoke-Product -Action 'status' -Params $null
    }
    'stop' {
      $null = Set-ServiceRunning -Name $serviceName -Running $false
      $processes = @(Get-Process -Name 'rustdesk' -ErrorAction SilentlyContinue)
      foreach ($process in $processes) {
        try {
          Stop-Process -Id ([int]$process.Id) -ErrorAction SilentlyContinue
        }
        catch {
        }
      }
      $ended = Wait-Until -TimeoutSec 5 -Condition {
        @(Get-Process -Name 'rustdesk' -ErrorAction SilentlyContinue).Count -eq 0
      }
      $forced = $false
      if (-not $ended) {
        $remaining = @(Get-Process -Name 'rustdesk' -ErrorAction SilentlyContinue)
        if ($remaining.Count -gt 0) {
          foreach ($process in $remaining) {
            try {
              Stop-Process -Id ([int]$process.Id) -Force -ErrorAction SilentlyContinue
            }
            catch {
            }
          }
          $forced = $true
        }
      }
      return [ordered]@{ stopped = $true; forced = [bool]$forced }
    }
    'uninstall' {
      $null = Invoke-Product -Action 'stop' -Params $null
      $entry = @(Get-UninstallEntry -NameLike $displayName) | Select-Object -First 1
      $uninstalled = $false
      if ($null -ne $entry -and -not [string]::IsNullOrWhiteSpace([string]$entry.ProductCode)) {
        $logPath = Join-Path (Get-BenchDir) 'downloads\rustdesk-uninstall.log'
        $null = Invoke-MsiUninstall -ProductCode ([string]$entry.ProductCode) -LogPath $logPath
        $uninstalled = $true
      }
      $leftoverDirs = @()
      $programDir = "$env:ProgramFiles\RustDesk"
      if (Test-Path -LiteralPath $programDir -PathType Container) {
        $leftoverDirs += $programDir
      }
      foreach ($candidate in $configCandidates) {
        $directory = Split-Path -Parent $candidate
        if (Test-Path -LiteralPath $directory -PathType Container) {
          $leftoverDirs += $directory
        }
      }
      return [ordered]@{ uninstalled = [bool]$uninstalled; leftover_dirs = @($leftoverDirs) }
    }
    default {
      throw ('unsupported RustDesk action: ' + $Action)
    }
  }
}
