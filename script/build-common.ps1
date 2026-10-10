<#
.SYNOPSIS
    Shared helpers for script/build-easytier-*.ps1

.NOTES
    Caller must set $RepoRoot (and ideally $CargoBin) before dot-sourcing.
    $PSScriptRoot here resolves to script\ (this file's directory).
#>

if (-not $RepoRoot) {
    throw 'build-common.ps1: $RepoRoot must be set by the caller before dot-sourcing.'
}

if (-not $CargoBin) {
    $CargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
}
if (Test-Path $CargoBin) {
    $env:Path = "$CargoBin;" + $env:Path
}

# wasm-pack pins Binaryen version_117 (see wasm-pack install::prebuilt_url_for)
$script:WasmOptBinaryenVersion = 'version_117'
$script:WasmOptWindowsArchiveUrl = "https://github.com/WebAssembly/binaryen/releases/download/$script:WasmOptBinaryenVersion/binaryen-$script:WasmOptBinaryenVersion-x86_64-windows.tar.gz"

# Wall-clock + elapsed since this common module was loaded (per packaging run).
if (-not $script:EasytierBuildStartedAt) {
    $script:EasytierBuildStartedAt = Get-Date
}

function Get-EasytierLogPrefix {
    $now = Get-Date
    $elapsed = $now - $script:EasytierBuildStartedAt
    $elapsedStr = '{0:00}:{1:00}:{2:00}' -f `
        [int][math]::Floor($elapsed.TotalHours), `
        $elapsed.Minutes, `
        $elapsed.Seconds
    return ('[{0} +{1}]' -f $now.ToString('HH:mm:ss'), $elapsedStr)
}

function Write-Step([string]$Message) {
    Write-Host ""
    Write-Host ("{0} ==> {1}" -f (Get-EasytierLogPrefix), $Message) -ForegroundColor Cyan
}

function Write-Log {
    param(
        [Parameter(Mandatory)][string]$Message,
        [ValidateSet('Info', 'Ok', 'Warn', 'Err')]
        [string]$Level = 'Info'
    )
    $color = switch ($Level) {
        'Ok' { 'Green' }
        'Warn' { 'Yellow' }
        'Err' { 'Red' }
        default { 'Gray' }
    }
    Write-Host ("{0} {1}" -f (Get-EasytierLogPrefix), $Message) -ForegroundColor $color
}

function Get-EasytierElapsedString {
    $elapsed = (Get-Date) - $script:EasytierBuildStartedAt
    return ('{0:00}:{1:00}:{2:00}' -f `
        [int][math]::Floor($elapsed.TotalHours), `
        $elapsed.Minutes, `
        $elapsed.Seconds)
}

function Write-EasytierBuildElapsed {
    param([string]$Label = 'Total elapsed')
    Write-Host ''
    Write-Log ("{0}: {1}" -f $Label, (Get-EasytierElapsedString)) -Level Ok
}

function Assert-Command([string]$Name) {
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Required command not found: $Name"
    }
}

function Set-EasytierWritableTemp {
    <#
      Some environments set TEMP to a restricted folder (e.g. ...\Temp\2).
      esbuild then fails with "Access is denied" when deleting its work dir.
    #>
    $etTemp = Join-Path $RepoRoot '.workbuddy-ai\tmp\esbuild-temp'
    New-Item -ItemType Directory -Force -Path $etTemp | Out-Null
    $env:TEMP = $etTemp
    $env:TMP = $etTemp
}

