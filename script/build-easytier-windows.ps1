<#
.SYNOPSIS
    One-click Windows package: GUI NSIS + headless zip + web console zip (mirrors CI triad).

.DESCRIPTION
    Aligns with CI windows.yml three products:
      Fast     = headless/web release-fast; GUI/NSIS still --release (Tauri path)
      Release  = --release for GUI + headless + web (closer to CI)
      Dev      = pnpm tauri dev (no installer / zip)

    Steps (package modes):
      1) Copy easytier/third_party/<arch>/*.dll|*.sys into easytier-gui/src-tauri/
      2) Build GUI frontend + tauri NSIS (unless -SkipGui)
      3) Build headless core+cli (unless -SkipHeadless)
      4) Build web embed console (unless -SkipWeb)
      5) Collect into artifacts/ (NSIS / ET-windows-* / ET-windows-*-web)

    Shared helpers: script/build-common.ps1

.PARAMETER Profile
    Fast | Release (default Fast)

.PARAMETER Dev
    Launch GUI via `pnpm tauri dev` (copies runtime DLLs first). No package.

.PARAMETER SkipGui
    Skip GUI / NSIS build.

.PARAMETER SkipHeadless
    Skip headless ET-core / ET-cli collection.

.PARAMETER SkipWeb
    Skip web console (ET-web-embed) collection.

.PARAMETER SkipFrontend
    Skip all pnpm frontend builds (require existing dist trees).

.PARAMETER SkipInstall
    Skip `pnpm -r install`.

.PARAMETER NoZip
    Do not write artifacts/ET-windows-<arch>.zip / ET-windows-<arch>-web.zip (folders still collected).

.PARAMETER OutDir
    Output root (default: <repo>/artifacts).

.PARAMETER Target
    Optional rustc target triple (e.g. x86_64-pc-windows-msvc). Empty = host.

.PARAMETER Interactive
    Prompt for options.

.EXAMPLE
    .\script\build-easytier-windows.ps1
    .\script\build-easytier-windows.ps1 -Profile Release
    .\script\build-easytier-windows.ps1 -Dev
    .\script\build-easytier-windows.ps1 -SkipGui -SkipWeb -Profile Fast
    .\script\build-easytier-windows.ps1 -Interactive
#>
param(
    [ValidateSet('Fast', 'Release')]
    [string]$Profile = 'Fast',

    [switch]$Dev,

    [switch]$SkipGui,
    [switch]$SkipHeadless,
    [switch]$SkipWeb,
    [switch]$SkipFrontend,
    [switch]$SkipInstall,
    [switch]$NoZip,

    [string]$OutDir = '',

    [string]$Target = '',

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
    Write-Host '  EasyTier Windows package'
    Write-Host '========================================'
    Write-Host ''
    Write-Host 'Build profile:'
    Write-Host '  [1] Fast     - release-fast (default)'
    Write-Host '  [2] Release  - --release (production)'
    $c = Read-MenuChoice -Prompt 'Choose [1/2] (Enter=1)' -Valid @('1', '2') -Default '1'
    $Profile = if ($c -eq '2') { 'Release' } else { 'Fast' }

    Write-Host ''
    Write-Host 'What to build?'
    Write-Host '  [1] All three (GUI + headless + web console; default, like CI)'
    Write-Host '  [2] GUI only (NSIS)'
    Write-Host '  [3] Headless only (ET-windows zip; core+cli)'
    Write-Host '  [4] Web console only (ET-windows-*-web)'
    $c = Read-MenuChoice -Prompt 'Choose [1/2/3/4] (Enter=1)' -Valid @('1', '2', '3', '4') -Default '1'
    $SkipGui = ($c -eq '3' -or $c -eq '4')
    $SkipHeadless = ($c -eq '2' -or $c -eq '4')
    $SkipWeb = ($c -eq '2' -or $c -eq '3')

    Write-Host ''
    Write-Host 'Build frontends (pnpm)?'
    Write-Host '  [1] Yes - install + build (default)'
    Write-Host '  [2] Yes - build only (skip pnpm install)'
    Write-Host '  [3] No  - skip (need existing dist)'
    $c = Read-MenuChoice -Prompt 'Choose [1/2/3] (Enter=1)' -Valid @('1', '2', '3') -Default '1'
    $SkipFrontend = $false
    $SkipInstall = $false
    if ($c -eq '2') { $SkipInstall = $true }
    if ($c -eq '3') { $SkipFrontend = $true }

    Write-Host ''
    $OutDir = Read-Host 'Output dir (Enter=artifacts)'
    if ($null -eq $OutDir) { $OutDir = '' }
    $OutDir = $OutDir.Trim()

    Write-Host ''
    Write-Host '----------------------------------------'
    Write-Host "  Profile  : $Profile"
    Write-Host ("  GUI      : " + ($(if ($SkipGui) { 'skip' } else { 'yes' })))
    Write-Host ("  Headless : " + ($(if ($SkipHeadless) { 'skip' } else { 'yes' })))
    Write-Host ("  Web      : " + ($(if ($SkipWeb) { 'skip' } else { 'yes' })))
    Write-Host ("  Frontend : " + ($(if ($SkipFrontend) { 'skip' } elseif ($SkipInstall) { 'build only' } else { 'install + build' })))
    Write-Host ("  OutDir   : " + ($(if ($OutDir) { $OutDir } else { 'artifacts' })))
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
Start-EasytierBuildTranscript -Name 'easytier-windows'

try {

$ArchDir = Get-EasytierWindowsArchDir -Target $Target
$ArtifactStem = "windows-$ArchDir"
if (-not $OutDir) {
    $OutDir = Join-Path $RepoRoot 'artifacts'
}
elseif (-not [System.IO.Path]::IsPathRooted($OutDir)) {
    $OutDir = Join-Path $RepoRoot $OutDir
}

$GuiSrcTauri = Join-Path $RepoRoot 'easytier-gui\src-tauri'
$GuiDir = Join-Path $RepoRoot 'easytier-gui'

function Start-EasytierGuiDev {
    Assert-Command pnpm
    Assert-Command cargo
    Ensure-WindowsBuildPrereqs
    Set-EasytierWritableTemp

    Write-Step "Copy third_party runtime ($ArchDir) into src-tauri"
    Copy-EasytierThirdPartyRuntime -ArchDir $ArchDir -DestDir $GuiSrcTauri

    Invoke-EasytierPnpmInstall -SkipInstall:$SkipInstall

    $devCmd = @"
`$Host.UI.RawUI.WindowTitle = 'EasyTier GUI (tauri dev)'
Set-Location '$GuiDir'
Write-Host 'EasyTier GUI - pnpm tauri dev' -ForegroundColor Cyan
Write-Host ''
pnpm tauri dev
Write-Host ''
Write-Host 'GUI exited. Press Enter to close.' -ForegroundColor Yellow
Read-Host | Out-Null
"@

    Write-Step 'Start GUI window (pnpm tauri dev)'
    Start-Process -FilePath 'powershell.exe' -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-NoExit', '-Command', $devCmd
    ) | Out-Null

    Write-Host ''
    Write-Host 'Debug mode started (new window).' -ForegroundColor Green
    Write-Host '  Close that window to stop. This launcher will exit.'
    Write-Host ''
}

function Remove-EasytierGuiCiConf {
    # Leftover from a prior tauri build; must not sit under easytier-gui/ during pnpm lint.
    $ciConf = Join-Path $GuiDir 'tauri.ci.conf.json'
    if (Test-Path $ciConf) {
        Remove-Item -Force $ciConf
        Write-Log "Removed stale $ciConf" -Level Info
    }
}

function Invoke-EasytierGuiFrontendBuild {
    Assert-Command pnpm
    Set-EasytierWritableTemp
    Invoke-EasytierPnpmInstall -SkipInstall:$SkipInstall
    Remove-EasytierGuiCiConf

    # Build GUI deps once (frontend-lib, vpnservice-api, …), then GUI app only.
    # Avoid `easytier-gui...` + package.json `build` which would compile frontend-lib twice.
    Write-Step 'pnpm build easytier-gui dependencies (easytier-gui^...)'
    $code = Invoke-NativeLogged -FilePath 'pnpm' -ArgumentList @(
        '-r', '--workspace-concurrency=1', '--filter', 'easytier-gui^...', 'build'
    )
    if ($code -ne 0) { throw "pnpm GUI dependency build failed (exit $code)" }

    Write-Step 'pnpm build:app easytier-gui (lint + vue-tsc + vite; frontend-lib already built)'
    Push-Location $GuiDir
    try {
        $code = Invoke-NativeLogged -FilePath 'pnpm' -ArgumentList @('run', 'build:app')
        if ($code -ne 0) { throw "pnpm GUI build:app failed (exit $code)" }
    }
    finally {
        Pop-Location
    }

    $dist = Join-Path $GuiDir 'dist\index.html'
    if (-not (Test-Path $dist)) {
        throw "Expected GUI dist missing: $dist"
    }
    Write-Log "GUI dist OK: $dist" -Level Ok
}

function Invoke-EasytierGuiTauriBuild {
    Assert-Command pnpm
    Ensure-WindowsBuildPrereqs
    # Re-assert TEMP: long GUI builds can outlive a manual delete of old cache dirs.
    Set-EasytierWritableTemp

    # Tauri externalBin: binaries/ET-cli-<triple>.exe → installed as ET-cli.exe.
    Stage-GuiEtCliSidecar -TargetTriple $Target -CargoProfile 'release'

    # Skip Tauri beforeBuildCommand — frontend already built (same as CI tauri.ci.conf.json).
    # Written only for this step; gitignored + eslint-ignored.
    $ciConf = Join-Path $GuiDir 'tauri.ci.conf.json'
    $ciConfBody = @'
{
  "build": {
    "beforeBuildCommand": ""
  }
}
'@
    Set-Content -Path $ciConf -Value $ciConfBody -Encoding utf8

    # NSIS bundler resolves binaries under target/**/release (not custom profiles).
    # Always use cargo --release for GUI; -Profile Fast only speeds the headless half.
    $tauriArgs = @('tauri', 'build', '--verbose', '--ci', '--bundles', 'nsis', '--config', 'tauri.ci.conf.json')
    if ($Target) {
        $tauriArgs += '--target', $Target
    }
    if ($Profile -eq 'Fast') {
        Write-Log 'Note: GUI/NSIS always uses --release (Tauri bundle path); headless uses release-fast.' -Level Info
    }

    Write-Step ("pnpm " + ($tauriArgs -join ' ') + "  (cwd: easytier-gui)")
    Push-Location $GuiDir
    try {
        $code = Invoke-NativeLogged -FilePath 'pnpm' -ArgumentList $tauriArgs
        if ($code -ne 0) { throw "tauri build failed (exit $code)" }
    }
    finally {
        Pop-Location
    }
}

