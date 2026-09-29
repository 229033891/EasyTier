<#
.SYNOPSIS
    Build easytier-web (frontend + optional embed binary).

.DESCRIPTION
    1) pnpm install + build easytier-web frontend workspace
    2) cargo build easytier-web with --features embed (default)
    Copies a renamed easytier-web-embed(.exe) next to the cargo artifact.

.PARAMETER Profile
    Fast     = cargo --profile release-fast  (daily / local, default)
    Release  = cargo --release               (production)

.PARAMETER SkipFrontend
    Skip pnpm install/build; only compile Rust (frontend/dist must already exist for embed).

.PARAMETER SkipInstall
    Skip `pnpm -r install`; only run frontend build + cargo.

.PARAMETER NoEmbed
    Build API-only binary (no --features embed). Output keeps name easytier-web(.exe).

.PARAMETER OutDir
    Optional directory to copy the final binary into (created if missing).

.PARAMETER Interactive
    Prompt for Profile / frontend / embed / OutDir (used by build-easytier-web.cmd).

.EXAMPLE
    .\script\build-easytier-web.ps1
    .\script\build-easytier-web.ps1 -Profile Release
    .\script\build-easytier-web.ps1 -SkipFrontend -Profile Fast
    .\script\build-easytier-web.ps1 -Profile Release -OutDir D:\deploy\easytier-web
    .\script\build-easytier-web.ps1 -Interactive
#>
param(
    [ValidateSet('Fast', 'Release')]
    [string]$Profile = 'Fast',

    [switch]$SkipFrontend,
    [switch]$SkipInstall,
    [switch]$NoEmbed,

    [string]$OutDir = '',

    [switch]$Interactive
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

function Read-MenuChoice {
    param(
        [string]$Prompt,
        [string[]]$Valid,
        [string]$Default
    )
    $raw = Read-Host $Prompt
    if ([string]::IsNullOrWhiteSpace($raw)) { return $Default }
    $raw = $raw.Trim()
    if ($Valid -contains $raw) { return $raw }
    Write-Host "Invalid choice '$raw', using default: $Default" -ForegroundColor Yellow
    return $Default
}

if ($Interactive) {
    Write-Host ''
    Write-Host '========================================'
    Write-Host '  EasyTier Web build'
    Write-Host '========================================'
    Write-Host ''
    Write-Host 'Local tools will be reused when present:'
    Write-Host '  wasm-bindgen: PATH / script\tools\wasm-bindgen-<ver> / %LOCALAPPDATA%\.wasm-pack'
    Write-Host '  wasm-opt:     PATH / script\tools\binaryen-version_117\bin / .wasm-pack cache'
    Write-Host '  (drop binaryen-version_117-x86_64-windows.tar.gz into script\tools\ to auto-extract)'
    Write-Host ''

    Write-Host 'Build profile:'
    Write-Host '  [1] Fast     - release-fast (default, faster link)'
    Write-Host '  [2] Release  - --release (production)'
    $c = Read-MenuChoice -Prompt 'Choose [1/2] (Enter=1)' -Valid @('1', '2') -Default '1'
    $Profile = if ($c -eq '2') { 'Release' } else { 'Fast' }

    Write-Host ''
    Write-Host 'Build frontend (pnpm)?'
    Write-Host '  [1] Yes - install + build (default)'
    Write-Host '  [2] Yes - build only (skip pnpm install)'
    Write-Host '  [3] No  - skip frontend (need existing frontend/dist)'
    $c = Read-MenuChoice -Prompt 'Choose [1/2/3] (Enter=1)' -Valid @('1', '2', '3') -Default '1'
    $SkipFrontend = $false
    $SkipInstall = $false
    if ($c -eq '2') { $SkipInstall = $true }
    if ($c -eq '3') { $SkipFrontend = $true }

    Write-Host ''
    Write-Host 'Embed frontend (--features embed)?'
    Write-Host '  [1] Yes - easytier-web-embed (default)'
    Write-Host '  [2] No  - API only (easytier-web)'
    $c = Read-MenuChoice -Prompt 'Choose [1/2] (Enter=1)' -Valid @('1', '2') -Default '1'
    $NoEmbed = ($c -eq '2')

    Write-Host ''
    $OutDir = Read-Host 'Optional output copy dir (Enter=skip)'
    if ($null -eq $OutDir) { $OutDir = '' }
    $OutDir = $OutDir.Trim()

    Write-Host ''
    Write-Host '----------------------------------------'
    Write-Host "  Profile : $Profile"
    if ($SkipFrontend) {
        Write-Host '  Frontend: skip'
    } elseif ($SkipInstall) {
        Write-Host '  Frontend: build only (skip install)'
    } else {
        Write-Host '  Frontend: install + build'
    }
    Write-Host ("  Embed   : " + ($(if ($NoEmbed) { 'No' } else { 'Yes' })))
    Write-Host ("  OutDir  : " + ($(if ($OutDir) { $OutDir } else { '(none)' })))
    Write-Host '----------------------------------------'
    Write-Host ''
    $confirm = Read-Host 'Start build? [Y/n] (Enter=Y)'
    if ($confirm -match '^[nN]') {
        Write-Host 'Cancelled.'
        exit 0
    }
    Write-Host ''
}

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $RepoRoot

# Prefer local cargo bin so wasm-pack reuses a preinstalled wasm-bindgen
$CargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (Test-Path $CargoBin) {
    $env:Path = "$CargoBin;" + $env:Path
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
    Write-Host "7z OK: $($sevenZip.Source)" -ForegroundColor Green

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
            Write-Host "VC_LTL=$p" -ForegroundColor Green
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
            Write-Host "YY_THUNKS=$p" -ForegroundColor Green
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
            Write-Host "PROTOC=$p" -ForegroundColor Green
            break
        }
    }
}

