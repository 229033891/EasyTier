; Tauri NSIS hooks for ET GUI.
;
; Closing the main window only hides to tray (prevent_close), so
; easytier-gui.exe often stays running and locks the install path:
;   Error opening file for writing: ...\ET\easytier-gui.exe
;
; Service mode registers Windows service "ET-Gui" with restart-on-failure.
; A plain taskkill can race the SCM restart and leave the exe locked again.
; Clear failure actions, set start= demand, stop the service, wait until it
; leaves RUNNING, force-kill with retries, then rename the main exe aside.
;
; Tauri's "Uninstall before installing" fails with "Unable to uninstall!" when
; either the old uninstaller exit code is non-zero OR
;   $INSTDIR\${MAINBINARYNAME}.exe still exists afterwards.
; Unlocking/renaming the main binary before that check is therefore required.
;
; CRITICAL: Tauri launches uninstall.exe with `_?=$INSTDIR`, so it runs from
; the install dir (not %TEMP%). Any "kill every process under $INSTDIR" sweep
; MUST exclude uninstall.exe, or ExecWait sees a non-zero exit and pops
; "Unable to uninstall!".
;
; taskkill / sc return non-zero when nothing matches — ignore that.
;
; NOTE: NSIS_HOOK_PREINSTALL runs before Tauri's CheckIfAppIsRunning.
; NOTE: PageLeaveReinstall (custom installer.nsi) also calls ET_EnsureGuiStopped
;       before ExecWait so upgrades work even when the *old* uninstall.exe
;       was built without these hooks.

!macro ET_ClearGuiServiceRecovery
  ; Clear restart-on-failure so taskkill is not undone by SCM.
  ; `actions= //` clears the action list on modern Windows sc.exe.
  nsExec::ExecToLog 'sc.exe failure ET-Gui reset= 0 actions= //'
  Pop $0
  nsExec::ExecToLog 'sc.exe failure easytier-gui reset= 0 actions= //'
  Pop $0
!macroend

!macro ET_StopGuiService
  DetailPrint "Stopping ET Gui Windows service if present..."
  !insertmacro ET_ClearGuiServiceRecovery
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
  nsExec::ExecToLog 'cmd /c "for /L %i in (1,1,20) do @(sc query ET-Gui 2>nul | findstr /I \"RUNNING\" >nul && (ping -n 2 127.0.0.1 >nul) || exit /b 0)"'
  Pop $0
  nsExec::ExecToLog 'cmd /c "for /L %i in (1,1,20) do @(sc query easytier-gui 2>nul | findstr /I \"RUNNING\" >nul && (ping -n 2 127.0.0.1 >nul) || exit /b 0)"'
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
  nsExec::ExecToLog 'cmd /c "for /L %i in (1,1,25) do @(tasklist /FI \"IMAGENAME eq easytier-gui.exe\" | find /I \"easytier-gui.exe\" >nul && (taskkill /F /T /IM easytier-gui.exe >nul 2>&1 & ping -n 2 127.0.0.1 >nul) || exit /b 0)"'
  Pop $0
  nsExec::ExecToLog 'cmd /c "for /L %i in (1,1,10) do @(tasklist /FI \"IMAGENAME eq ET.exe\" | find /I \"ET.exe\" >nul && (taskkill /F /T /IM ET.exe >nul 2>&1 & ping -n 2 127.0.0.1 >nul) || exit /b 0)"'
  Pop $0
  Sleep 800
!macroend

!macro ET_SweepInstdirProcesses
  ; Kill processes whose ExecutablePath is under $INSTDIR, but NEVER
  ; uninstall.exe (Tauri `_?=` keeps it running from $INSTDIR).
  DetailPrint "Sweeping processes still running from $INSTDIR ..."
  nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$d = $$env:INSTDIR_FOR_SWEEP; if (-not $$d) { exit 0 }; $$d = $$d.TrimEnd([char]92) + [char]92; $$end = (Get-Date).AddSeconds(12); do { $$p = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object { $$_.ExecutablePath -and $$_.ExecutablePath.StartsWith($$d, [System.StringComparison]::OrdinalIgnoreCase) -and ($$_.Name -ine \"uninstall.exe\") -and ($$_.ProcessId -ne $$PID) }); $$p | ForEach-Object { Stop-Process -Id $$_.ProcessId -Force -ErrorAction SilentlyContinue }; if ($$p.Count -eq 0) { exit 0 }; Start-Sleep -Milliseconds 400 } while ((Get-Date) -lt $$end); exit 0"'
  Pop $0
!macroend

!macro ET_UnlockInstallExe
  ; Move the main binary aside so Tauri's post-uninstall FileExists check
  ; cannot false-fail. Prefer Rename (atomic unlock) over Delete /REBOOTOK,
  ; which leaves the file visible until reboot and still trips FileExists.
  ; Labels use __LINE__ so this macro can be inserted more than once per function.
  !define ET_UNLOCK_ID ${__LINE__}
  IfFileExists "$INSTDIR\easytier-gui.exe" 0 et_unlock_done_${ET_UNLOCK_ID}
    DetailPrint "Trying to unlock $INSTDIR\easytier-gui.exe ..."
    Delete "$INSTDIR\easytier-gui.exe.bak"
    ClearErrors
    Rename "$INSTDIR\easytier-gui.exe" "$INSTDIR\easytier-gui.exe.bak"
    IfErrors 0 et_unlock_renamed_${ET_UNLOCK_ID}
      ; Still locked: one more kill+wait then retry rename.
      !insertmacro ET_KillGuiProcesses
      Sleep 500
      ClearErrors
      Rename "$INSTDIR\easytier-gui.exe" "$INSTDIR\easytier-gui.exe.bak"
      IfErrors 0 et_unlock_renamed_${ET_UNLOCK_ID}
        ; Last resort: cmd move often succeeds when NSIS Rename fails.
        nsExec::ExecToLog 'cmd /c move /Y "$INSTDIR\easytier-gui.exe" "$INSTDIR\easytier-gui.exe.bak"'
        Pop $0
        IfFileExists "$INSTDIR\easytier-gui.exe" 0 et_unlock_renamed_${ET_UNLOCK_ID}
          Goto et_unlock_done_${ET_UNLOCK_ID}
    et_unlock_renamed_${ET_UNLOCK_ID}:
    Delete /REBOOTOK "$INSTDIR\easytier-gui.exe.bak"
  et_unlock_done_${ET_UNLOCK_ID}:
  !undef ET_UNLOCK_ID
!macroend

!macro ET_EnsureGuiStopped
  !insertmacro ET_StopGuiService
  !insertmacro ET_KillGuiProcesses
  !insertmacro ET_WaitUntilGuiGone
  ; Export INSTDIR for the PowerShell sweep (child processes inherit env).
  System::Call 'Kernel32::SetEnvironmentVariable(t "INSTDIR_FOR_SWEEP", t "$INSTDIR")'
  !insertmacro ET_SweepInstdirProcesses
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
  ; Ensure main exe is gone/renamed so the parent installer does not see
  ; FileExists("$INSTDIR\easytier-gui.exe") and pop "Unable to uninstall!".
  !insertmacro ET_UnlockInstallExe
!macroend