function Find-NsisInstallers {
    # GUI always builds with cargo --release (see Invoke-EasytierGuiTauriBuild).
    # Packaging sets CARGO_TARGET_DIR=.cache\cargo-target — search only that tree so
    # leftover repo\target\...\ET_<oldver>_*-setup.exe are never collected again.
    $guiProfileDir = 'release'
    $targetRoot = if ($env:CARGO_TARGET_DIR) {
        $env:CARGO_TARGET_DIR
    }
    else {
        Join-Path $RepoRoot 'target'
    }
    $candidates = @()
    if ($Target) {
        $candidates += Join-Path $targetRoot "$Target\$guiProfileDir\bundle\nsis"
    }
    $candidates += Join-Path $targetRoot "$guiProfileDir\bundle\nsis"

    $found = @()
    foreach ($dir in ($candidates | Select-Object -Unique)) {
        if (Test-Path $dir) {
            $found += @(Get-ChildItem $dir -Filter '*setup.exe' -File -ErrorAction SilentlyContinue)
        }
    }
    if ($found.Count -eq 0) {
        return @()
    }

    # Prefer installer matching tauri.conf.json version; else newest by mtime.
    # Version bumps leave prior ET_<ver>_*-setup.exe beside the new one in the same folder.
    $ver = $null
    $tauriConf = Join-Path $GuiSrcTauri 'tauri.conf.json'
    if (Test-Path $tauriConf) {
        try {
            $ver = (Get-Content -LiteralPath $tauriConf -Raw | ConvertFrom-Json).version
        }
        catch { }
    }
    if ($ver) {
        $matched = @($found | Where-Object { $_.Name -like "*_${ver}_*" })
        if ($matched.Count -gt 0) {
            $found = $matched
        }
    }
    $newest = $found | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    return ,@($newest)
}

