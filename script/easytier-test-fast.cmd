@echo off
setlocal EnableExtensions
REM Local ET Test Fast: fmt + lockfile + clippy + cargo-hack features. Pre-push.

cd /d "%~dp0.."
if not exist "%~dp0test-easytier.ps1" goto :missing_ps1

where powershell >nul 2>&1
if errorlevel 1 goto :missing_ps

echo.
echo ========================================
echo   EasyTier ET Test - FAST
echo   fmt + lockfile + clippy + features
echo ========================================
echo.

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0test-easytier.ps1" -Profile Fast -InstallTools
set "ERR=%ERRORLEVEL%"

echo.
if not "%ERR%"=="0" goto :fail

echo ET Test Fast finished.
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

:fail
echo ET Test Fast failed, exit code %ERR%.
echo See latest log under artifacts\logs\
pause
exit /b %ERR%
