@echo off
setlocal EnableExtensions
REM Local ET Test Full: Fast + WASI + nextest. three_node needs Linux or WSL.

cd /d "%~dp0.."
if not exist "%~dp0test-easytier.ps1" goto :missing_ps1

where powershell >nul 2>&1
if errorlevel 1 goto :missing_ps

echo.
echo ========================================
echo   EasyTier ET Test - FULL
echo   Fast + WASI + nextest (default features on Windows)
echo   three_node skipped on Windows
echo ========================================
echo.

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0test-easytier.ps1" -Profile Full -InstallTools
set "ERR=%ERRORLEVEL%"

echo.
if not "%ERR%"=="0" goto :fail

echo ET Test Full finished.
echo Logs: artifacts\logs\easytier-test-*.log
echo Note: three_node needs Linux/WSL - script/test-easytier.sh --full --three-node
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

:fail
echo ET Test Full failed, exit code %ERR%.
echo See latest log under artifacts\logs\
pause
exit /b %ERR%
