[CmdletBinding()]
param(
    [ValidateSet('Release', 'Debug')]
    [string]$Configuration = 'Release',
    [string]$CertName = 'Remote Screen Test Driver',
    [string]$Version = '0.5.0.0',
    [string]$TimestampServer = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-Native {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [Parameter(ValueFromRemainingArguments = $true)]
        [string[]]$ArgumentList
    )

    & $FilePath @ArgumentList
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath failed with exit code $LASTEXITCODE"
    }
}

try {
    $solution = Join-Path $PSScriptRoot 'RsIdd.sln'
    $driverRoot = Join-Path $PSScriptRoot 'RsIdd'
    $buildDll = Join-Path $driverRoot (Join-Path "x64\$Configuration" 'RsIdd.dll')
    $sourceInf = Join-Path $driverRoot 'RsIdd.inf'
    $outRoot = Join-Path $PSScriptRoot 'out'
    $packageDir = Join-Path $outRoot 'RsIdd'
    $packageDll = Join-Path $packageDir 'RsIdd.dll'
    $packageInf = Join-Path $packageDir 'RsIdd.inf'
    $packageCat = Join-Path $packageDir 'RsIdd.cat'
    $cerPath = Join-Path $outRoot 'RsIdd-test.cer'

    if (-not (Test-Path -LiteralPath $solution)) {
        throw "Solution not found: $solution"
    }

    Write-Host "Building $Configuration x64..."
    Invoke-Native -FilePath 'msbuild' -ArgumentList @(
        $solution
        "/p:Configuration=$Configuration"
        '/p:Platform=x64'
        '/m'
    )

    if (-not (Test-Path -LiteralPath $buildDll)) {
        throw "Build output not found: $buildDll"
    }

    New-Item -ItemType Directory -Force -Path $packageDir | Out-Null
    Copy-Item -LiteralPath $buildDll -Destination $packageDll -Force
    Copy-Item -LiteralPath $sourceInf -Destination $packageInf -Force

    Write-Host 'Stamping driver metadata...'
    # The UMDF version must match the WDK used on the Windows build machine.
    Invoke-Native -FilePath 'stampinf' -ArgumentList @(
        '-f'
        $packageInf
        '-d'
        '*'
        '-v'
        $Version
        '-u'
        '2.15.0'
    )

    Write-Host 'Generating catalog...'
    Invoke-Native -FilePath 'Inf2Cat' -ArgumentList @(
        "/driver:$packageDir"
        '/os:10_X64'
    )
    if (-not (Test-Path -LiteralPath $packageCat)) {
        throw "Catalog not found: $packageCat"
    }

    $subject = "CN=$CertName"
    $certificate = Get-ChildItem -Path 'Cert:\CurrentUser\My' |
        Where-Object {
            $_.Subject -eq $subject -and
            $_.HasPrivateKey -and
            $_.NotAfter -gt (Get-Date)
        } |
        Sort-Object -Property NotAfter -Descending |
        Select-Object -First 1

    if ($null -eq $certificate) {
        Write-Host "Creating test code-signing certificate: $subject"
        $certificate = New-SelfSignedCertificate -Type CodeSigningCert -Subject $subject `
            -CertStoreLocation 'Cert:\CurrentUser\My' -KeyExportPolicy NonExportable
    }

    if (Test-Path -LiteralPath $cerPath) {
        Remove-Item -LiteralPath $cerPath -Force
    }
    Export-Certificate -Cert $certificate -FilePath $cerPath -Type CERT | Out-Null

    $thumbprint = ($certificate.Thumbprint -replace '\s', '')
    $signArguments = @('/sign', '/fd', 'SHA256', '/sha1', $thumbprint)
    if (-not [string]::IsNullOrWhiteSpace($TimestampServer)) {
        $signArguments += @('/tr', $TimestampServer, '/td', 'SHA256')
    }
    $signArguments += @($packageDll, $packageCat)

    Write-Host 'Signing driver and catalog...'
    Invoke-Native -FilePath 'signtool' -ArgumentList $signArguments

    Write-Host "RsIdd.dll: $packageDll"
    Write-Host "RsIdd.inf: $packageInf"
    Write-Host "RsIdd.cat: $packageCat"
    Write-Host "Test certificate: $cerPath"
    Write-Host "Certificate thumbprint: $thumbprint"
}
catch {
    Write-Error $_
    exit 1
}