function Invoke-EasytierHeadlessBuild {
    Assert-Command cargo
    Ensure-WindowsBuildPrereqs

    # Node product only (core + cli). Web console is a separate package.
    $cargoArgs = @('build', '-p', 'easytier')
    if ($Profile -eq 'Release') {
        $cargoArgs += '--release'
    }
    else {
        $cargoArgs += '--profile', 'release-fast'
    }
    if ($Target) {
        $cargoArgs += '--target', $Target
    }

    Write-Step ("cargo " + ($cargoArgs -join ' '))
    $code = Invoke-NativeLogged -FilePath 'cargo' -ArgumentList $cargoArgs
    if ($code -ne 0) { throw "cargo headless build failed (exit $code)" }

    $artifactDir = [string](Get-CargoArtifactDir -Profile $Profile -Target $Target)
    return , $artifactDir
}

function Invoke-EasytierWebConsoleBuild {
    Assert-Command cargo
    Ensure-WindowsBuildPrereqs

    if (-not $SkipFrontend) {
        Invoke-EasytierWebFrontendBuild -SkipInstall:$SkipInstall
    }
    else {
        $dist = Join-Path $RepoRoot 'easytier-web\frontend\dist\index.html'
        if (-not (Test-Path $dist)) {
            throw "Web console embed needs frontend dist: $dist"
        }
    }

    $cargoArgs = @('build', '-p', 'easytier-web', '--features', 'easytier-web/embed')
    if ($Profile -eq 'Release') {
        $cargoArgs += '--release'
    }
    else {
        $cargoArgs += '--profile', 'release-fast'
    }
    if ($Target) {
        $cargoArgs += '--target', $Target
    }

    Write-Step ("cargo " + ($cargoArgs -join ' '))
    $code = Invoke-NativeLogged -FilePath 'cargo' -ArgumentList $cargoArgs
    if ($code -ne 0) { throw "cargo web-embed build failed (exit $code)" }

    $artifactDir = [string](Get-CargoArtifactDir -Profile $Profile -Target $Target)
    $webBuilt = Join-Path $artifactDir 'easytier-web.exe'
    $webEmbed = Join-Path $artifactDir 'easytier-web-embed.exe'
    if (-not (Test-Path $webBuilt)) {
        throw "Build artifact not found: $webBuilt"
    }
    Copy-Item -Force $webBuilt $webEmbed | Out-Null
    return , $artifactDir
}

