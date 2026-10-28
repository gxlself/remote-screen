[CmdletBinding()]
param(
    [string]$Version,
    [string]$Out = 'target\rs-release\win',
    [switch]$SkipBuild,
    [string]$StageDir,
    [string]$ServicePackage = 'rs-host-agent',
    [string]$ServiceBin = 'rs-host',
    [string]$HostUiExe,
    [string]$WebView2Bootstrapper,
    [switch]$DownloadWebView2,
    [switch]$SkipServiceStart,
    [switch]$Sign,
    [switch]$RequireSign
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Invoke-External {
    param(
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $false)][string[]]$Arguments = @()
    )
    & $Command @Arguments | Out-Host
    $status = $LASTEXITCODE
    if ($status -ne 0) {
        throw "Command failed with exit code ${status}: $Command"
    }
}

function Read-ExternalText {
    param(
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $false)][string[]]$Arguments = @()
    )
    $text = (& $Command @Arguments | Out-String).Trim()
    $status = $LASTEXITCODE
    if ($status -ne 0) {
        throw "Command failed with exit code ${status}: $Command"
    }
    return $text
}

function Resolve-FullPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    $fullPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Path)
    $rootPath = [System.IO.Path]::GetPathRoot($fullPath)
    if ($fullPath.Length -gt $rootPath.Length) {
        return $fullPath.TrimEnd('\', '/')
    }
    return $fullPath
}

function Read-ProductDefine {
    param(
        [Parameter(Mandatory = $true)][string]$ProductPath,
        [Parameter(Mandatory = $true)][string]$Name
    )
    $productText = [System.IO.File]::ReadAllText($ProductPath)
    $escapedName = [regex]::Escape($Name)
    $match = [regex]::Match($productText, '<\?define\s+' + $escapedName + '\s*=\s*"([^"]+)"\s*\?>')
    if (-not $match.Success) {
        throw "Product.wxi is missing define $Name"
    }
    return $match.Groups[1].Value
}

function Invoke-SignFiles {
    param(
        [Parameter(Mandatory = $true)][string[]]$Files,
        [Parameter(Mandatory = $true)][string]$Description,
        [Parameter(Mandatory = $true)][bool]$Required
    )
    $signScript = Join-Path $winRoot 'sign.ps1'
    # Call the script with PowerShell array binding. Its exit sets LASTEXITCODE
    # and returns to this caller (it is deliberately not dot-sourced).
    & $signScript -Files $Files -Description $Description | Out-Host
    $status = $LASTEXITCODE
    if ($status -eq 10) {
        if ($Required) {
            throw 'Signing is required but no Windows signing configuration was provided.'
        }
        Write-Warning 'Signing is not configured; continuing with an unsigned artifact.'
        return $false
    }
    if ($status -ne 0) {
        throw "Signing failed with exit code $status."
    }
    return $true
}

function Get-SignatureInfo {
    param([Parameter(Mandatory = $true)][string]$Path)
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne 'Valid') {
        throw "Authenticode signature is not valid for ${Path}: $($signature.Status)"
    }
    if ($null -eq $signature.SignerCertificate) {
        throw "Authenticode signer is missing for $Path"
    }
    return $signature.SignerCertificate.Subject
}

function Write-AtomicJson {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)]$Value
    )
    $temporary = "$Path.tmp"
    $json = ($Value | ConvertTo-Json -Depth 8) -replace "`r`n", "`n"
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($temporary, $json + "`n", $utf8)
    if (Test-Path -LiteralPath $Path -PathType Leaf) {
        [System.IO.File]::Replace($temporary, $Path, $null)
    } else {
        [System.IO.File]::Move($temporary, $Path)
    }
}