function Ensure-WindowsBuildPrereqs {
    <#
      thunk-rs needs `7z` on PATH. Cached VC-LTL / YY-Thunks / protoc avoid re-download.
    #>
    $sevenZipDirs = @(
        (Join-Path ${env:ProgramFiles} '7-Zip'),
        (Join-Path ${env:ProgramFiles(x86)} '7-Zip')
    )
    $sevenZip = Get-Command 7z -ErrorAction SilentlyContinue
    if (-not $sevenZip) {
        foreach ($dir in $sevenZipDirs) {
            $exe = Join-Path $dir '7z.exe'
            if (Test-Path $exe) {
                $env:Path = "$dir;" + $env:Path
                $sevenZip = Get-Command 7z -ErrorAction SilentlyContinue
                break
            }
        }
    }
    if (-not $sevenZip) {
        throw @"
7z not found. thunk-rs needs 7-Zip to unpack VC-LTL / YY-Thunks.
Install 7-Zip and ensure 7z.exe is on PATH, e.g.:
  C:\Program Files\7-Zip\7z.exe
Download: https://www.7-zip.org/
"@
    }
    Write-Log "7z OK: $($sevenZip.Source)" -Level Ok

    $thunkCache = Join-Path $env:USERPROFILE '.cache\thunk-deps'
    $repoThunkCache = Join-Path $RepoRoot '.deps-cache\thunk'
    $vcCandidates = @(
        $env:VC_LTL,
        (Join-Path $thunkCache 'VC-LTL-5.2.2'),
        (Join-Path $repoThunkCache 'VC-LTL-5.2.2')
    ) | Where-Object { $_ }
    foreach ($p in $vcCandidates) {
        if (Test-Path $p) {
            $env:VC_LTL = $p
            Write-Log "VC_LTL=$p" -Level Ok
            break
        }
    }

    $yyCandidates = @(
        $env:YY_THUNKS,
        (Join-Path $thunkCache 'YY-Thunks-1.1.7'),
        (Join-Path $repoThunkCache 'YY-Thunks-1.1.7')
    ) | Where-Object { $_ }
    foreach ($p in $yyCandidates) {
        if (Test-Path $p) {
            $env:YY_THUNKS = $p
            Write-Log "YY_THUNKS=$p" -Level Ok
            break
        }
    }

    $protocCandidates = @(
        $env:PROTOC,
        (Join-Path $env:USERPROFILE '.cache\protoc\bin\protoc.exe'),
        (Join-Path $RepoRoot '.deps-cache\protoc\bin\protoc.exe')
    ) | Where-Object { $_ }
    foreach ($p in $protocCandidates) {
        if (Test-Path $p) {
            $env:PROTOC = $p
            Write-Log "PROTOC=$p" -Level Ok
            break
        }
    }
}

function Get-RequiredWasmBindgenVersion {
    $lock = Join-Path $RepoRoot 'Cargo.lock'
    if (-not (Test-Path $lock)) { return $null }
    $lines = Get-Content $lock
    for ($i = 0; $i -lt $lines.Count - 1; $i++) {
        if ($lines[$i] -eq 'name = "wasm-bindgen"') {
            if ($lines[$i + 1] -match '^version = "([^"]+)"') {
                return $Matches[1]
            }
        }
    }
    return $null
}

