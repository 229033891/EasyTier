@echo off
setlocal EnableExtensions
REM Package: formal --release + embed frontend (production)

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
echo   EasyTier Web - RELEASE package
echo   cargo --release + embed (production)
echo ========================================
echo.

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-easytier-web.ps1" -Profile Release
set "ERR=%ERRORLEVEL%"

echo.
if not "%ERR%"=="0" (
  echo Release package failed, exit code %ERR%.
  pause
  exit /b %ERR%
)

echo Release package finished.
echo Binary: target\release\easytier-web-embed.exe
pause
exit /b 0
