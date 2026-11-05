#Requires -Version 5.1
<#
.SYNOPSIS
  Regression guard for the Windows NSIS PATH helper.

.DESCRIPTION
  prove-path-hook.ps1 only exercises PowerShell-level behaviour. This script
  compiles the REAL ET_WritePathHelperScript macro with makensis (Unicode true,
  same as Tauri's installer.nsi), runs a tiny silent installer so the helper
  .ps1 files are generated, and asserts both helpers are valid multi-line
  PowerShell that powershell.exe -File can execute (exit 0).

  Catches the "ET installed, but adding ... to PATH failed (code 1)" bug:
  `r`n written by FileWrite is literal text (NSIS needs $\r$\n), so the helper
  collapses into one unparsable line and exits 1.

  No PATH changes are made. The add helper is pointed at a directory already on
  Machine PATH (it returns early); the remove helper at a directory that is not
  on PATH at all (it removes nothing).

.PARAMETER Makensis
  Path to makensis.exe. Defaults to the NSIS bundled by Tauri.

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File verify-path-hook-nsis.ps1
#>
param(
    [string]$Makensis = (Join-Path $env:LOCALAPPDATA 'tauri\NSIS\makensis.exe'),
    [string]$Hooks    = (Join-Path $PSScriptRoot 'installer-hooks.nsh')
)
$ErrorActionPreference = 'Stop'

if (-not (Test-Path $Makensis)) { throw "makensis.exe not found: $Makensis" }
if (-not (Test-Path $Hooks))    { throw "hooks file not found: $Hooks" }

$hooks = Get-Content $Hooks -Raw
$macro = [regex]::Match($hooks, '(?ms)^!macro ET_WritePathHelperScript.*?^!macroend\s*$')
if (-not $macro.Success) { throw 'ET_WritePathHelperScript macro not found in hooks file' }

# A PATH entry that already exists => the add helper's idempotency check fires
# before any SetEnvironmentVariable call. Avoid spaces/quotes for the NSIS cmdline.
$already = @([Environment]::GetEnvironmentVariable('Path', 'Machine') -split ';' |
    Where-Object { $_ -and ($_ -notmatch '[\s"%]') }) | Select-Object -First 1
if (-not $already) { throw 'no usable Machine PATH entry for the idempotency probe' }
$fake = 'C:\ET-Path-Hook-Probe-Nonexistent'

$work = Join-Path ([System.IO.Path]::GetTempPath()) ('et-path-hook-' + [guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Path $work | Out-Null
try {
    $exe    = Join-Path $work 'probe.exe'
    $nsi    = Join-Path $work 'probe.nsi'
    $result = Join-Path $work 'result.txt'

    $template = @'
Unicode true
SilentInstall silent
OutFile "__EXE__"
__MACRO__
Section "probe"
  !insertmacro ET_WritePathHelperScript "$EXEDIR\add.ps1" "add" "add"
  !insertmacro ET_WritePathHelperScript "$EXEDIR\remove.ps1" "remove" "rm"
  nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$EXEDIR\add.ps1" -InstallDir "__ALREADY__"'
  Pop $1
  nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$EXEDIR\remove.ps1" -InstallDir "__FAKE__"'
  Pop $2
  FileOpen $R8 "__RESULT__" w
  FileWrite $R8 "add=$1 remove=$2$\r$\n"
  FileClose $R8
SectionEnd
'@
    $script = $template.Replace('__MACRO__', $macro.Value).
                        Replace('__EXE__', $exe).
                        Replace('__ALREADY__', $already).
                        Replace('__FAKE__', $fake).
                        Replace('__RESULT__', $result)
    [System.IO.File]::WriteAllText($nsi, $script, (New-Object System.Text.UTF8Encoding($false)))

    & $Makensis $nsi | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "makensis failed (exit $LASTEXITCODE)" }
    & $exe | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "probe installer failed (exit $LASTEXITCODE)" }

    $out = (Get-Content $result -Raw).Trim()
    if ($out -ne 'add=0 remove=0') { throw "helper exit codes unexpected: '$out' (want 'add=0 remove=0')" }

    foreach ($name in @('add.ps1', 'remove.ps1')) {
        $file = Join-Path $work $name
        $text = Get-Content $file -Raw
        $lines = @($text -split "`r?`n" | Where-Object { $_ })
        if ($lines.Count -lt 10) { throw "$name collapsed to $($lines.Count) lines" }
        if ($text -match '`r`n') { throw "$name contains literal ``r``n (bad NSIS escaping)" }
    }
    Write-Host "PASS: NSIS helpers parse and run (add=0 remove=0); Machine PATH untouched."
    exit 0
}
finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
