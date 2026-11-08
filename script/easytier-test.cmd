@echo off
setlocal EnableExtensions
REM ============================================================================
REM  EasyTier — local ET Test launcher
REM  Mirrors GitHub Actions workflow: .github/workflows/test.yml ("ET Test")
REM ============================================================================
REM  Usage:
REM    easytier-test.cmd              interactive menu
REM    easytier-test.cmd fast         fmt + lockfile + clippy + cargo-hack
REM    easytier-test.cmd full         Fast + WASI (if installed) + nextest
REM    easytier-test.cmd info         print what CI / local cover (no build)
REM ============================================================================

cd /d "%~dp0.."
if not exist "%~dp0test-easytier.ps1" goto :missing_ps1
where powershell >nul 2>&1
if errorlevel 1 goto :missing_ps

set "MODE=%~1"
if /I "%MODE%"=="fast" goto :run_fast
if /I "%MODE%"=="full" goto :run_full
if /I "%MODE%"=="info" goto :show_info
if /I "%MODE%"=="/?" goto :show_info
if /I "%MODE%"=="-h" goto :show_info
if /I "%MODE%"=="--help" goto :show_info
if not "%MODE%"=="" (
  echo Unknown mode: %MODE%
  echo.
  goto :show_info
)

echo.
echo ========================================
echo   EasyTier ET Test ^(local^)
echo ========================================
echo.
echo   GitHub "ET Test" jobs ^(test.yml^):
echo     1. check-fmt      cargo fmt --check + Cargo.lock --locked
echo     2. check-clippy   clippy --all-targets --features full --all -D warnings
echo     3. check-hack     cargo hack --each-feature ^(easytier^)
echo     4. check-wasi     easytier-core wasm32-wasip1 subset
echo     5. bench-routing  hot_path_alloc + routing_lookup + threshold gate
echo     6. test_matrix    nextest: easytier / three_node / subnet_proxy
echo.
echo   Local profiles ^(this machine^):
echo     [1] Fast  — fmt + lock + clippy + features   ^(daily push gate^)
echo     [2] Full  — Fast + WASI + nextest             ^(three_node needs Linux/WSL^)
echo     [3] Info  — print CI vs local coverage only
echo     [Q] Quit
echo.
choice /C 123Q /N /M "Select [1/2/3/Q]: "
if errorlevel 4 goto :eof
if errorlevel 3 goto :show_info
if errorlevel 2 goto :run_full
if errorlevel 1 goto :run_fast
goto :eof

:run_fast
echo.
echo --- ET Test Fast ---
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0test-easytier.ps1" -Profile Fast -InstallTools
set "ERR=%ERRORLEVEL%"
goto :done

:run_full
echo.
echo --- ET Test Full ---
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0test-easytier.ps1" -Profile Full -InstallTools
set "ERR=%ERRORLEVEL%"
goto :done

:show_info
echo.
echo ========== GitHub ET Test ^(test.yml^) ==========
echo.
echo Trigger: PR always; push only on releases/**; workflow_dispatch.
echo Skip:    docs-only / duplicate successful runs ^(path filter^).
echo.
echo Jobs:
echo   check-fmt       cargo fmt --all -- --check
echo                   cargo metadata --locked
echo   check-clippy    cargo clippy --all-targets --features full --all -- -D warnings
echo   check-hack      cargo hack check -p easytier --each-feature --exclude-features macos-ne
echo                   ^(Windows local also excludes openssl-crypto,jemalloc,jemalloc-prof^)
echo   check-wasi      cargo check -p easytier-core --lib --target wasm32-wasip1
echo                   --features management-rpc,proxy-smoltcp-stack,ring-crypto,wasi-crypto-offload
echo   bench-routing   cargo test -p easytier-core --features test-utils --test hot_path_alloc
echo                   cargo bench -p easytier-benches --bench routing_lookup -- --noplot
echo                   python3 script/check_routing_bench.py
echo   test_matrix     nextest archive ^(easytier + easytier-core, --features full^) then:
echo                     - easytier ^(exclude tests::three_node^)
echo                     - three_node ^(exclude subnet_proxy_three_node_test^)
echo                     - subnet_proxy_three_node_test
echo                   Needs Linux: tun/bridge/iptables/sudo + EASYTIER_LINUX_BPF_INTEGRATION
echo.
echo ========== Local scripts ==========
echo.
echo   script\easytier-test.cmd          this menu / fast / full / info
echo   script\easytier-test-fast.cmd     Fast only
echo   script\easytier-test-full.cmd     Full only
echo   script\test-easytier.ps1          PowerShell driver ^(-Profile Fast^|Full^)
echo   script\test-easytier.sh           Linux/WSL ^(supports --three-node^)
echo.
echo Windows vs CI ^(intentional^):
echo   - clippy: lib+bins on key crates; NO --features full / --all-targets
echo   - nextest: default features; three_node SKIPPED ^(use WSL^)
echo   - bench-routing: NOT run locally by these scripts ^(CI-only gate^)
echo   - MSVC: scripts auto-load vcvars64 ^(no permanent PATH hack^)
echo   Logs:  artifacts\logs\easytier-test-*.log
echo.
if "%MODE%"=="" pause
exit /b 0

:done
echo.
if not "%ERR%"=="0" (
  echo ET Test failed, exit code %ERR%.
  echo See latest log under artifacts\logs\
  pause
  exit /b %ERR%
)
echo ET Test finished OK.
echo Logs: artifacts\logs\easytier-test-*.log
pause
exit /b 0

:missing_ps1
echo ERROR: missing "%~dp0test-easytier.ps1"
pause
exit /b 1

:missing_ps
echo ERROR: powershell not found in PATH
pause
exit /b 1