function Ensure-WasmBindgenLocal {
    <#
      Make wasm-bindgen CLI available locally so wasm-pack does not re-download.
      Order: PATH match → vendor dir → wasm-pack cache → copy into ~/.cargo/bin
    #>
    $required = Get-RequiredWasmBindgenVersion
    if (-not $required) {
        Write-Warning "Could not read wasm-bindgen version from Cargo.lock; skipping local ensure."
        return
    }

    $vendorDir = Join-Path $PSScriptRoot "tools\wasm-bindgen-$required"
    $cacheRoot = Join-Path $env:LOCALAPPDATA '.wasm-pack'

    function Test-WasmBindgenAt([string]$ExePath, [string]$ExpectVer) {
        if (-not (Test-Path $ExePath)) { return $false }
        try {
            $out = & $ExePath --version 2>&1 | Out-String
            return ($out -match [regex]::Escape($ExpectVer))
        } catch {
            return $false
        }
    }

    # Clear stale install locks that can hang wasm-pack on tool install
    if (Test-Path $cacheRoot) {
        Get-ChildItem $cacheRoot -Filter '.wasm-bindgen-*.lock' -Force -ErrorAction SilentlyContinue |
            Remove-Item -Force -ErrorAction SilentlyContinue
        Get-ChildItem $cacheRoot -Filter '.wasm-opt-*.lock' -Force -ErrorAction SilentlyContinue |
            Remove-Item -Force -ErrorAction SilentlyContinue
    }

    # 1) Already on PATH with correct version
    $existing = Get-Command wasm-bindgen -ErrorAction SilentlyContinue
    if ($existing -and (Test-WasmBindgenAt $existing.Source $required)) {
        Write-Log "wasm-bindgen $required OK (PATH: $($existing.Source))" -Level Ok
        if (-not (Test-Path $vendorDir)) {
            New-Item -ItemType Directory -Path $vendorDir | Out-Null
        }
        $vendorExePath = Join-Path $vendorDir 'wasm-bindgen.exe'
        if (-not (Test-Path $vendorExePath)) {
            Copy-Item -Force $existing.Source $vendorExePath
        }
        return
    }

    # 2) Vendor copy next to scripts (manual drop-in)
    $vendorExe = Join-Path $vendorDir 'wasm-bindgen.exe'
    $sourceExe = $null
    if (Test-WasmBindgenAt $vendorExe $required) {
        $sourceExe = $vendorExe
        Write-Log "Using vendor wasm-bindgen: $vendorExe"
    }

    # 3) wasm-pack cache under %LOCALAPPDATA%\.wasm-pack\
    if (-not $sourceExe -and (Test-Path $cacheRoot)) {
        foreach ($dir in @(Get-ChildItem $cacheRoot -Directory -Filter 'wasm-bindgen-*' -ErrorAction SilentlyContinue)) {
            $candidate = Join-Path $dir.FullName 'wasm-bindgen.exe'
            if (Test-WasmBindgenAt $candidate $required) {
                $sourceExe = $candidate
                Write-Log "Using cached wasm-bindgen: $candidate"
                break
            }
        }
    }

    if (-not $sourceExe) {
        Write-Warning @"
wasm-bindgen $required not found locally. wasm-pack may download it.
Place the Windows release here to avoid network:
  $vendorDir\wasm-bindgen.exe
Download:
  https://github.com/wasm-bindgen/wasm-bindgen/releases/download/$required/wasm-bindgen-$required-x86_64-pc-windows-msvc.tar.gz
"@
        return
    }

    if (-not (Test-Path $CargoBin)) {
        New-Item -ItemType Directory -Path $CargoBin | Out-Null
    }
    $dest = Join-Path $CargoBin 'wasm-bindgen.exe'
    Copy-Item -Force $sourceExe $dest
    $runnerSrc = Join-Path (Split-Path $sourceExe -Parent) 'wasm-bindgen-test-runner.exe'
    if (Test-Path $runnerSrc) {
        Copy-Item -Force $runnerSrc (Join-Path $CargoBin 'wasm-bindgen-test-runner.exe')
    }

    if (-not (Test-Path $vendorDir)) {
        New-Item -ItemType Directory -Path $vendorDir | Out-Null
    }
    Copy-Item -Force $sourceExe (Join-Path $vendorDir 'wasm-bindgen.exe')
    if (Test-Path $runnerSrc) {
        Copy-Item -Force $runnerSrc (Join-Path $vendorDir 'wasm-bindgen-test-runner.exe')
    }

    if (-not (Test-WasmBindgenAt $dest $required)) {
        throw "Failed to install local wasm-bindgen $required into $CargoBin"
    }
    Write-Log "wasm-bindgen $required installed to $dest (reused next time)" -Level Ok
}

function Test-WasmOptExe([string]$ExePath) {
    if (-not (Test-Path $ExePath)) { return $false }
    try {
        $out = & $ExePath --version 2>&1 | Out-String
        return ($out -match 'wasm-opt\s+version')
    } catch {
        return $false
    }
}

