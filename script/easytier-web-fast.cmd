@echo off
setlocal EnableExtensions
REM Package: release-fast + embed frontend (daily / local)

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
echo   EasyTier Web - FAST package
echo   cargo --profile release-fast + embed
echo ========================================
echo.

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-easytier-web.ps1" -Profile Fast
set "ERR=%ERRORLEVEL%"

echo.
if not "%ERR%"=="0" (
  echo Fast package failed, exit code %ERR%.
  echo See latest log under artifacts\logs\
  pause
  exit /b %ERR%
)

echo Fast package finished.
echo Binary: target\release-fast\easytier-web-embed.exe
echo Logs:   artifacts\logs\easytier-web-*.log
pause
exit /b 0