function Write-Step([string]$Message) {
    Write-Host ""
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Assert-Command([string]$Name) {
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Required command not found: $Name"
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
        Write-Host "wasm-bindgen $required OK (PATH: $($existing.Source))" -ForegroundColor Green
        # Keep vendor copy in sync for offline reuse
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
        Write-Host "Using vendor wasm-bindgen: $vendorExe"
    }

    # 3) wasm-pack cache under %LOCALAPPDATA%\.wasm-pack\
    if (-not $sourceExe -and (Test-Path $cacheRoot)) {
        foreach ($dir in @(Get-ChildItem $cacheRoot -Directory -Filter 'wasm-bindgen-*' -ErrorAction SilentlyContinue)) {
            $candidate = Join-Path $dir.FullName 'wasm-bindgen.exe'
            if (Test-WasmBindgenAt $candidate $required) {
                $sourceExe = $candidate
                Write-Host "Using cached wasm-bindgen: $candidate"
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

    # Also keep a stable vendor copy for next builds / offline machines
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
    Write-Host "wasm-bindgen $required installed to $dest (reused next time)" -ForegroundColor Green
}

# wasm-pack pins Binaryen version_117 (see wasm-pack install::prebuilt_url_for)
$script:WasmOptBinaryenVersion = 'version_117'
$script:WasmOptWindowsArchiveUrl = "https://github.com/WebAssembly/binaryen/releases/download/$script:WasmOptBinaryenVersion/binaryen-$script:WasmOptBinaryenVersion-x86_64-windows.tar.gz"

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
        Write-Host "wasm-opt OK (PATH: $($existing.Source))" -ForegroundColor Green
        return
    }

    # 2) Vendor extract from local tar.gz if present
    if (-not (Test-WasmOptExe $vendorExe) -and (Test-Path $vendorArchive)) {
        Write-Host "Extracting local Binaryen archive: $vendorArchive"
        Expand-BinaryenArchive -ArchivePath $vendorArchive -VendorRoot $vendorRoot | Out-Null
    }

    # 3) Already extracted vendor
    if (Test-WasmOptExe $vendorExe) {
        $env:Path = "$vendorBin;" + $env:Path
        Write-Host "wasm-opt OK (vendor: $vendorExe)" -ForegroundColor Green
        return
    }

    # 4) wasm-pack cache: .../wasm-opt-*/bin/wasm-opt.exe
    if (Test-Path $cacheRoot) {
        foreach ($dir in @(Get-ChildItem $cacheRoot -Directory -Filter 'wasm-opt-*' -ErrorAction SilentlyContinue)) {
            $candidate = Join-Path $dir.FullName 'bin\wasm-opt.exe'
            if (-not (Test-Path $candidate)) {
                $candidate = Join-Path $dir.FullName 'wasm-opt.exe'
            }
            if (Test-WasmOptExe $candidate) {
                $binDir = Split-Path $candidate -Parent
                $env:Path = "$binDir;" + $env:Path
                Write-Host "wasm-opt OK (cache: $candidate)" -ForegroundColor Green
                # Mirror into vendor for next time
                if (-not (Test-Path $vendorBin)) {
                    New-Item -ItemType Directory -Path $vendorBin -Force | Out-Null
                }
                Copy-Item -Force (Join-Path $binDir '*') $vendorBin
                return
            }
        }
    }

    # 5) Try download into script/tools (may fail behind restricted network)
    Write-Host "Attempting to download Binaryen $script:WasmOptBinaryenVersion ..."
    try {
        Invoke-WebRequest -Uri $script:WasmOptWindowsArchiveUrl -OutFile $vendorArchive -UseBasicParsing
        Expand-BinaryenArchive -ArchivePath $vendorArchive -VendorRoot $vendorRoot | Out-Null
        $env:Path = "$vendorBin;" + $env:Path
        if (Test-WasmOptExe $vendorExe) {
            Write-Host "wasm-opt OK (downloaded to $vendorExe)" -ForegroundColor Green
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
  3) Re-run build-easytier-web.cmd

wasm-pack will reuse PATH/vendor and will not re-download.
"@
}

Write-Host "EasyTier Web build"
Write-Host "  Repo:    $RepoRoot"
Write-Host "  Profile: $Profile"
Write-Host "  Embed:   $(-not $NoEmbed)"
Write-Host "  Frontend:$(-not $SkipFrontend)"

# --- Frontend ----------------------------------------------------------------
if (-not $SkipFrontend) {
    Assert-Command pnpm
    Assert-Command wasm-pack

    Write-Step 'Ensure local wasm-bindgen (avoid re-download)'
    Ensure-WasmBindgenLocal

    Write-Step 'Ensure local wasm-opt / Binaryen (avoid re-download)'
    Ensure-WasmOptLocal

    if (-not $SkipInstall) {
        Write-Step "pnpm -r install"
        pnpm -r install
        if ($LASTEXITCODE -ne 0) { throw "pnpm install failed (exit $LASTEXITCODE)" }
    }

    Write-Step 'pnpm build easytier-web frontend workspaces'
    pnpm -r --workspace-concurrency=1 --filter "./easytier-web/*" build
    if ($LASTEXITCODE -ne 0) { throw "pnpm frontend build failed (exit $LASTEXITCODE)" }

    $dist = Join-Path $RepoRoot 'easytier-web\frontend\dist'
    if (-not (Test-Path $dist)) {
        throw "Expected frontend dist missing: $dist"
    }
    Write-Host "Frontend dist OK: $dist" -ForegroundColor Green
}
elseif (-not $NoEmbed) {
    $dist = Join-Path $RepoRoot 'easytier-web\frontend\dist'
    if (-not (Test-Path $dist)) {
        Write-Warning "embed build but dist missing: $dist — UI may be empty/404"
    }
}

# --- Rust --------------------------------------------------------------------
Assert-Command cargo
Ensure-WindowsBuildPrereqs

$cargoArgs = @('build', '--package', 'easytier-web')
if ($Profile -eq 'Release') {
    $cargoArgs += '--release'
    $artifactDir = Join-Path $RepoRoot 'target\release'
}
else {
    $cargoArgs += '--profile', 'release-fast'
    $artifactDir = Join-Path $RepoRoot 'target\release-fast'
}

if (-not $NoEmbed) {
    $cargoArgs += '--features', 'embed'
}

Write-Step ("cargo " + ($cargoArgs -join ' '))
& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)" }

$isWin = ($env:OS -match 'Windows') -or ($PSVersionTable.Platform -eq 'Win32NT')
$builtName = if ($isWin) { 'easytier-web.exe' } else { 'easytier-web' }
$builtPath = Join-Path $artifactDir $builtName
if (-not (Test-Path $builtPath)) {
    throw "Build artifact not found: $builtPath"
}

# Rename copy for embed products
$finalName = if ($NoEmbed) {
    $builtName
} elseif ($isWin) {
    'easytier-web-embed.exe'
} else {
    'easytier-web-embed'
}
$finalPath = Join-Path $artifactDir $finalName
if ($finalName -ne $builtName) {
    Copy-Item -Force $builtPath $finalPath
}

Write-Host ""
Write-Host "Build OK" -ForegroundColor Green
Write-Host "  Binary: $finalPath"

if ($OutDir) {
    if (-not (Test-Path $OutDir)) {
        New-Item -ItemType Directory -Path $OutDir | Out-Null
    }
    $dest = Join-Path $OutDir $finalName
    Copy-Item -Force $finalPath $dest
    Write-Host "  Copied: $dest" -ForegroundColor Green
}

Write-Host ""
Write-Host "Run example:"
Write-Host "  & '$finalPath' --db `"$RepoRoot\et.db`""
