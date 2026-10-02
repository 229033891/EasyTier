; Tauri NSIS hooks for ET GUI.
;
; Closing the main window only hides to tray (prevent_close), so
; easytier-gui.exe often stays running and locks the install path:
;   Error opening file for writing: ...\ET\easytier-gui.exe
;
; Also: switching to "service mode" registers Windows service "ET-Gui".
; The App uninstaller does not call GUI service uninstall by itself, so
; remove the service here on uninstall to avoid leftovers.
;
; Kill the GUI (and any leftover child tree) before overwrite / uninstall.
; taskkill / sc return non-zero when nothing matches — ignore that.

!macro NSIS_HOOK_PREINSTALL
  DetailPrint "Stopping running ET GUI (tray) if present..."
  nsExec::ExecToLog 'taskkill /F /T /IM easytier-gui.exe'
  Pop $0
  ; Give Windows a moment to release file handles.
  Sleep 800
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Stopping ET Gui Windows service if present..."
  nsExec::ExecToLog 'sc.exe stop ET-Gui'
  Pop $0
  nsExec::ExecToLog 'sc.exe delete ET-Gui'
  Pop $0
  DetailPrint "Stopping running ET GUI (tray) if present..."
  nsExec::ExecToLog 'taskkill /F /T /IM easytier-gui.exe'
  Pop $0
  Sleep 800
!macroend
