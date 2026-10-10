@echo off
setlocal EnableExtensions
REM Package: release-fast GUI NSIS + headless ET-windows zip (daily / local)

cd /d "%~dp0.."
if not exist "%~dp0build-easytier-windows.ps1" (
  echo ERROR: missing "%~dp0build-easytier-windows.ps1"
  pause
  exit /b 1
)

where powershell >nul 2>&1
if errorlevel 1 (
  echo ERROR: powershell not found in PATH
  pause
  exit /b 1
)

echo.
echo ========================================
echo   EasyTier Windows - FAST package
echo   GUI NSIS (--release) + headless (release-fast)
echo ========================================
echo.

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-easytier-windows.ps1" -Profile Fast
set "ERR=%ERRORLEVEL%"

echo.
if not "%ERR%"=="0" (
  echo Fast package failed, exit code %ERR%.
  echo See latest log under artifacts\logs\
  pause
  exit /b %ERR%
)

echo Fast package finished.
echo Output: artifacts\  (NSIS installer + ET-windows-*\ + .zip)
echo Logs:   artifacts\logs\easytier-windows-*.log
pause
exit /b 0
