; Tauri NSIS hooks for ET GUI.
;
; Closing the main window only hides to tray (prevent_close), so
; easytier-gui.exe often stays running and locks the install path:
;   Error opening file for writing: ...\ET\easytier-gui.exe
;
; Service mode registers Windows service "ET-Gui" which may restart the
; process after a plain taskkill. Stop the service first, then kill the
; GUI (and a short retry) before overwrite / uninstall.
;
; taskkill / sc return non-zero when nothing matches — ignore that.
;
; NOTE: NSIS_HOOK_PREINSTALL runs before Tauri's CheckIfAppIsRunning.

!macro ET_StopGuiService
  DetailPrint "Stopping ET Gui Windows service if present..."
  ; Current service name (sc): ET-Gui  | display name: ET Gui Service
  nsExec::ExecToLog 'sc.exe stop ET-Gui'
  Pop $0
  ; Pre-rename leftover (sc): easytier-gui  | display name: EasyTier Gui Service
  nsExec::ExecToLog 'sc.exe stop easytier-gui'
  Pop $0
  Sleep 1000
!macroend

!macro ET_KillGuiProcesses
  DetailPrint "Stopping running ET GUI processes..."
  ; Primary binary name (crate / installed exe)
  nsExec::ExecToLog 'taskkill /F /T /IM easytier-gui.exe'
  Pop $0
  ; If a future build renames the main binary
  nsExec::ExecToLog 'taskkill /F /T /IM ET.exe'
  Pop $0
  Sleep 800
!macroend

!macro ET_EnsureGuiStopped
  !insertmacro ET_StopGuiService
  !insertmacro ET_KillGuiProcesses
  ; Retry once in case the service raced a restart
  !insertmacro ET_KillGuiProcesses
  Sleep 500
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro ET_EnsureGuiStopped
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