function Expand-BinaryenArchive([string]$ArchivePath, [string]$VendorRoot) {
    if (-not (Test-Path $ArchivePath)) {
        throw "Binaryen archive not found: $ArchivePath"
    }
    $extractTmp = Join-Path $env:TEMP ("binaryen-extract-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $extractTmp | Out-Null
    try {
        tar -xf $ArchivePath -C $extractTmp
        if ($LASTEXITCODE -ne 0) { throw "tar extract failed for $ArchivePath" }

        $extracted = Get-ChildItem $extractTmp -Directory | Select-Object -First 1
        if (-not $extracted) { throw "Unexpected Binaryen archive layout (no top-level dir)" }

        if (Test-Path $VendorRoot) {
            Remove-Item -Recurse -Force $VendorRoot
        }
        New-Item -ItemType Directory -Path (Split-Path $VendorRoot -Parent) -Force | Out-Null
        Move-Item -Force $extracted.FullName $VendorRoot

        $exe = Join-Path $VendorRoot 'bin\wasm-opt.exe'
        if (-not (Test-WasmOptExe $exe)) {
            throw "wasm-opt.exe missing or invalid after extract: $exe"
        }
        return $exe
    }
    finally {
        if (Test-Path $extractTmp) {
            Remove-Item -Recurse -Force $extractTmp -ErrorAction SilentlyContinue
        }
    }
}

function Ensure-WasmOptLocal {
    <#
      Prefer a local wasm-opt so wasm-pack does not download Binaryen.
      wasm-pack checks PATH first (any working wasm-opt), then its cache.
    #>
    $cacheRoot = Join-Path $env:LOCALAPPDATA '.wasm-pack'
    $vendorRoot = Join-Path $PSScriptRoot "tools\binaryen-$script:WasmOptBinaryenVersion"
    $vendorBin = Join-Path $vendorRoot 'bin'
    $vendorExe = Join-Path $vendorBin 'wasm-opt.exe'
    $archiveName = "binaryen-$script:WasmOptBinaryenVersion-x86_64-windows.tar.gz"
    $vendorArchive = Join-Path $PSScriptRoot "tools\$archiveName"

    if (Test-Path $cacheRoot) {
        Get-ChildItem $cacheRoot -Filter '.wasm-opt-*.lock' -Force -ErrorAction SilentlyContinue |
            Remove-Item -Force -ErrorAction SilentlyContinue
    }

    # 1) PATH
    $existing = Get-Command wasm-opt -ErrorAction SilentlyContinue
    if ($existing -and (Test-WasmOptExe $existing.Source)) {
        Write-Log "wasm-opt OK (PATH: $($existing.Source))" -Level Ok
        return
    }

    # 2) Vendor extract from local tar.gz if present
    if (-not (Test-WasmOptExe $vendorExe) -and (Test-Path $vendorArchive)) {
        Write-Log "Extracting local Binaryen archive: $vendorArchive"
        Expand-BinaryenArchive -ArchivePath $vendorArchive -VendorRoot $vendorRoot | Out-Null
    }

    # 3) Already extracted vendor
    if (Test-WasmOptExe $vendorExe) {
        $env:Path = "$vendorBin;" + $env:Path
        Write-Log "wasm-opt OK (vendor: $vendorExe)" -Level Ok
        return
    }

    # 4) wasm-pack cache
    if (Test-Path $cacheRoot) {
        foreach ($dir in @(Get-ChildItem $cacheRoot -Directory -Filter 'wasm-opt-*' -ErrorAction SilentlyContinue)) {
            $candidate = Join-Path $dir.FullName 'bin\wasm-opt.exe'
            if (-not (Test-Path $candidate)) {
                $candidate = Join-Path $dir.FullName 'wasm-opt.exe'
            }
            if (Test-WasmOptExe $candidate) {
                $binDir = Split-Path $candidate -Parent
                $env:Path = "$binDir;" + $env:Path
                Write-Log "wasm-opt OK (cache: $candidate)" -Level Ok
                if (-not (Test-Path $vendorBin)) {
                    New-Item -ItemType Directory -Path $vendorBin -Force | Out-Null
                }
                Copy-Item -Force (Join-Path $binDir '*') $vendorBin
                return
            }
        }
    }

    # 5) Try download into script/tools
    Write-Log "Attempting to download Binaryen $script:WasmOptBinaryenVersion ..."
    try {
        Invoke-WebRequest -Uri $script:WasmOptWindowsArchiveUrl -OutFile $vendorArchive -UseBasicParsing
        Expand-BinaryenArchive -ArchivePath $vendorArchive -VendorRoot $vendorRoot | Out-Null
        $env:Path = "$vendorBin;" + $env:Path
        if (Test-WasmOptExe $vendorExe) {
            Write-Log "wasm-opt OK (downloaded to $vendorExe)" -Level Ok
            return
        }
    } catch {
        Write-Warning "Auto-download failed: $($_.Exception.Message)"
        if (Test-Path $vendorArchive) {
            Remove-Item -Force $vendorArchive -ErrorAction SilentlyContinue
        }
    }

    throw @"
wasm-opt (Binaryen $script:WasmOptBinaryenVersion) not found locally, and download failed.

Manual setup (then re-run this script):
  1) Download:
     $($script:WasmOptWindowsArchiveUrl)
  2) Save as:
     $vendorArchive
     OR extract so this exists:
     $vendorExe
  3) Re-run script\easytier-web-fast.cmd / easytier-windows-fast.cmd (or *-release.cmd)