function Collect-HeadlessArtifacts {
    param([Parameter(Mandatory)][string]$ArtifactDir)

    $headlessDir = Join-Path $OutDir "ET-$ArtifactStem"
    if (Test-Path $headlessDir) {
        Remove-Item -Recurse -Force $headlessDir
    }
    New-Item -ItemType Directory -Force -Path $headlessDir | Out-Null

    $rename = @{
        'easytier-core.exe' = 'ET-core.exe'
        'easytier-cli.exe'  = 'ET-cli.exe'
    }
    foreach ($srcName in $rename.Keys) {
        $src = Join-Path $ArtifactDir $srcName
        if (-not (Test-Path $src)) {
            throw "Missing headless binary: $src"
        }
        Copy-Item -Force $src (Join-Path $headlessDir $rename[$srcName])
    }

    Copy-EasytierThirdPartyRuntime -ArchDir $ArchDir -DestDir $headlessDir

    foreach ($must in @('ET-core.exe', 'ET-cli.exe')) {
        if (-not (Test-Path (Join-Path $headlessDir $must))) {
            throw "Headless collect incomplete: missing $must"
        }
    }

    Write-Log "Headless folder: $headlessDir" -Level Ok

    if (-not $NoZip) {
        $zipPath = Join-Path $OutDir "ET-$ArtifactStem.zip"
        if (Test-Path $zipPath) {
            Remove-Item -Force $zipPath
        }
        Write-Step "Zip headless -> $zipPath"
        Compress-Archive -Path (Join-Path $headlessDir '*') -DestinationPath $zipPath -Force
        Write-Log "Headless zip: $zipPath" -Level Ok
    }

    return $headlessDir
}

