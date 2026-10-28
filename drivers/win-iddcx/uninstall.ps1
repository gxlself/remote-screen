[CmdletBinding()]
param(
    [string]$DevCon = 'devcon.exe',
    [switch]$RemoveCert,
    [string]$CertName = 'Remote Screen Test Driver'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

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

function Invoke-BestEffort {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [Parameter(ValueFromRemainingArguments = $true)]
        [string[]]$ArgumentList
    )

    & $FilePath @ArgumentList 2>&1 | ForEach-Object { Write-Host $_ }
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        Write-Host "$FilePath returned $exitCode; the device or package may already be absent."
    }
}

try {
    if (-not (Test-Administrator)) {
        throw 'uninstall.ps1 must be run from an elevated Administrator PowerShell.'
    }

    $devconPath = Resolve-CommandPath $DevCon
    if ($null -ne $devconPath) {
        Write-Host 'Removing Root\RsVirtualDisplay device...'
        Invoke-BestEffort -FilePath $devconPath -ArgumentList @('remove', 'Root\RsVirtualDisplay')
    }
    else {
        Write-Host "devcon.exe was not found at '$DevCon'; continuing because the device may already be absent."
    }

    Write-Host 'Looking for installed RsIdd driver packages...'
    $driverOutput = (& pnputil.exe /enum-drivers 2>&1 | Out-String)
    Write-Host $driverOutput

    $publishedNames = @()
    foreach ($block in ($driverOutput -split '(?m)(?=^\s*Published Name\s*:)')) {
        if ($block -match '(?im)^\s*Original Name\s*:\s*rsidd\.inf\s*$' -and
            $block -match '(?im)^\s*Published Name\s*:\s*(oem\d+\.inf)\s*$') {
            $publishedNames += $Matches[1]
        }
    }
    foreach ($publishedName in ($publishedNames | Sort-Object -Unique)) {
        Write-Host "Deleting driver package $publishedName..."
        Invoke-BestEffort -FilePath 'pnputil.exe' -ArgumentList @('/delete-driver', $publishedName, '/uninstall', '/force')
    }

    if ($RemoveCert) {
        $subject = "CN=$CertName"
        foreach ($storePath in @('Cert:\LocalMachine\Root', 'Cert:\LocalMachine\TrustedPublisher')) {
            $certificates = @(Get-ChildItem -Path $storePath |
                Where-Object { $_.Subject -eq $subject })
            foreach ($certificate in $certificates) {
                Write-Host "Removing $subject from $storePath"
                Remove-Item -LiteralPath $certificate.PSPath -Force -ErrorAction SilentlyContinue
            }
        }
    }

    Write-Host 'UNINSTALLED'
}
catch {
    Write-Error $_
    exit 1
}
