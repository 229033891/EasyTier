<#
.SYNOPSIS
    Local ET Test preflight (mirrors .github/workflows/test.yml as far as the host allows).

.DESCRIPTION
    Aligns with script/easytier-windows-*.cmd style:
      Fast  = fmt + Cargo.lock + clippy + cargo-hack features  (pre-push default)
      Full  = Fast + WASI check (if target installed) + nextest (non-three_node)

    three_node / subnet_proxy tests need Linux (tun, bridge, sudo). On Windows they
    are skipped unless you pass -IncludeThreeNode (then still fail with a clear error).
    On Linux/WSL, Full includes them unless -SkipThreeNode.

    Logs: artifacts\logs\easytier-test-*.log

.PARAMETER Profile
    Fast | Full (default Fast)

.PARAMETER SkipFmt
.PARAMETER SkipLockfile
.PARAMETER SkipClippy
.PARAMETER SkipFeatures
.PARAMETER SkipWasi
.PARAMETER SkipTests
.PARAMETER SkipThreeNode
    Skip three_node matrix (default on Windows; optional on Linux).

.PARAMETER IncludeThreeNode
    Attempt three_node on this host (Linux/WSL only in practice).

.PARAMETER InstallTools
    Install missing cargo-hack / cargo-nextest via `cargo install`.

.PARAMETER Interactive
    Prompt for profile / skips.

.EXAMPLE
    .\script\test-easytier.ps1
    .\script\test-easytier.ps1 -Profile Full
    .\script\test-easytier.ps1 -Profile Fast -SkipFeatures
    .\script\test-easytier.ps1 -Interactive
#>
param(
    [ValidateSet('Fast', 'Full')]
    [string]$Profile = 'Fast',

    [switch]$SkipFmt,
    [switch]$SkipLockfile,
    [switch]$SkipClippy,
    [switch]$SkipFeatures,
    [switch]$SkipWasi,
    [switch]$SkipTests,
    [switch]$SkipThreeNode,
    [switch]$IncludeThreeNode,

    [switch]$InstallTools,
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
    Write-Host '  EasyTier ET Test (local)'
    Write-Host '========================================'
    Write-Host ''
    Write-Host '  Fast = fmt + lockfile + clippy + features'
    Write-Host '  Full = Fast + WASI (if installed) + nextest'
    Write-Host '         (+ three_node on Linux unless skipped)'
    Write-Host ''
    $Profile = Read-MenuChoice 'Profile [Fast/Full] (Enter=Fast)' @('Fast', 'Full') 'Fast'
    $confirm = Read-Host 'Start? [Y/n] (Enter=Y)'
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
Start-EasytierBuildTranscript -Name 'easytier-test'

$script:FailedSteps = [System.Collections.Generic.List[string]]::new()
$script:SkippedSteps = [System.Collections.Generic.List[string]]::new()

try {

function Test-IsWindowsHost {
    return ($env:OS -match 'Windows') -or ($PSVersionTable.Platform -eq 'Win32NT')
}

function Ensure-CargoTool {
    param(
        [Parameter(Mandatory)][string]$BinName,
        [Parameter(Mandatory)][string]$CrateName,
        [Parameter(Mandatory)][string]$CargoSubcommand
    )
    if (Get-Command $BinName -ErrorAction SilentlyContinue) {
        return $true
    }
    # cargo-<tool> exposes `cargo <subcommand>`; probe without failing the script.
    $probe = Invoke-NativeLogged -FilePath 'cargo' -ArgumentList @($CargoSubcommand, '--version')
    if ($probe -eq 0) { return $true }

    if (-not $InstallTools) {
        Write-Log "Missing tool: $BinName (crate $CrateName). Re-run with -InstallTools, or: cargo install $CrateName" -Level Warn
        return $false
    }
    Write-Step "cargo install $CrateName"
    $code = Invoke-NativeLogged -FilePath 'cargo' -ArgumentList @('install', $CrateName, '--locked')
    if ($code -ne 0) {
        throw "cargo install $CrateName failed (exit $code)"
    }
    return $true
}

function Invoke-EtStep {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][scriptblock]$Action,
        [switch]$SoftFail
    )
    Write-Step $Name
    try {
        & $Action
        Write-Log "OK: $Name" -Level Ok
    }
    catch {
        $msg = "$_"
        Write-Log "FAIL: $Name — $msg" -Level Err
        $script:FailedSteps.Add($Name) | Out-Null
        if (-not $SoftFail) { throw }
    }
}

function Assert-NativeOk {
    param(
        [Parameter(Mandatory)][string]$FilePath,
        [Parameter()][string[]]$ArgumentList = @(),
        [Parameter(Mandatory)][string]$FailMessage
    )
    $code = Invoke-NativeLogged -FilePath $FilePath -ArgumentList $ArgumentList
    if ($code -ne 0) {
        throw "$FailMessage (exit $code)"
    }
}

