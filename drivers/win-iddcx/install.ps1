[CmdletBinding()]
param(
    [string]$PackageDir = (Join-Path $PSScriptRoot 'out\RsIdd'),
    [string]$Cer = (Join-Path $PSScriptRoot 'out\RsIdd-test.cer'),
    [string]$DevCon = 'devcon.exe'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$lastPnpOutput = ''

function Test-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object -TypeName Security.Principal.WindowsPrincipal -ArgumentList $identity
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Resolve-CommandPath {
    param([Parameter(Mandatory = $true)][string]$Name)

    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($null -ne $command) {
        return $command.Source
    }
    if ([IO.Path]::IsPathRooted($Name) -and (Test-Path -LiteralPath $Name)) {
        return $Name
    }
    return $null
}

try {
    if (-not (Test-Administrator)) {
        throw 'install.ps1 must be run from an elevated Administrator PowerShell.'
    }

    $infPath = Join-Path $PackageDir 'RsIdd.inf'
    if (-not (Test-Path -LiteralPath $infPath)) {
        throw "Driver INF not found: $infPath"
    }
    if (-not (Test-Path -LiteralPath $Cer)) {
        throw "Test certificate not found: $Cer"
    }

    $devconPath = Resolve-CommandPath $DevCon
    if ($null -eq $devconPath) {
        throw "devcon.exe was not found in PATH or at '$DevCon'."
    }

    $certificate = New-Object -TypeName System.Security.Cryptography.X509Certificates.X509Certificate2 -ArgumentList $Cer
    $thumbprint = ($certificate.Thumbprint -replace '\s', '')
    foreach ($storePath in @('Cert:\LocalMachine\Root', 'Cert:\LocalMachine\TrustedPublisher')) {
        $existing = Get-ChildItem -Path $storePath |
            Where-Object { ($_.Thumbprint -replace '\s', '') -eq $thumbprint } |
            Select-Object -First 1
        if ($null -eq $existing) {
            Write-Host "Importing test certificate into $storePath"
            Import-Certificate -FilePath $Cer -CertStoreLocation $storePath | Out-Null
        }
        else {
            Write-Host "Certificate already present in $storePath"
        }
    }

    Write-Host 'Staging and installing driver package...'
    $lastPnpOutput = (& pnputil.exe /add-driver $infPath /install 2>&1 | Out-String)
    $pnpExitCode = $LASTEXITCODE
    Write-Host $lastPnpOutput
    if ($pnpExitCode -ne 0) {
        throw "pnputil /add-driver failed with exit code $pnpExitCode"
    }

    $deviceOutput = (& pnputil.exe /enum-devices /deviceid 'Root\RsVirtualDisplay' 2>&1 | Out-String)
    $deviceExitCode = $LASTEXITCODE
    Write-Host $deviceOutput
    $devicePresent = $deviceOutput -match '(?i)Root\\RsVirtualDisplay'
    if (-not $devicePresent) {
        Write-Host 'Creating the Root device with devcon...'
        & $devconPath install $infPath 'Root\RsVirtualDisplay'
        if ($LASTEXITCODE -ne 0) {
            throw "devcon install failed with exit code $LASTEXITCODE"
        }
    }

    $deadline = (Get-Date).AddSeconds(10)
    $started = $false
    do {
        $statusOutput = (& pnputil.exe /enum-devices /deviceid 'Root\RsVirtualDisplay' /properties 2>&1 | Out-String)
        $statusExitCode = $LASTEXITCODE
        Write-Host $statusOutput
        $started = $statusOutput -match '(?im)Status\s*:\s*Started'

        if (-not $started) {
            $devices = @(Get-PnpDevice -InstanceId 'ROOT\RSVIRTUALDISPLAY*' -ErrorAction SilentlyContinue)
            $started = @($devices | Where-Object { $_.Status -eq 'OK' }).Count -gt 0
        }

        if (-not $started -and (Get-Date) -lt $deadline) {
            Start-Sleep -Seconds 1
        }
    } while (-not $started -and (Get-Date) -lt $deadline)

    if (-not $started) {
        throw 'Root\RsVirtualDisplay did not reach Started/OK within 10 seconds.'
    }

    Write-Host 'INSTALLED'
}
catch {
    Write-Error $_
    if ($lastPnpOutput) {
        Write-Host 'Last pnputil output:'
        Write-Host $lastPnpOutput
    }
    Write-Host 'If installation still fails, test-signing mode may be required:'
    Write-Host '  bcdedit /set testsigning on'
    Write-Host 'Disable Secure Boot before enabling it and reboot. This script never runs bcdedit automatically.'
    exit 1
}
