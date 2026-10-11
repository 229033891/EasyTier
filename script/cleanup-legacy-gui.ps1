<#
.SYNOPSIS
    Remove leftover EasyTier GUI files/services from the easytier-gui -> ET rename.

.DESCRIPTION
    Safe cleanup for Windows after upgrading to mainBinaryName=ET:
      - stop legacy processes (easytier-gui.exe)
      - remove legacy Windows service name "easytier-gui"
      - rewrite ET-Gui / easytier-gui service PathName if it still points at easytier-gui.exe
      - delete easytier-gui.exe (+ .bak) under known install roots when ET.exe is present
      - optional: remove Start Menu / Desktop shortcuts that still target easytier-gui.exe

    Does NOT uninstall ET, delete ET.exe, or remove user config/data.

.PARAMETER Apply
    Actually change the system. Default is dry-run (report only).

.PARAMETER AlsoRemoveShim
    When ET.exe exists beside easytier-gui.exe, delete the legacy shim
    (only after PathName migration succeeds or no GUI service is installed).

.PARAMETER InstallDir
    Extra install directory to scan (repeatable via array).

.EXAMPLE
    .\script\cleanup-legacy-gui.ps1
    .\script\cleanup-legacy-gui.ps1 -Apply
    .\script\cleanup-legacy-gui.ps1 -Apply -AlsoRemoveShim