$isWindows = Test-IsWindowsHost
$runThreeNode = $false
if ($IncludeThreeNode) {
    $runThreeNode = -not $SkipThreeNode
}
elseif ($Profile -eq 'Full' -and -not $isWindows -and -not $SkipThreeNode) {
    $runThreeNode = $true
}

Write-Log 'EasyTier ET Test (local)'
Write-Log "  Repo:     $RepoRoot"
Write-Log "  Profile:  $Profile"
Write-Log "  Host:     $(if ($isWindows) { 'Windows' } else { 'Unix/Linux' })"
Write-Log "  three_node: $runThreeNode"
Write-Log '  Mirrors:  .github/workflows/test.yml (check-* + nextest matrix)'

Assert-Command cargo

if ($isWindows) {
    Ensure-WindowsBuildPrereqs
    Set-EasytierWritableTemp
    Initialize-EasytierWindowsCargoEnv
}

function Get-EasytierClippyArgs {
    # CI (Linux): cargo clippy --all-targets --features full --all
    # Windows: lib + bins only (no --tests). Integration/unit test modules include
    # unix-only helpers (dead_code on Windows) and are covered by CI anyway.
    $args = @('clippy')
    if ($isWindows) {
        $args += @(
            '-p', 'easytier',
            '-p', 'easytier-core',
            '-p', 'easytier-web',
            '-p', 'easytier-proto'
        )
    }
    else {
        $args += @('--all-targets', '--features', 'full', '--all')
    }
    $args += @('--', '-D', 'warnings')
    return $args
}

function Get-EasytierHackExcludeFeatures {
    # openssl-crypto needs vendored OpenSSL + full Perl on Windows (Git perl is not enough).
    if ($isWindows) { return 'macos-ne,openssl-crypto' }
    return 'macos-ne'
}

function Write-EasytierWindowsNativeBuildHint {
    Write-Log 'Native build failed (not a Rust lint): use Developer shell (cl.exe); D8050 -> CARGO_TARGET_DIR already shortened. For --features full / openssl-crypto locally install Strawberry Perl — or rely on CI Linux. See docs/ops/windows-msvc-local-build.md' -Level Warn
}

# --- fmt -------------------------------------------------------------------
if (-not $SkipFmt) {
    Invoke-EtStep 'cargo fmt --all -- --check' {
        Assert-NativeOk cargo @('fmt', '--all', '--', '--check') 'fmt check failed'
    }
}
else {
    $script:SkippedSteps.Add('fmt') | Out-Null
}