wasm-pack will reuse PATH/vendor and will not re-download.
"@
}

function Get-EasytierWindowsArchDir {
    param([string]$Target = '')
    if ($Target) {
        switch -Regex ($Target) {
            '^x86_64' { return 'x86_64' }
            '^i686' { return 'i686' }
            '^aarch64' { return 'arm64' }
        }
    }
    switch -Regex ($env:PROCESSOR_ARCHITECTURE) {
        'ARM64' { return 'arm64' }
        'x86' { return 'i686' }
        default { return 'x86_64' }
    }
}

function Copy-EasytierThirdPartyRuntime {
    param(
        [Parameter(Mandatory)][string]$ArchDir,
        [Parameter(Mandatory)][string]$DestDir
    )
    $srcDir = Join-Path $RepoRoot "easytier\third_party\$ArchDir"
    if (-not (Test-Path $srcDir)) {
        throw "third_party runtime dir missing: $srcDir"
    }
    New-Item -ItemType Directory -Force -Path $DestDir | Out-Null
    $copied = @()
    Get-ChildItem $srcDir -File | Where-Object {
        $_.Extension -in @('.dll', '.sys')
    } | ForEach-Object {
        Copy-Item -Force $_.FullName (Join-Path $DestDir $_.Name)
        $copied += $_.Name
    }
    if ($copied.Count -eq 0) {
        throw "No .dll/.sys found under $srcDir"
    }
    Write-Log ("Copied runtime: " + ($copied -join ', ') + " -> $DestDir") -Level Ok
}

function Get-CargoArtifactDir {
    param(
        [ValidateSet('Fast', 'Release')]
        [string]$Profile,
        [string]$Target = ''
    )
    $profileDir = if ($Profile -eq 'Release') { 'release' } else { 'release-fast' }
    if ($Target) {
        return (Join-Path $RepoRoot "target\$Target\$profileDir")
    }
    return (Join-Path $RepoRoot "target\$profileDir")
}

function Invoke-EasytierPnpmInstall {
    param([switch]$SkipInstall)
    $needInstall = -not (Test-Path (Join-Path $RepoRoot 'node_modules'))
    if ($SkipInstall -and -not $needInstall) {
        Write-Log 'Skip pnpm install (-SkipInstall)' -Level Info
        return
    }
    if ($SkipInstall -and $needInstall) {
        Write-Warning 'node_modules missing; running pnpm install despite -SkipInstall'
    }
    Write-Step 'pnpm -r install'
    pnpm -r install
    if ($LASTEXITCODE -ne 0) { throw "pnpm install failed (exit $LASTEXITCODE)" }
}

function Invoke-EasytierWebFrontendBuild {
    param([switch]$SkipInstall)
    Assert-Command pnpm
    Assert-Command wasm-pack
    Set-EasytierWritableTemp

    Write-Step 'Ensure local wasm-bindgen (avoid re-download)'
    Ensure-WasmBindgenLocal

    Write-Step 'Ensure local wasm-opt / Binaryen (avoid re-download)'
    Ensure-WasmOptLocal

    Invoke-EasytierPnpmInstall -SkipInstall:$SkipInstall

    Write-Step 'pnpm build easytier-web frontend workspaces'
    pnpm -r --workspace-concurrency=1 --filter "./easytier-web/*" build
    if ($LASTEXITCODE -ne 0) { throw "pnpm frontend build failed (exit $LASTEXITCODE)" }

    $dist = Join-Path $RepoRoot 'easytier-web\frontend\dist'
    if (-not (Test-Path $dist)) {
        throw "Expected frontend dist missing: $dist"
    }
    Write-Log "Frontend dist OK: $dist" -Level Ok
}
