<#
EV signing notes:
* A USB EV token is selected with RS_WIN_SIGN_MODE=thumbprint and
  RS_WIN_SIGN_THUMBPRINT. The token vendor client owns PIN entry in the
  release-machine session; this script never handles that interaction.
* A cloud/HSM provider is selected with RS_WIN_SIGN_MODE=dlib,
  RS_WIN_SIGN_DLIB, and RS_WIN_SIGN_METADATA. Provider credentials stay in
  its recommended environment or managed identity.
* The caller signs DLLs and EXEs first, then the MSI, then the detached Burn
  engine, and finally the Burn package. Every signature receives an RFC3161
  timestamp and is checked with signtool verify.
* Certificate and credential storage follows the existing RN build secret
  management described in the release documentation.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string[]]$Files,
    [Parameter(Mandatory = $true)]
    [string]$Description
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Find-SignTool {
    $configured = [Environment]::GetEnvironmentVariable('RS_WIN_SIGNTOOL')
    if (-not [string]::IsNullOrWhiteSpace($configured)) {
        if (Test-Path -LiteralPath $configured -PathType Leaf) {
            return (Get-Item -LiteralPath $configured).FullName
        }
        $fromPath = Get-Command $configured -ErrorAction SilentlyContinue
        if ($null -ne $fromPath) {
            return $fromPath.Source
        }
        throw "RS_WIN_SIGNTOOL does not point to signtool.exe: $configured"
    }

    $pathTool = Get-Command 'signtool.exe' -ErrorAction SilentlyContinue
    if ($null -ne $pathTool) {
        return $pathTool.Source
    }

    $programFilesX86 = [Environment]::GetEnvironmentVariable('ProgramFiles(x86)')
    if ([string]::IsNullOrWhiteSpace($programFilesX86)) {
        throw 'signtool.exe was not found and the Windows SDK location is unavailable.'
    }
    $kitsRoot = Join-Path $programFilesX86 'Windows Kits\10\bin'
    $kitTools = @()
    if (Test-Path -LiteralPath $kitsRoot) {
        $kitTools = @(Get-ChildItem -LiteralPath $kitsRoot -Directory -ErrorAction SilentlyContinue |
            ForEach-Object {
                $kitVersion = $null
                $candidate = Join-Path $_.FullName 'x64\signtool.exe'
                if ([version]::TryParse($_.Name, [ref]$kitVersion) -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
                    [pscustomobject]@{ Version = $kitVersion; Path = $candidate }
                }
            } |
            Sort-Object Version -Descending)
    }
    if ($kitTools.Count -gt 0) {
        return $kitTools[0].Path
    }
    throw 'signtool.exe was not found in PATH or the Windows 10 SDK.'
}

try {
    if ($null -eq $Files -or $Files.Count -eq 0) {
        throw 'At least one file is required for signing.'
    }
    foreach ($file in $Files) {
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
            throw "Signing input does not exist: $file"
        }
    }

    $mode = [Environment]::GetEnvironmentVariable('RS_WIN_SIGN_MODE')
    if ([string]::IsNullOrWhiteSpace($mode) -or $mode -eq 'none') {
        Write-Warning 'Windows signing is not configured; leaving outputs unsigned.'
        exit 10
    }
    if ($mode -ne 'thumbprint' -and $mode -ne 'dlib') {
        throw "Unsupported RS_WIN_SIGN_MODE: $mode"
    }

    $signtool = Find-SignTool
    $timestamp = [Environment]::GetEnvironmentVariable('RS_WIN_TIMESTAMP_URL')
    if ([string]::IsNullOrWhiteSpace($timestamp)) {
        $timestamp = 'http://timestamp.digicert.com'
    }

    $certificateArguments = @()
    if ($mode -eq 'thumbprint') {
        $thumbprint = [Environment]::GetEnvironmentVariable('RS_WIN_SIGN_THUMBPRINT')
        if ([string]::IsNullOrWhiteSpace($thumbprint)) {
            throw 'RS_WIN_SIGN_THUMBPRINT is required for thumbprint signing.'
        }
        $thumbprint = $thumbprint.Replace(' ', '').Trim()
        if ($thumbprint -notmatch '^[0-9A-Fa-f]{40}$') {
            throw 'RS_WIN_SIGN_THUMBPRINT must contain a 40-character hexadecimal thumbprint.'
        }
        $certificateArguments = @('/sha1', $thumbprint)
        Write-Host ('Using certificate thumbprint ending in ' + $thumbprint.Substring(36, 4))
    } else {
        $dlib = [Environment]::GetEnvironmentVariable('RS_WIN_SIGN_DLIB')
        $metadata = [Environment]::GetEnvironmentVariable('RS_WIN_SIGN_METADATA')
        if ([string]::IsNullOrWhiteSpace($dlib) -or [string]::IsNullOrWhiteSpace($metadata)) {
            throw 'RS_WIN_SIGN_DLIB and RS_WIN_SIGN_METADATA are required for dlib signing.'
        }
        if (-not (Test-Path -LiteralPath $dlib -PathType Leaf)) {
            throw "Signing dlib does not exist: $dlib"
        }
        if (-not (Test-Path -LiteralPath $metadata -PathType Leaf)) {
            throw "Signing metadata does not exist: $metadata"
        }
        $certificateArguments = @('/dlib', $dlib, '/dmdf', $metadata)
    }

    $signArguments = @('sign', '/fd', 'sha256', '/tr', $timestamp, '/td', 'sha256', '/d', $Description)
    $signArguments += $certificateArguments
    $signArguments += $Files
    & $signtool @signArguments
    $signStatus = $LASTEXITCODE
    if ($signStatus -ne 0) {
        # Ten is reserved for absent configuration, not a signtool failure.
        exit 1
    }

    foreach ($file in $Files) {
        & $signtool 'verify' '/pa' '/v' $file
        $verifyStatus = $LASTEXITCODE
        if ($verifyStatus -ne 0) {
            exit 1
        }
    }
    exit 0
} catch {
    Write-Error $_ -ErrorAction Continue
    exit 1
}