function Collect-WebConsoleArtifacts {
    param([Parameter(Mandatory)][string]$ArtifactDir)

    $webDir = Join-Path $OutDir "ET-$ArtifactStem-web"
    if (Test-Path $webDir) {
        Remove-Item -Recurse -Force $webDir
    }
    New-Item -ItemType Directory -Force -Path $webDir | Out-Null

    $src = Join-Path $ArtifactDir 'easytier-web-embed.exe'
    if (-not (Test-Path $src)) {
        throw "Missing web console binary: $src"
    }
    Copy-Item -Force $src (Join-Path $webDir 'ET-web-embed.exe')
    Write-Log "Web console folder: $webDir" -Level Ok

    if (-not $NoZip) {
        $zipPath = Join-Path $OutDir "ET-$ArtifactStem-web.zip"
        if (Test-Path $zipPath) {
            Remove-Item -Force $zipPath
        }
        Write-Step "Zip web console -> $zipPath"
        Compress-Archive -Path (Join-Path $webDir '*') -DestinationPath $zipPath -Force
        Write-Log "Web console zip: $zipPath" -Level Ok
    }

    return $webDir
}

function Collect-GuiArtifacts {
    $nsis = @(Find-NsisInstallers)
    if ($nsis.Count -eq 0) {
        throw 'No NSIS installer found under $CARGO_TARGET_DIR/**/release/bundle/nsis'
    }

    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    # Expand names first: under Set-StrictMode, `$nsis.Name` on a 1-element
    # array can fail / behave oddly; never use member enumeration here.
    $nsisNames = @($nsis | ForEach-Object { $_.Name })
    # Drop stale versioned installers left in OutDir from earlier builds.
    Get-ChildItem -LiteralPath $OutDir -Filter 'ET_*setup.exe' -File -ErrorAction SilentlyContinue |
        Where-Object { $nsisNames -notcontains $_.Name } |
        ForEach-Object {
            Write-Log "Remove stale GUI installer: $($_.FullName)" -Level Info
            Remove-Item -LiteralPath $_.FullName -Force
        }

    $copied = @()
    foreach ($exe in $nsis) {
        $dest = Join-Path $OutDir $exe.Name
        Copy-Item -Force $exe.FullName $dest
        $copied += $dest
        Write-Log "GUI installer: $dest" -Level Ok
    }
    return $copied
}

# --- Dev ---------------------------------------------------------------------
if ($Dev) {
    Start-EasytierGuiDev
    exit 0
}

if ($SkipGui -and $SkipHeadless) {
    throw 'Nothing to build: both -SkipGui and -SkipHeadless are set.'
}

