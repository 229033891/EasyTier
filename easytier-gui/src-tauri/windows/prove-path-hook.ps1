#Requires -Version 5.1
<#
.SYNOPSIS
  Reproduce why the 2.7.10 NSIS PATH hook could install successfully yet leave
  `ET-cli` off PATH — and show that `-File -InstallDir` fixes it.

.DESCRIPTION
  Experiments (User PATH only; restores afterwards):

  A  Empty `$env:ET_INSTDIR_FOR_PATH` + old early-exit body
     → exit 0, Path unchanged  (silent success)

  B  Env set + Start-Process ArgumentList -Command
     → Path updated             (logic itself is fine)

  C  Env set + `cmd /c "powershell … -Command \"$d=…\""`
     → command line truncates at `$d`, Path unchanged
       (same nested-quote failure mode as nsExec single cmdline)

  D  New approach: `powershell -File add.ps1 -InstallDir <dir>`
     → Path updated, no env dependency

  Run:
    powershell -NoProfile -ExecutionPolicy Bypass -File prove-path-hook.ps1

  Expect:
    A path_changed=False exit=0
    B path_changed=True
    C path_changed=False (and often a cmd parse error on stderr)
    D path_changed=True exit=0
#>
$ErrorActionPreference = 'Continue'
$probe = Join-Path $env:TEMP ('et-path-prove-' + [guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Path $probe | Out-Null
$fake = Join-Path $probe 'ET-FakeInstall'
New-Item -ItemType Directory -Path $fake | Out-Null

function Test-HasDir([string]$scope, [string]$dir) {
    $p = [Environment]::GetEnvironmentVariable('Path', $scope)
    if ($null -eq $p) { return $false }
    return [bool](@($p -split ';') | Where-Object { $_ -and ($_.TrimEnd('\') -ieq $dir.TrimEnd('\')) })
}

$userBefore = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($null -eq $userBefore) { $userBefore = '' }

function Restore-UserPath {
    [Environment]::SetEnvironmentVariable('Path', $userBefore, 'User')
}

$oldBody = @'
$d = $env:ET_INSTDIR_FOR_PATH; if (-not $d) { exit 0 }; $d = $d.TrimEnd([char]92); $p = [Environment]::GetEnvironmentVariable('Path','User'); if ($null -eq $p) { $p = '' }; if (@($p -split [char]59) | Where-Object { $_ -and ($_.TrimEnd([char]92) -ieq $d) } | Select-Object -First 1) { exit 0 }; $parts = @(@($p -split [char]59) | Where-Object { $_ }); $parts += $d; [Environment]::SetEnvironmentVariable('Path', ($parts -join [char]59), 'User'); exit 0
'@

$results = @()
try {
    # A — empty env → silent no-op
    Restore-UserPath
    Remove-Item Env:ET_INSTDIR_FOR_PATH -ErrorAction SilentlyContinue
    $p = Start-Process -FilePath "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" `
        -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-Command', $oldBody) `
        -Wait -PassThru -NoNewWindow
    $results += [pscustomobject]@{
        Exp           = 'A_empty_env'
        ExitCode      = $p.ExitCode
        PathChanged   = (Test-HasDir 'User' $fake)
        Expect        = 'exit0 path_changed=False'
    }

    # B — env set, ArgumentList -Command (no cmd nesting)
    Restore-UserPath
    $env:ET_INSTDIR_FOR_PATH = $fake
    $p = Start-Process -FilePath "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" `
        -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-Command', $oldBody) `
        -Wait -PassThru -NoNewWindow
    $results += [pscustomobject]@{
        Exp           = 'B_ArgumentList_Command'
        ExitCode      = $p.ExitCode
        PathChanged   = (Test-HasDir 'User' $fake)
        Expect        = 'path_changed=True'
    }

    # C — nsExec-like: one cmdline through cmd /c (nested quotes)
    Restore-UserPath
    $env:ET_INSTDIR_FOR_PATH = $fake
    $cmdLine = "`"$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe`" -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command `"$oldBody`""
    $p = Start-Process -FilePath "$env:SystemRoot\System32\cmd.exe" `
        -ArgumentList @('/c', $cmdLine) `
        -Wait -PassThru -NoNewWindow
    $results += [pscustomobject]@{
        Exp           = 'C_cmd_nested_Command'
        ExitCode      = $p.ExitCode
        PathChanged   = (Test-HasDir 'User' $fake)
        Expect        = 'path_changed=False (quote truncate)'
    }

    # D — new installer approach
    Restore-UserPath
    Remove-Item Env:ET_INSTDIR_FOR_PATH -ErrorAction SilentlyContinue
    $ps1 = Join-Path $probe 'add-path.ps1'
    @'
param([Parameter(Mandatory=$true)][string]$InstallDir)
$ErrorActionPreference = 'Stop'
$d = $InstallDir.TrimEnd([char]92)
if (-not $d) { exit 1 }
$p = [Environment]::GetEnvironmentVariable('Path','User')
if ($null -eq $p) { $p = '' }
if (-not (@($p -split ';') | Where-Object { $_ -and ($_.TrimEnd('\') -ieq $d) } | Select-Object -First 1)) {
  $parts = @(@($p -split ';') | Where-Object { $_ })
  $parts += $d
  [Environment]::SetEnvironmentVariable('Path', ($parts -join ';'), 'User')
}
$p2 = [Environment]::GetEnvironmentVariable('Path','User')
if (@($p2 -split ';') | Where-Object { $_ -and ($_.TrimEnd('\') -ieq $d) } | Select-Object -First 1) { exit 0 } else { exit 2 }
'@ | Set-Content -Path $ps1 -Encoding UTF8
    $p = Start-Process -FilePath "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" `
        -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $ps1, '-InstallDir', $fake) `
        -Wait -PassThru -NoNewWindow
    $results += [pscustomobject]@{
        Exp           = 'D_File_InstallDir'
        ExitCode      = $p.ExitCode
        PathChanged   = (Test-HasDir 'User' $fake)
        Expect        = 'exit0 path_changed=True'
    }
}
finally {
    Restore-UserPath
    Remove-Item Env:ET_INSTDIR_FOR_PATH -ErrorAction SilentlyContinue
}

$results | Format-Table -AutoSize | Out-String | Write-Host
$fail = $false
if (-not ($results[0].ExitCode -eq 0 -and $results[0].PathChanged -eq $false)) { $fail = $true; Write-Host 'FAIL A' }
if (-not ($results[1].PathChanged -eq $true)) { $fail = $true; Write-Host 'FAIL B' }
if (-not ($results[2].PathChanged -eq $false)) { $fail = $true; Write-Host 'FAIL C' }
if (-not ($results[3].ExitCode -eq 0 -and $results[3].PathChanged -eq $true)) { $fail = $true; Write-Host 'FAIL D' }

$out = Join-Path $probe 'results.txt'
$results | Format-Table -AutoSize | Out-String | Set-Content $out
Write-Host "Results: $out"
if ($fail) { exit 1 } else { Write-Host 'ALL CHECKS PASSED'; exit 0 }
