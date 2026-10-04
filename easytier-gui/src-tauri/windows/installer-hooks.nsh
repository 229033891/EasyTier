; Tauri NSIS hooks for ET GUI.
;
; Closing the main window only hides to tray (prevent_close), so
; easytier-gui.exe often stays running and locks the install path:
;   Error opening file for writing: ...\ET\easytier-gui.exe
;
; Service mode registers Windows service "ET-Gui" with restart-on-failure.
; A plain taskkill can race the SCM restart and leave the exe locked again
; before Tauri copies files. Disable auto-start, stop the service, wait for
; it to leave RUNNING, then force-kill with retries until tasklist is clear.
;
; taskkill / sc return non-zero when nothing matches — ignore that.
;
; NOTE: NSIS_HOOK_PREINSTALL runs before Tauri's CheckIfAppIsRunning.

!macro ET_StopGuiService
  DetailPrint "Stopping ET Gui Windows service if present..."
  ; Prevent SCM from restarting the GUI while we replace the binary.
  ; Current service name (sc): ET-Gui  | display name: ET Gui Service
  nsExec::ExecToLog 'sc.exe config ET-Gui start= demand'
  Pop $0
  nsExec::ExecToLog 'sc.exe stop ET-Gui'
  Pop $0
  ; Pre-rename leftover (sc): easytier-gui  | display name: EasyTier Gui Service
  nsExec::ExecToLog 'sc.exe config easytier-gui start= demand'
  Pop $0
  nsExec::ExecToLog 'sc.exe stop easytier-gui'
  Pop $0
  ; Wait until services leave RUNNING (sc query; ignore missing service).
  nsExec::ExecToLog 'cmd /c "for /L %i in (1,1,15) do @(sc query ET-Gui 2>nul | findstr /I \"RUNNING\" >nul && (ping -n 2 127.0.0.1 >nul) || exit /b 0)"'
  Pop $0
  nsExec::ExecToLog 'cmd /c "for /L %i in (1,1,15) do @(sc query easytier-gui 2>nul | findstr /I \"RUNNING\" >nul && (ping -n 2 127.0.0.1 >nul) || exit /b 0)"'
  Pop $0
  Sleep 500
!macroend

!macro ET_KillGuiProcesses
  DetailPrint "Stopping running ET GUI processes..."
  ; Primary binary name (crate / installed exe)
  nsExec::ExecToLog 'taskkill /F /T /IM easytier-gui.exe'
  Pop $0
  ; If a future build renames the main binary
  nsExec::ExecToLog 'taskkill /F /T /IM ET.exe'
  Pop $0
  Sleep 400
!macroend

!macro ET_WaitUntilGuiGone
  ; Retry kill until tasklist shows no easytier-gui.exe (or attempts exhausted).
  DetailPrint "Waiting for easytier-gui.exe to release install files..."
  nsExec::ExecToLog 'cmd /c "for /L %i in (1,1,20) do @(tasklist /FI \"IMAGENAME eq easytier-gui.exe\" | find /I \"easytier-gui.exe\" >nul && (taskkill /F /T /IM easytier-gui.exe >nul 2>&1 & ping -n 2 127.0.0.1 >nul) || exit /b 0)"'
  Pop $0
  nsExec::ExecToLog 'cmd /c "for /L %i in (1,1,10) do @(tasklist /FI \"IMAGENAME eq ET.exe\" | find /I \"ET.exe\" >nul && (taskkill /F /T /IM ET.exe >nul 2>&1 & ping -n 2 127.0.0.1 >nul) || exit /b 0)"'
  Pop $0
  Sleep 800
!macroend

!macro ET_UnlockInstallExe
  ; Best-effort: if a stale lock remains, move the old exe aside so File can write.
  ; $INSTDIR is set by Tauri before NSIS_HOOK_PREINSTALL.
  IfFileExists "$INSTDIR\easytier-gui.exe" 0 et_unlock_done
    DetailPrint "Trying to unlock $INSTDIR\easytier-gui.exe ..."
    Delete "$INSTDIR\easytier-gui.exe.bak"
    Rename "$INSTDIR\easytier-gui.exe" "$INSTDIR\easytier-gui.exe.bak"
    Delete /REBOOTOK "$INSTDIR\easytier-gui.exe.bak"
  et_unlock_done:
!macroend

!macro ET_EnsureGuiStopped
  !insertmacro ET_StopGuiService
  !insertmacro ET_KillGuiProcesses
  !insertmacro ET_WaitUntilGuiGone
  !insertmacro ET_UnlockInstallExe
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro ET_EnsureGuiStopped
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; We set start= demand while replacing binaries; restore auto-start if the
  ; service still exists (GUI service mode). Ignore missing service.
  DetailPrint "Restoring ET Gui service start type if present..."
  nsExec::ExecToLog 'sc.exe config ET-Gui start= auto'
  Pop $0
  nsExec::ExecToLog 'sc.exe config easytier-gui start= auto'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro ET_EnsureGuiStopped
  DetailPrint "Removing ET Gui Windows service if present..."
  nsExec::ExecToLog 'sc.exe delete ET-Gui'
  Pop $0
  nsExec::ExecToLog 'sc.exe delete easytier-gui'
  Pop $0
  Sleep 500
!macroend
