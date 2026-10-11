@echo off
setlocal
REM Cleanup leftovers from easytier-gui.exe -> ET.exe rename (Windows).
REM Default: dry-run. Pass -Apply (and optionally -AlsoRemoveShim).

cd /d "%~dp0\.."
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0cleanup-legacy-gui.ps1" %*
set ERR=%ERRORLEVEL%
echo.
pause
exit /b %ERR%