#>
[CmdletBinding()]
param(
    [switch]$Apply,
    [switch]$AlsoRemoveShim,
    [string[]]$InstallDir = @()
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

function Write-Step([string]$Message) {
    Write-Host ""
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Write-Action([string]$Message, [string]$Level = 'Info') {
    $color = switch ($Level) {
        'Ok' { 'Green' }
        'Warn' { 'Yellow' }
        'Err' { 'Red' }
        'Dry' { 'DarkYellow' }
        default { 'Gray' }
    }
    $prefix = if ($Apply) { '[apply]' } else { '[dry-run]' }
    Write-Host ("{0} {1}" -f $prefix, $Message) -ForegroundColor $color
}

function Get-CandidateInstallDirs {
    $dirs = [System.Collections.Generic.List[string]]::new()
    foreach ($p in @(
            (Join-Path $env:LOCALAPPDATA 'ET'),
            (Join-Path ${env:ProgramFiles} 'ET'),
            (Join-Path ${env:ProgramFiles(x86)} 'ET'),
            (Join-Path $env:LOCALAPPDATA 'easytier-gui'),
            (Join-Path ${env:ProgramFiles} 'easytier-gui'),
            (Join-Path ${env:ProgramFiles(x86)} 'easytier-gui')
        )) {
        if ($p -and (Test-Path -LiteralPath $p)) { $dirs.Add($p) }
    }

    foreach ($hive in @('HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
            'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
            'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall')) {
        if (-not (Test-Path $hive)) { continue }
        Get-ChildItem $hive -ErrorAction SilentlyContinue | ForEach-Object {
            try {
                $p = Get-ItemProperty $_.PSPath -ErrorAction Stop
                $name = [string]$p.DisplayName
                $loc = [string]$p.InstallLocation
                if (-not $loc) { return }
                $loc = $loc.Trim('"')
                if ($name -match '^(ET|EasyTier)' -or $loc -match '\\(ET|easytier-gui)\\?$') {
                    if (Test-Path -LiteralPath $loc) { $dirs.Add($loc) }
                }
            }
            catch { }
        }
    }

    foreach ($extra in $InstallDir) {
        if ($extra -and (Test-Path -LiteralPath $extra)) { $dirs.Add($extra) }
    }

    $dirs | Select-Object -Unique
}

function Stop-LegacyGuiProcesses {
    Write-Step 'Stop legacy GUI processes (easytier-gui.exe)'
    $procs = @(Get-Process -Name 'easytier-gui' -ErrorAction SilentlyContinue)
    if ($procs.Count -eq 0) {
        Write-Action 'No easytier-gui.exe process' -Level Ok
        return
    }
    foreach ($p in $procs) {
        Write-Action ("Stop PID {0} ({1})" -f $p.Id, $p.Path) -Level Warn
        if ($Apply) {
            Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
        }
    }
}

function Update-ServiceBinPathIfLegacy {
    param([Parameter(Mandatory)][string]$ServiceName)

    $svc = Get-CimInstance Win32_Service -Filter ("Name='$ServiceName'") -ErrorAction SilentlyContinue
    if (-not $svc) {
        Write-Action ("Service '{0}' not installed" -f $ServiceName) -Level Ok
        return $true
    }

    $path = [string]$svc.PathName
    if ($path -notmatch '(?i)easytier-gui\.exe') {
        Write-Action ("Service '{0}' PathName already OK: {1}" -f $ServiceName, $path) -Level Ok
        return $true
    }

    $newPath = [regex]::Replace($path, 'easytier-gui\.exe', 'ET.exe', 'IgnoreCase')
    Write-Action ("Rewrite '{0}' PathName:`n    {1}`n -> {2}" -f $ServiceName, $path, $newPath) -Level Warn
    if (-not $Apply) { return $false }

    $out = & sc.exe config $ServiceName binPath= $newPath 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Action ("sc config failed for '{0}': {1}" -f $ServiceName, ($out -join ' ')) -Level Err
        return $false
    }
    Write-Action ("Updated '{0}' PathName" -f $ServiceName) -Level Ok
    return $true
}

function Remove-LegacyService {
    param([Parameter(Mandatory)][string]$ServiceName)

    $svc = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
    if (-not $svc) {
        Write-Action ("Legacy service '{0}' not present" -f $ServiceName) -Level Ok
        return
    }

    Write-Action ("Remove legacy service '{0}' (Status={1})" -f $ServiceName, $svc.Status) -Level Warn
    if (-not $Apply) { return }

    & sc.exe failure $ServiceName reset= 0 actions= // | Out-Null
    & sc.exe config $ServiceName start= demand | Out-Null
    & sc.exe stop $ServiceName | Out-Null
    Start-Sleep -Seconds 1
    & sc.exe delete $ServiceName | Out-Null
    if ($LASTEXITCODE -eq 0) {
        Write-Action ("Deleted service '{0}'" -f $ServiceName) -Level Ok
    }
    else {
        Write-Action ("sc delete '{0}' failed (need Admin?)" -f $ServiceName) -Level Err
    }
}

function Remove-LegacyBinaries {
    param(
        [string[]]$Dirs,
        [bool]$PathMigrationOk
    )

    Write-Step 'Remove legacy easytier-gui.exe / .bak under install dirs'
    if ($Dirs.Count -eq 0) {
        Write-Action 'No install directories found' -Level Ok
        return
    }

    foreach ($dir in $Dirs) {
        Write-Host ("  scan: {0}" -f $dir) -ForegroundColor DarkGray
        $legacy = Join-Path $dir 'easytier-gui.exe'
        $bak = Join-Path $dir 'easytier-gui.exe.bak'
        $et = Join-Path $dir 'ET.exe'

        if (Test-Path -LiteralPath $bak) {
            Write-Action ("Delete {0}" -f $bak) -Level Warn
            if ($Apply) { Remove-Item -LiteralPath $bak -Force -ErrorAction SilentlyContinue }
        }

        if (-not (Test-Path -LiteralPath $legacy)) { continue }

        if (-not (Test-Path -LiteralPath $et)) {
            Write-Action ("Keep {0} (ET.exe missing in same dir — not a post-rename install)" -f $legacy) -Level Warn
            continue
        }

        if (-not $AlsoRemoveShim) {
            Write-Action ("Found shim {0} (pass -AlsoRemoveShim to delete when safe)" -f $legacy) -Level Dry
            continue
        }

        if (-not $PathMigrationOk) {
            Write-Action ("Skip deleting shim {0}: service PathName migration did not fully succeed" -f $legacy) -Level Err
            continue
        }

        Write-Action ("Delete legacy shim {0}" -f $legacy) -Level Warn
        if ($Apply) { Remove-Item -LiteralPath $legacy -Force -ErrorAction SilentlyContinue }
    }
}

function Repair-LegacyShortcuts {
    Write-Step 'Retarget shortcuts that still point at easytier-gui.exe'
    $roots = @(
        [Environment]::GetFolderPath('Desktop'),
        [Environment]::GetFolderPath('CommonDesktopDirectory'),
        (Join-Path ([Environment]::GetFolderPath('StartMenu')) 'Programs'),
        (Join-Path ([Environment]::GetFolderPath('CommonStartMenu')) 'Programs')
    ) | Where-Object { $_ -and (Test-Path $_) }

    $shell = New-Object -ComObject WScript.Shell
    foreach ($root in $roots) {
        Get-ChildItem -Path $root -Filter '*.lnk' -Recurse -ErrorAction SilentlyContinue | ForEach-Object {
            try {
                $lnk = $shell.CreateShortcut($_.FullName)
                if (-not $lnk.TargetPath -or ($lnk.TargetPath -notmatch '(?i)easytier-gui\.exe$')) {
                    return
                }
                $etBeside = Join-Path (Split-Path -Parent $lnk.TargetPath) 'ET.exe'
                if (Test-Path -LiteralPath $etBeside) {
                    Write-Action ("Retarget {0}`n    {1}`n -> {2}" -f $_.FullName, $lnk.TargetPath, $etBeside) -Level Warn
                    if ($Apply) {
                        $lnk.TargetPath = $etBeside
                        $lnk.Save()
                    }
                }
                else {
                    Write-Action ("Shortcut still legacy (ET.exe not beside target): {0} -> {1}" -f $_.FullName, $lnk.TargetPath) -Level Dry
                }
            }
            catch { }
        }
    }
}

# --- main ---
Write-Host 'EasyTier legacy GUI cleanup (Windows)' -ForegroundColor Cyan
Write-Host 'Targets leftovers from easytier-gui.exe -> ET.exe rename.'
if (-not $Apply) {
    Write-Host 'Dry-run only. Re-run with -Apply to make changes.' -ForegroundColor Yellow
}

Stop-LegacyGuiProcesses

Write-Step 'Migrate / inspect GUI Windows services'
$migOk = $true
$migOk = (Update-ServiceBinPathIfLegacy -ServiceName 'ET-Gui') -and $migOk
$migOk = (Update-ServiceBinPathIfLegacy -ServiceName 'easytier-gui') -and $migOk

Write-Step 'Remove legacy service name only (easytier-gui)'
Remove-LegacyService -ServiceName 'easytier-gui'

$dirs = @(Get-CandidateInstallDirs)
Remove-LegacyBinaries -Dirs $dirs -PathMigrationOk:$migOk
Repair-LegacyShortcuts

Write-Step 'Done'
if (-not $Apply) {
    Write-Host 'No changes were made. Example:' -ForegroundColor Yellow
    Write-Host '  .\script\cleanup-legacy-gui.ps1 -Apply'
    Write-Host '  .\script\cleanup-legacy-gui.ps1 -Apply -AlsoRemoveShim'
}
else {
    Write-Host 'Finished. If ET-Gui still fails to start, open GUI once and re-enable Service mode.' -ForegroundColor Green
}