$locationPushed = $false
try {
    $scriptRoot = $PSScriptRoot
    $repoRoot = Resolve-FullPath (Join-Path $scriptRoot '..\..')
    $winRoot = Join-Path $scriptRoot 'win'
    $productPath = Join-Path $winRoot 'Product.wxi'
    if (-not (Test-Path -LiteralPath $productPath -PathType Leaf)) {
        throw 'Dependency missing: Product.wxi'
    }
    $outPath = Resolve-FullPath $Out
    if ([string]::IsNullOrWhiteSpace($StageDir)) {
        $stagePath = Join-Path $outPath 'stage'
    } else {
        $stagePath = Resolve-FullPath $StageDir
    }
    if ([string]::IsNullOrWhiteSpace($WebView2Bootstrapper)) {
        $webView2Path = Join-Path $outPath 'deps\MicrosoftEdgeWebview2Setup.exe'
    } else {
        $webView2Path = Resolve-FullPath $WebView2Bootstrapper
    }
    $providedTray = $null
    if (-not [string]::IsNullOrWhiteSpace($HostUiExe)) {
        $providedTray = Resolve-FullPath $HostUiExe
    }
    Push-Location -LiteralPath $repoRoot
    $locationPushed = $true

    Write-Host '==> Tool checks'
    if ($null -eq (Get-Command 'wix' -ErrorAction SilentlyContinue)) {
        throw 'wix was not found in PATH.'
    }
    $wixVersion = Read-ExternalText 'wix' @('--version')
    if ($wixVersion -notmatch '^5\.') {
        throw "WiX v5 is required; found: $wixVersion"
    }
    $extensions = Read-ExternalText 'wix' @('extension', 'list', '-g')
    $missingExtensions = @()
    foreach ($extensionName in @('Firewall', 'Util', 'Bal')) {
        $extensionId = 'WixToolset.' + $extensionName + '.wixext'
        if ($extensions -notmatch ([regex]::Escape($extensionId) + '[\s/]+5\.0\.\d+(?:\s|$)')) {
            Write-Host "Missing WiX extension. Install it with: wix extension add -g WixToolset.$extensionName.wixext/5.0.2"
            $missingExtensions += $extensionName
        }
    }
    if ($missingExtensions.Count -gt 0) {
        throw ('Required WiX 5.0.x extensions are missing: ' + ($missingExtensions -join ', '))
    }
    if (-not $SkipBuild) {
        foreach ($toolName in @('cargo', 'pnpm', 'rustup')) {
            if ($null -eq (Get-Command $toolName -ErrorAction SilentlyContinue)) {
                if ($toolName -eq 'pnpm') {
                    Write-Host 'pnpm was not found; run corepack enable, then retry.'
                }
                throw "$toolName was not found in PATH."
            }
        }
        $installedTargets = Read-ExternalText 'rustup' @('target', 'list', '--installed')
        if (($installedTargets -split '\r?\n') -notcontains 'x86_64-pc-windows-msvc') {
            throw 'Rust target x86_64-pc-windows-msvc is not installed.'
        }
    }

    Write-Host '==> Version'
    $cargoToml = Join-Path $repoRoot 'Cargo.toml'
    if ([string]::IsNullOrWhiteSpace($Version)) {
        $cargoText = [System.IO.File]::ReadAllText($cargoToml)
        $workspaceMatch = [regex]::Match($cargoText, '(?ms)^\[workspace\.package\](.*?)(?:^\[|\z)')
        if (-not $workspaceMatch.Success) {
            throw 'Could not find [workspace.package] in Cargo.toml.'
        }
        $versionMatch = [regex]::Match($workspaceMatch.Groups[1].Value, '(?m)^\s*version\s*=\s*"([^"]+)"')
        if (-not $versionMatch.Success) {
            throw 'Could not read workspace package version from Cargo.toml.'
        }
        $Version = $versionMatch.Groups[1].Value
    }
    if ($Version -notmatch '^\d+\.\d+\.\d+$') {
        throw "Version must contain exactly three numeric components: $Version"
    }

    New-Item -ItemType Directory -Path $outPath -Force | Out-Null

    $serviceExeName = Read-ProductDefine $productPath 'ServiceExe'
    $sessionExeName = Read-ProductDefine $productPath 'SessionExe'
    $trayExeName = Read-ProductDefine $productPath 'TrayExe'
    foreach ($exeName in @($serviceExeName, $sessionExeName, $trayExeName)) {
        if ([System.IO.Path]::GetFileName($exeName) -ne $exeName -or [System.IO.Path]::GetExtension($exeName) -ne '.exe') {
            throw "Product.wxi executable define must be an .exe file name: $exeName"
        }
    }

    $serviceSource = $null
    $sessionSource = $null
    $traySource = $null
    $rustFlagsChanged = $false
    $oldRustFlags = $null
    if (-not $SkipBuild) {
        Write-Host '==> Build'
        $oldRustFlags = [Environment]::GetEnvironmentVariable('CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS', 'Process')
        [Environment]::SetEnvironmentVariable('CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS', '-C target-feature=+crt-static', 'Process')
        $rustFlagsChanged = $true
        try {
            $serviceArgs = @('build', '--locked', '--release', '--target', 'x86_64-pc-windows-msvc', '-p', $ServicePackage, '--bin', $ServiceBin)
            Invoke-External 'cargo' $serviceArgs
            $sessionArgs = @('build', '--locked', '--release', '--target', 'x86_64-pc-windows-msvc', '-p', 'host-win', '--bin', 'rs-host-session')
            & cargo @sessionArgs
            $sessionStatus = $LASTEXITCODE
            if ($sessionStatus -ne 0) {
                throw "rs-host-session could not be built (exit $sessionStatus); use -SkipBuild with a prepared StageDir."
            }
            $metadataText = Read-ExternalText 'cargo' @('metadata', '--locked', '--no-deps', '--format-version', '1')
            $metadata = $metadataText | ConvertFrom-Json
            $targetDirectory = [string]$metadata.target_directory
            $serviceSource = Join-Path $targetDirectory ('x86_64-pc-windows-msvc\release\' + $ServiceBin + '.exe')
            $sessionSource = Join-Path $targetDirectory 'x86_64-pc-windows-msvc\release\rs-host-session.exe'

            if ([string]::IsNullOrWhiteSpace($HostUiExe)) {
                Invoke-External 'pnpm' @('--dir', 'host-ui', 'install', '--frozen-lockfile')
                # A JSON file avoids native argument quote stripping in PS 5.1.
                $tauriConfigPath = Join-Path $outPath '.tauri-win-config.json'
                [System.IO.File]::WriteAllText($tauriConfigPath, '{"mainBinaryName":"RemoteHost","bundle":{"externalBin":[]}}', (New-Object System.Text.UTF8Encoding($false)))
                Push-Location (Join-Path $repoRoot 'host-ui')
                try {
                    & pnpm 'tauri' 'build' '--no-bundle' '--target' 'x86_64-pc-windows-msvc' '--config' $tauriConfigPath
                    $tauriStatus = $LASTEXITCODE
                    if ($tauriStatus -ne 0) {
                        throw "Tauri build failed with exit code $tauriStatus."
                    }
                    $uiMetadataText = Read-ExternalText 'cargo' @('metadata', '--locked', '--manifest-path', 'src-tauri\Cargo.toml', '--no-deps', '--format-version', '1')
                    $uiMetadata = $uiMetadataText | ConvertFrom-Json
                    $uiTargetDirectory = [string]$uiMetadata.target_directory
                    $traySource = Join-Path $uiTargetDirectory 'x86_64-pc-windows-msvc\release\RemoteHost.exe'
                } finally {
                    Pop-Location
                    Remove-Item -LiteralPath $tauriConfigPath -Force
                }
            } else {
                $traySource = $providedTray
            }
        } finally {
            if ($rustFlagsChanged) {
                [Environment]::SetEnvironmentVariable('CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS', $oldRustFlags, 'Process')
                $rustFlagsChanged = $false
            }
        }
    } else {
        Write-Host '==> Build (skipped)'
        $serviceSource = Join-Path $stagePath $serviceExeName
        $sessionSource = Join-Path $stagePath $sessionExeName
        $traySource = Join-Path $stagePath $trayExeName
    }

    Write-Host '==> Stage'
    if ($SkipBuild) {
        if (-not (Test-Path -LiteralPath $stagePath -PathType Container)) {
            throw "StageDir does not exist for -SkipBuild: $stagePath"
        }
    } else {
        foreach ($source in @($serviceSource, $sessionSource, $traySource)) {
            if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
                throw "Build output is missing: $source"
            }
        }
        $stagePrefix = $stagePath.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
        foreach ($protectedPath in @($repoRoot, $outPath, $serviceSource, $sessionSource, $traySource)) {
            if ($stagePath.Equals($protectedPath, [System.StringComparison]::OrdinalIgnoreCase) -or $protectedPath.StartsWith($stagePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw 'StageDir must not contain the repository, output directory, or build inputs.'
            }
        }
        if (Test-Path -LiteralPath $stagePath) {
            if (((Get-Item -LiteralPath $stagePath).Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw 'StageDir must not be a symbolic link or directory junction.'
            }
            Remove-Item -LiteralPath $stagePath -Recurse -Force
        }
        New-Item -ItemType Directory -Path $stagePath -Force | Out-Null
        Copy-Item -LiteralPath $serviceSource -Destination (Join-Path $stagePath $serviceExeName) -Force
        Copy-Item -LiteralPath $sessionSource -Destination (Join-Path $stagePath $sessionExeName) -Force
        Copy-Item -LiteralPath $traySource -Destination (Join-Path $stagePath $trayExeName) -Force
    }
    foreach ($requiredFile in @($serviceExeName, $sessionExeName, $trayExeName)) {
        if (-not (Test-Path -LiteralPath (Join-Path $stagePath $requiredFile) -PathType Leaf)) {
            throw "StageDir is missing required executable: $requiredFile"
        }
    }
    $dependencyDir = Join-Path $outPath 'deps'
    if (Test-Path -LiteralPath $dependencyDir -PathType Container) {
        foreach ($dependency in @(Get-ChildItem -LiteralPath $dependencyDir -File -Filter '*.dll')) {
            $dependencyTarget = Join-Path $stagePath $dependency.Name
            if ($dependency.FullName -ne $dependencyTarget) {
                Copy-Item -LiteralPath $dependency.FullName -Destination $dependencyTarget -Force
            }
        }
    }

    $signingEnabled = $Sign -or $RequireSign
    $signedArtifacts = @()
    $signerSubjects = @{}
    $engineSubject = $null
    if ($signingEnabled) {
        Write-Host '==> Sign executables and libraries'
        $initialSigningFiles = @(Get-ChildItem -LiteralPath $stagePath -File |
            Where-Object { $_.Extension -in @('.exe', '.dll') } |
            Select-Object -ExpandProperty FullName)
        if ($initialSigningFiles.Count -eq 0) {
            throw 'No executable or DLL files were found to sign in StageDir.'
        }
        $signingEnabled = Invoke-SignFiles -Files $initialSigningFiles -Description 'Remote Host executable' -Required ([bool]$RequireSign)
        if ($signingEnabled) {
            $signedArtifacts += $initialSigningFiles
        }
    }

    $nameSuffix = $Version
    if ($SkipServiceStart) {
        $nameSuffix = $Version + '-test'
    }
    $msiPath = Join-Path $outPath ('RemoteHost-' + $nameSuffix + '.msi')
    $setupPath = Join-Path $outPath ('RemoteHostSetup-' + $nameSuffix + '.exe')

    Write-Host '==> Build MSI'
    $msiSource = Join-Path $winRoot 'RemoteHost.wxs'
    if (-not (Test-Path -LiteralPath $msiSource -PathType Leaf)) {
        throw 'Dependency missing: RemoteHost.wxs.'
    }
    $msiArguments = @('build', '-arch', 'x64', '-ext', 'WixToolset.Firewall.wixext', '-ext', 'WixToolset.Util.wixext', '-d', ('Version=' + $Version), '-d', ('StageDir=' + $stagePath))
    if ($SkipServiceStart) {
        $msiArguments += @('-d', 'SkipServiceStart=1')
    }
    $msiArguments += @('-o', $msiPath, $msiSource)
    Invoke-External 'wix' $msiArguments
    if ($signingEnabled) {
        Write-Host '==> Sign MSI'
        Invoke-SignFiles -Files @($msiPath) -Description 'Remote Host MSI' -Required $true | Out-Null
        $signedArtifacts += $msiPath
    }

    Write-Host '==> Prepare WebView2 bootstrapper'
    if (-not (Test-Path -LiteralPath $webView2Path -PathType Leaf)) {
        if (-not $DownloadWebView2) {
            throw "WebView2 bootstrapper is missing: $webView2Path. Provide it or use -DownloadWebView2."
        }
        $webView2Parent = Split-Path -Parent $webView2Path
        New-Item -ItemType Directory -Path $webView2Parent -Force | Out-Null
        $oldSecurityProtocol = [Net.ServicePointManager]::SecurityProtocol
        try {
            [Net.ServicePointManager]::SecurityProtocol = $oldSecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
            Invoke-WebRequest -UseBasicParsing -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile $webView2Path
            $downloadSignature = Get-AuthenticodeSignature -LiteralPath $webView2Path
            if ($downloadSignature.Status -ne 'Valid' -or $null -eq $downloadSignature.SignerCertificate -or $downloadSignature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') {
                throw 'Downloaded WebView2 bootstrapper did not have a valid Microsoft Corporation signature.'
            }
        } catch {
            if (Test-Path -LiteralPath $webView2Path -PathType Leaf) {
                Remove-Item -LiteralPath $webView2Path -Force
            }
            throw
        } finally {
            [Net.ServicePointManager]::SecurityProtocol = $oldSecurityProtocol
        }
    }

    Write-Host '==> Build Burn bundle'
    $bundleArguments = @('build', '-arch', 'x64', '-ext', 'WixToolset.Bal.wixext', '-ext', 'WixToolset.Util.wixext', '-d', ('Version=' + $Version), '-d', ('MsiPath=' + $msiPath), '-d', ('WebView2Bootstrapper=' + $webView2Path), '-o', $setupPath, (Join-Path $winRoot 'Bundle.wxs'))
    Invoke-External 'wix' $bundleArguments
    if ($signingEnabled) {
        Write-Host '==> Sign Burn engine and bundle'
        $enginePath = Join-Path $outPath 'engine.exe'
        try {
            & wix burn detach $setupPath '-engine' $enginePath
            $detachStatus = $LASTEXITCODE
            if ($detachStatus -ne 0) {
                throw "Burn engine detach failed with exit code $detachStatus."
            }
            Invoke-SignFiles -Files @($enginePath) -Description 'Remote Host Burn engine' -Required $true | Out-Null
            $engineSubject = Get-SignatureInfo $enginePath
            & wix burn reattach $setupPath '-engine' $enginePath '-o' $setupPath
            $reattachStatus = $LASTEXITCODE
            if ($reattachStatus -ne 0) {
                throw "Burn engine reattach failed with exit code $reattachStatus."
            }
            Invoke-SignFiles -Files @($setupPath) -Description 'Remote Host Burn bundle' -Required $true | Out-Null
            $signedArtifacts += $setupPath
        } finally {
            if (Test-Path -LiteralPath $enginePath -PathType Leaf) {
                Remove-Item -LiteralPath $enginePath -Force
            }
        }
    }

    Write-Host '==> Verify signatures'
    if ($signingEnabled) {
        foreach ($artifact in $signedArtifacts) {
            $subject = Get-SignatureInfo $artifact
            $signerSubjects[$artifact] = $subject
        }
        $distinctSubjects = @((@($signerSubjects.Values) + @($engineSubject)) | Sort-Object -Unique -CaseSensitive)
        if ($distinctSubjects.Count -ne 1) {
            throw 'Signed artifacts have different Authenticode signer subjects.'
        }
    }

    Write-Host '==> Generate manifests'
    $releaseFiles = @($msiPath, $setupPath)
    $sumLines = @()
    $artifactRecords = @()
    foreach ($releaseFile in $releaseFiles) {
        if (-not (Test-Path -LiteralPath $releaseFile -PathType Leaf)) {
            throw "Expected release artifact was not produced: $releaseFile"
        }
        $hash = (Get-FileHash -LiteralPath $releaseFile -Algorithm SHA256).Hash.ToLowerInvariant()
        $size = (Get-Item -LiteralPath $releaseFile).Length
        $fileName = Split-Path -Leaf $releaseFile
        $sumLines += ($hash + '  ' + $fileName)
        $artifactSubject = $null
        $artifactSigned = $false
        if ($signerSubjects.ContainsKey($releaseFile)) {
            $artifactSigned = $true
            $artifactSubject = $signerSubjects[$releaseFile]
        }
        $artifactRecords += [ordered]@{
            file = $fileName
            sha256 = $hash
            size = [int64]$size
            signed = $artifactSigned
            signer_subject = $artifactSubject
        }
    }
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText((Join-Path $outPath 'SHA256SUMS'), (($sumLines -join "`n") + "`n"), $utf8)

    if (-not $SkipServiceStart) {
        $msiRecord = $artifactRecords[0]
        $releasePayload = [ordered]@{
            version = $Version
            os = 'win'
            arch = 'x64'
            file = $msiRecord.file
            sha256 = $msiRecord.sha256
            size = $msiRecord.size
            signed = $msiRecord.signed
            signer_subject = $msiRecord.signer_subject
        }
        $releasePath = Join-Path $outPath 'release.json'
        Write-AtomicJson -Path $releasePath -Value $releasePayload
    } else {
        $staleReleasePath = Join-Path $outPath 'release.json'
        if (Test-Path -LiteralPath $staleReleasePath -PathType Leaf) {
            Remove-Item -LiteralPath $staleReleasePath -Force
        }
    }

    $gitCommit = $null
    if ($null -ne (Get-Command 'git' -ErrorAction SilentlyContinue)) {
        try {
            $gitOutput = (& git '-C' $repoRoot 'rev-parse' 'HEAD' 2>$null | Out-String).Trim()
            $gitStatus = $LASTEXITCODE
            if ($gitStatus -eq 0 -and $gitOutput -match '^[0-9a-fA-F]{7,}$') {
                $gitCommit = $gitOutput
            }
        } catch {
            $gitCommit = $null
        }
    }
    $summaryPayload = [ordered]@{
        version = $Version
        os = 'win'
        arch = 'x64'
        artifacts = $artifactRecords
        git_commit = $gitCommit
        wix_version = $wixVersion
        test_build = [bool]$SkipServiceStart
    }
    Write-AtomicJson (Join-Path $outPath 'release-summary.json') $summaryPayload

    if ($SkipServiceStart) {
        Write-Host 'WARNING: test build: service is not started by the MSI' -ForegroundColor Yellow
    }
    Write-Host 'Next step on the release machine: sign the manifest with the Ed25519 publishing key (see 09).'
    exit 0
} catch {
    Write-Error $_ -ErrorAction Continue
    exit 1
} finally {
    if ($locationPushed) {
        Pop-Location
    }
}
