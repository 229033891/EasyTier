@echo off
setlocal EnableExtensions
REM Debug: cargo run API + Vite frontend (no package exe)

cd /d "%~dp0.."
if not exist "%~dp0build-easytier-web.ps1" (
  echo ERROR: missing "%~dp0build-easytier-web.ps1"
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
echo   EasyTier Web - DEBUG
echo   cargo run API + Vite (no exe)
echo ========================================
echo.

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-easytier-web.ps1" -Dev
set "ERR=%ERRORLEVEL%"

echo.
if not "%ERR%"=="0" (
  echo Debug launch failed, exit code %ERR%.
  pause
  exit /b %ERR%
)

echo Launcher finished. Backend/frontend windows stay open until you close them.
pause
exit /b 0