Write-Log 'EasyTier Windows package'
Write-Log "  Repo:     $RepoRoot"
Write-Log "  Profile:  $Profile"
Write-Log "  Arch:     $ArchDir"
Write-Log "  Target:   $(if ($Target) { $Target } else { '(host)' })"
Write-Log "  GUI:      $(-not $SkipGui)"
Write-Log "  Headless: $(-not $SkipHeadless)"
Write-Log "  OutDir:   $OutDir"

# Short cargo target + writable TEMP under .cache\ (link.exe needs TEMP for .tmp).
Initialize-EasytierWindowsCargoEnv
Set-EasytierWritableTemp

# Fail fast: NSIS embedBootstrapper needs a local WebView2 setup exe.
# Without it Tauri downloads from Microsoft at the very end (often EOF behind proxies).
if (-not $SkipGui) {
    Write-Step 'Check local WebView2 bootstrapper (NSIS embed)'
    Ensure-WebView2BootstrapperLocal
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

Write-Step "Copy third_party runtime ($ArchDir) into src-tauri (GUI resources)"
Copy-EasytierThirdPartyRuntime -ArchDir $ArchDir -DestDir $GuiSrcTauri

$guiOutputs = @()
$headlessDir = $null
$webDir = $null

if (-not $SkipGui) {
    if (-not $SkipFrontend) {
        Invoke-EasytierGuiFrontendBuild
    }
    else {
        $dist = Join-Path $GuiDir 'dist\index.html'
        if (-not (Test-Path $dist)) {
            throw "GUI build needs dist: $dist"
        }
    }
    # Stage-GuiEtCliSidecar runs inside Invoke-EasytierGuiTauriBuild.
    Invoke-EasytierGuiTauriBuild
    $guiOutputs = @(Collect-GuiArtifacts)
}

if (-not $SkipHeadless) {
    $artifactDir = [string]((@(Invoke-EasytierHeadlessBuild) | Select-Object -Last 1))
    if ([string]::IsNullOrWhiteSpace($artifactDir) -or -not (Test-Path -LiteralPath $artifactDir)) {
        throw "Headless artifact directory invalid: '$artifactDir'"
    }
    $headlessDir = Collect-HeadlessArtifacts -ArtifactDir $artifactDir
}

if (-not $SkipWeb) {
    # If GUI already ran pnpm install, SkipInstall for the web frontend step.
    if (-not $SkipGui -and -not $SkipFrontend) {
        $SkipInstall = $true
    }
    $webArtifactDir = [string]((@(Invoke-EasytierWebConsoleBuild) | Select-Object -Last 1))
    if ([string]::IsNullOrWhiteSpace($webArtifactDir) -or -not (Test-Path -LiteralPath $webArtifactDir)) {
        throw "Web console artifact directory invalid: '$webArtifactDir'"
    }
    $webDir = Collect-WebConsoleArtifacts -ArtifactDir $webArtifactDir
}

Write-Host ''
Write-Log 'Build OK' -Level Ok
if ($guiOutputs.Count -gt 0) {
    Write-Log '  GUI NSIS:'
    foreach ($p in $guiOutputs) { Write-Log "    $p" }
}
if ($headlessDir) {
    Write-Log "  Headless (core+cli): $headlessDir"
    if (-not $NoZip) {
        Write-Log ("  Zip:      " + (Join-Path $OutDir "ET-$ArtifactStem.zip"))
    }
}
if ($webDir) {
    Write-Log "  Web console: $webDir"
    if (-not $NoZip) {
        Write-Log ("  Zip:      " + (Join-Path $OutDir "ET-$ArtifactStem-web.zip"))
    }
}
Write-EasytierBuildElapsed
Write-Host ''
Write-Log 'Same layout as CI: ET-windows-* / ET-windows-*-web / NSIS (local OutDir).'

} finally {
    Stop-EasytierBuildTranscript
}
