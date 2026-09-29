@echo off
setlocal EnableExtensions
REM Thin launcher: interactive menu lives in PowerShell (avoids CMD UTF-8 flash-exit).

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

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-easytier-web.ps1" -Interactive
set "ERR=%ERRORLEVEL%"

echo.
if not "%ERR%"=="0" (
  echo Build failed, exit code %ERR%.
  pause
  exit /b %ERR%
)

echo Build finished.
pause
exit /b 0