# --- lockfile --------------------------------------------------------------
if (-not $SkipLockfile) {
    Invoke-EtStep 'cargo metadata --locked' {
        Assert-NativeOk cargo @('metadata', '--format-version', '1', '--locked') `
            'Cargo.lock out of date (run cargo generate-lockfile / cargo build, then commit Cargo.lock)'
    }
}
else {
    $script:SkippedSteps.Add('lockfile') | Out-Null
}

# --- clippy ----------------------------------------------------------------
if (-not $SkipClippy) {
    $clippyLabel = if ($isWindows) {
        'cargo clippy -p easytier -p easytier-core -p easytier-web -p easytier-proto (lib+bins, no tests)'
    } else {
        'cargo clippy --all-targets --features full --all'
    }
    Invoke-EtStep $clippyLabel {
        try {
            Assert-NativeOk cargo (Get-EasytierClippyArgs) 'clippy failed'
        }
        catch {
            if ($isWindows) { Write-EasytierWindowsNativeBuildHint }
            throw
        }
    }
    if ($isWindows) {
        # CI check-hack uses `cargo check`, not clippy -D warnings: no-default-features
        # on Windows leaves platform modules unused (dead_code noise under clippy).
        Invoke-EtStep 'cargo check -p easytier --no-default-features (CI check-hack path)' {
            try {
                Assert-NativeOk cargo @(
                    'check', '-p', 'easytier', '--no-default-features'
                ) 'cargo check --no-default-features failed'
            }
            catch {
                Write-EasytierWindowsNativeBuildHint
                throw
            }
        }
    }
}
else {
    $script:SkippedSteps.Add('clippy') | Out-Null
}

# --- features (cargo-hack) -------------------------------------------------
if (-not $SkipFeatures) {
    Invoke-EtStep 'cargo hack check --package easytier --each-feature' {
        if (-not (Ensure-CargoTool -BinName 'cargo-hack' -CrateName 'cargo-hack' -CargoSubcommand 'hack')) {
            throw 'cargo-hack not available'
        }
        try {
            $hackExclude = Get-EasytierHackExcludeFeatures
            Assert-NativeOk cargo @(
                'hack', 'check',
                '--package', 'easytier',
                '--each-feature',
                '--exclude-features', $hackExclude
            ) "cargo hack / features check failed (exclude: $hackExclude)"
        }
        catch {
            if ($isWindows) { Write-EasytierWindowsNativeBuildHint }
            throw
        }
    }
}
else {
    $script:SkippedSteps.Add('features') | Out-Null
}

# --- WASI (Full only) ------------------------------------------------------
if ($Profile -eq 'Full' -and -not $SkipWasi) {
    $wasiInstalled = $false
    try {
        $targets = & rustup target list --installed 2>$null
        if ($targets -match 'wasm32-wasip1') { $wasiInstalled = $true }
    }
    catch { }

    if (-not $wasiInstalled) {
        Write-Log 'Skip WASI: rustup target wasm32-wasip1 not installed (rustup target add wasm32-wasip1)' -Level Warn
        $script:SkippedSteps.Add('wasi') | Out-Null
    }
    else {
        Invoke-EtStep 'cargo check easytier-core (wasm32-wasip1)' {
            Assert-NativeOk cargo @(
                'check', '--package', 'easytier-core', '--lib',
                '--target', 'wasm32-wasip1',
                '--features', 'management-rpc,proxy-smoltcp-stack,ring-crypto,wasi-crypto-offload'
            ) 'WASI check failed'
        }
    }
}
elseif ($Profile -eq 'Full') {
    $script:SkippedSteps.Add('wasi') | Out-Null
}

# --- nextest (Full only) ---------------------------------------------------
if ($Profile -eq 'Full' -and -not $SkipTests) {
    Invoke-EtStep 'cargo nextest (not three_node)' {
        if (-not (Ensure-CargoTool -BinName 'cargo-nextest' -CrateName 'cargo-nextest' -CargoSubcommand 'nextest')) {
            throw 'cargo-nextest not available'
        }
        try {
            $nextestArgs = @(
                'nextest', 'run',
                '--package', 'easytier',
                '--package', 'easytier-core',
                '-E', 'not test(tests::three_node)',
                '--test-threads', '1',
                '--no-fail-fast'
            )
            if (-not $isWindows) {
                $nextestArgs += @('--features', 'full')
            }
            Assert-NativeOk cargo $nextestArgs 'nextest (unit/integration, no three_node) failed'
        }
        catch {
            if ($isWindows) { Write-EasytierWindowsNativeBuildHint }
            throw
        }
    }

    if ($runThreeNode) {
        if ($isWindows) {
            Write-Log 'three_node requires Linux (tun/bridge/sudo). Use WSL or script/test-easytier.sh --three-node.' -Level Warn
            $script:SkippedSteps.Add('three_node') | Out-Null
        }
        else {
            Invoke-EtStep 'cargo nextest (three_node, no subnet_proxy)' {
                Assert-NativeOk cargo @(
                    'nextest', 'run',
                    '--package', 'easytier',
                    '--package', 'easytier-core',
                    '--features', 'full',
                    '-E', 'test(tests::three_node) and not test(subnet_proxy_three_node_test)',
                    '--test-threads', '1',
                    '--no-fail-fast'
                ) 'nextest three_node failed'
            }
            Invoke-EtStep 'cargo nextest (subnet_proxy_three_node_test)' {
                Assert-NativeOk cargo @(
                    'nextest', 'run',
                    '--package', 'easytier',
                    '--package', 'easytier-core',
                    '--features', 'full',
                    '-E', 'test(subnet_proxy_three_node_test)',
                    '--test-threads', '1',
                    '--no-fail-fast'
                ) 'nextest subnet_proxy_three_node_test failed'
            }
        }
    }
    else {
        $script:SkippedSteps.Add('three_node') | Out-Null
        if ($isWindows -and $Profile -eq 'Full') {
            Write-Log 'Note: three_node skipped on Windows (CI runs it on ubuntu-latest).' -Level Info
        }
    }
}
elseif ($Profile -eq 'Full') {
    $script:SkippedSteps.Add('tests') | Out-Null
}

Write-Host ''
if ($script:FailedSteps.Count -gt 0) {
    Write-Log ('Failed steps: ' + ($script:FailedSteps -join ', ')) -Level Err
    throw 'ET Test local preflight failed.'
}

Write-Log 'ET Test local preflight OK' -Level Ok
if ($script:SkippedSteps.Count -gt 0) {
    Write-Log ('Skipped: ' + ($script:SkippedSteps -join ', ')) -Level Info
}
if ($Profile -eq 'Fast') {
    Write-Log 'Tip: .\script\test-easytier.ps1 -Profile Full  for nextest (+ WASI / Linux three_node)'
}
Write-EasytierBuildElapsed

} finally {
    Stop-EasytierBuildTranscript
}
