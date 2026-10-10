<#
.SYNOPSIS
    Build easytier-web (frontend + optional embed binary).

.DESCRIPTION
    1) pnpm install + build easytier-web frontend workspace
    2) cargo build easytier-web with --features embed (default)
    Copies a renamed easytier-web-embed(.exe) next to the cargo artifact.

    Shared helpers live in script/build-common.ps1 (also used by build-easytier-windows.ps1).

.PARAMETER Profile
    Fast     = cargo --profile release-fast  (daily / local, default)
    Release  = cargo --release               (production)

.PARAMETER Dev
    Debug mode: cargo run (API) + Vite frontend in two windows; no package exe.

.PARAMETER SkipFrontend
    Skip pnpm install/build; only compile Rust (frontend/dist must already exist for embed).

.PARAMETER SkipInstall
    Skip `pnpm -r install`; only run frontend build + cargo.

.PARAMETER NoEmbed
    Build API-only binary (no --features embed). Output keeps name easytier-web(.exe).

.PARAMETER OutDir
    Optional directory to copy the final binary into (created if missing).

.PARAMETER Interactive
    Prompt for Profile / frontend / embed / OutDir.

.EXAMPLE
    .\script\build-easytier-web.ps1
    .\script\build-easytier-web.ps1 -Profile Release
    .\script\build-easytier-web.ps1 -Dev
    .\script\build-easytier-web.ps1 -SkipFrontend -Profile Fast
    .\script\build-easytier-web.ps1 -Profile Release -OutDir D:\deploy\easytier-web
    .\script\build-easytier-web.ps1 -Interactive
#>
param(
    [ValidateSet('Fast', 'Release')]
    [string]$Profile = 'Fast',

    [switch]$Dev,

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

$CargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
. (Join-Path $PSScriptRoot 'build-common.ps1')
Start-EasytierBuildTranscript -Name 'easytier-web'

try {

function Start-EasytierWebDev {
    <#
      Debug: cargo run (debug API) + Vite frontend; no embed exe.
    #>
    Assert-Command cargo
    Assert-Command pnpm
    Ensure-WindowsBuildPrereqs
    Set-EasytierWritableTemp

    $frontendDir = Join-Path $RepoRoot 'easytier-web\frontend'
    $dbPath = Join-Path $RepoRoot 'et.db'
    $apiUrl = 'http://localhost:11211'
    $webHint = 'http://localhost:5173'
    $etTemp = $env:TEMP

    Invoke-EasytierPnpmInstall -SkipInstall:$SkipInstall

    $backendCmd = @"
`$Host.UI.RawUI.WindowTitle = 'EasyTier-web API (debug)'
Set-Location '$RepoRoot'
Write-Host 'EasyTier-web backend - cargo run (debug, no embed)' -ForegroundColor Cyan
Write-Host '  DB:  $dbPath'
Write-Host '  API: $apiUrl'
Write-Host ''
cargo run -p easytier-web -- --db `"$dbPath`" --console-log-level debug
Write-Host ''
Write-Host 'Backend exited. Press Enter to close.' -ForegroundColor Yellow
Read-Host | Out-Null
"@

    $frontendCmd = @"
`$Host.UI.RawUI.WindowTitle = 'EasyTier-web frontend (Vite)'
Set-Location '$frontendDir'
`$env:TEMP = '$etTemp'
`$env:TMP = '$etTemp'
Write-Host 'EasyTier-web frontend - pnpm dev (Vite)' -ForegroundColor Cyan
Write-Host '  proxy /api -> $apiUrl'
Write-Host '  waiting for API port 11211 (cargo may still be compiling) ...'
`$deadline = (Get-Date).AddMinutes(8)
do {
  `$ok = `$false
  try {
    `$tcp = New-Object System.Net.Sockets.TcpClient
    `$tcp.Connect('127.0.0.1', 11211)
    `$tcp.Close()
    `$ok = `$true
  } catch {
    Start-Sleep -Seconds 2
  }
} while (-not `$ok -and (Get-Date) -lt `$deadline)
if (-not `$ok) {
  Write-Host '  WARNING: API port not open yet; Vite will start anyway (proxy errors until backend is up).' -ForegroundColor Yellow
} else {
  Write-Host '  API port is open.' -ForegroundColor Green
}
Write-Host '  open the Local URL printed by Vite'
Write-Host '  typical URL: $webHint'
Write-Host ''
pnpm dev
Write-Host ''
Write-Host 'Frontend exited. Press Enter to close.' -ForegroundColor Yellow
Read-Host | Out-Null
"@

    Write-Step 'Start backend window (cargo run -p easytier-web)'
    Start-Process -FilePath 'powershell.exe' -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-NoExit', '-Command', $backendCmd
    ) | Out-Null

    Write-Step 'Start frontend window (waits for API, then pnpm dev)'
    Start-Process -FilePath 'powershell.exe' -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-NoExit', '-Command', $frontendCmd
    ) | Out-Null

    Write-Host ''
    Write-Host 'Debug mode started (two new windows).' -ForegroundColor Green
    Write-Host "  API:  $apiUrl"
    Write-Host "  Web:  $webHint  (use the port Vite prints)"
    Write-Host "  DB:   $dbPath"
    Write-Host ''
    Write-Host 'Wait for cargo compile + Vite Local URL, then open the frontend in a browser.'
    Write-Host 'Close those two windows to stop. This launcher will exit.'
    Write-Host ''
}

if ($Dev) {
    Start-EasytierWebDev
    exit 0
}

Write-Log 'EasyTier Web build'
Write-Log "  Repo:    $RepoRoot"
Write-Log "  Profile: $Profile"
Write-Log "  Embed:   $(-not $NoEmbed)"
Write-Log "  Frontend:$(-not $SkipFrontend)"

# --- Frontend ----------------------------------------------------------------
if (-not $SkipFrontend) {
    Invoke-EasytierWebFrontendBuild -SkipInstall:$SkipInstall
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
}
else {
    $cargoArgs += '--profile', 'release-fast'
}
$artifactDir = Get-CargoArtifactDir -Profile $Profile

if (-not $NoEmbed) {
    $cargoArgs += '--features', 'embed'
}

Write-Step ("cargo " + ($cargoArgs -join ' '))
$code = Invoke-NativeLogged -FilePath 'cargo' -ArgumentList $cargoArgs
if ($code -ne 0) { throw "cargo build failed (exit $code)" }

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
Write-Log 'Build OK' -Level Ok
Write-Log "  Binary: $finalPath"

if ($OutDir) {
    if (-not (Test-Path $OutDir)) {
        New-Item -ItemType Directory -Path $OutDir | Out-Null
    }
    $dest = Join-Path $OutDir $finalName
    Copy-Item -Force $finalPath $dest
    Write-Log "  Copied: $dest" -Level Ok
}

Write-EasytierBuildElapsed

Write-Host ""
Write-Log 'Run example:'
Write-Log "  & '$finalPath' --db `"$RepoRoot\et.db`""

} finally {
    Stop-EasytierBuildTranscript
}
