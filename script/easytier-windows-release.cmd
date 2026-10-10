@echo off
setlocal EnableExtensions
REM Package: formal --release GUI NSIS + headless ET-windows zip (production)

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
echo   EasyTier Windows - RELEASE package
echo   GUI NSIS + headless (--release)
echo ========================================
echo.

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-easytier-windows.ps1" -Profile Release
set "ERR=%ERRORLEVEL%"

echo.
if not "%ERR%"=="0" (
  echo Release package failed, exit code %ERR%.
  echo See latest log under artifacts\logs\
  pause
  exit /b %ERR%
)

echo Release package finished.
echo Output: artifacts\  (NSIS installer + ET-windows-*\ + .zip)
echo Logs:   artifacts\logs\easytier-windows-*.log
pause
exit /b 0
