@echo off
setlocal EnableExtensions
REM Debug: pnpm tauri dev (copies third_party DLLs; no installer / zip)

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
echo   EasyTier Windows - DEBUG
echo   pnpm tauri dev (no package)
echo ========================================
echo.

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-easytier-windows.ps1" -Dev
set "ERR=%ERRORLEVEL%"

echo.
if not "%ERR%"=="0" (
  echo Debug launch failed, exit code %ERR%.
  pause
  exit /b %ERR%
)

echo Launcher finished. GUI window stays open until you close it.
pause
exit /b 0
