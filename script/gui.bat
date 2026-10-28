@echo off
setlocal EnableExtensions
chcp 65001 >nul
echo === Legacy service name: easytier-gui ===
sc query easytier-gui >nul 2>&1
if errorlevel 1 (
  echo [OK] No legacy service name easytier-gui
) else (
  echo [WARN] Legacy service name easytier-gui still exists
  sc qc easytier-gui
)

echo.
echo === Current service: ET-Gui PathName ===
sc query ET-Gui >nul 2>&1
if errorlevel 1 (
  echo [INFO] ET-Gui not installed
) else (
  sc qc ET-Gui | findstr /I "SERVICE_NAME DISPLAY_NAME BINARY_PATH START_TYPE"
  sc qc ET-Gui | findstr /I "easytier-gui.exe" >nul
  if errorlevel 1 (
    echo [OK] ET-Gui PathName does not reference easytier-gui.exe
  ) else (
    echo [WARN] ET-Gui PathName still references easytier-gui.exe
  )
)

echo.
echo === Legacy shim file ===
if exist "%ProgramFiles%\ET\easytier-gui.exe" (
  echo [WARN] Legacy shim still present: %ProgramFiles%\ET\easytier-gui.exe
) else (
  echo [OK] No %ProgramFiles%\ET\easytier-gui.exe
)

echo.
pause