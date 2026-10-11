; Tauri NSIS hooks for ET GUI.
;
; Two independent lock classes, both must be handled before the file copy:
;
; 1. $INSTDIR\ET.exe (legacy: easytier-gui.exe) - the app hides to tray on
;    close, so it often stays running and locks the install path:
;      Error opening file for writing: ...\ET\ET.exe
;
; 2. $INSTDIR\Packet.dll / wintun.dll / WinDivert*.sys - third-party binaries.
;    Packet.dll is a **static import** of the GUI exe (pnet_datalink's
;    `#[link(name = "Packet")]`), so it is mapped for the whole process lifetime;
;    wintun.dll is LoadLibrary'd while a TUN exists; WinDivert*.sys is mapped by
;    the kernel driver. Any surviving process therefore locks them:
;      Error opening file for writing: ...\ET\Packet.dll
;    See ET_UnlockThirdPartyBinaries.
;
; Service mode registers Windows service "ET-Gui" with restart-on-failure.
; A plain taskkill can race the SCM restart and leave the exe locked again.
; Clear failure actions, set start= demand, stop the service, wait until it
; leaves RUNNING, force-kill with retries, then rename the main exe aside.
;
; NOTE: a per-user (default) install is not elevated, so `sc.exe stop ET-Gui`
; on a SYSTEM-owned service fails with access denied and the PowerShell sweep
; cannot even read its ExecutablePath.
;
; Install therefore stops that service ELEVATED first (one UAC prompt, only when
; a service is actually running - see ET_ElevateServiceStopForInstall), so the
; stop/kill/wait below and the file copy run against a quiet $INSTDIR. The
; rename-aside in ET_UnlockThirdPartyBinaries / ET_UnlockInstallExe stays as the
; fallback for the case where elevation is declined or a stray process survives.
;
; Uninstall: the same elevation gap means `sc.exe delete ET-Gui` fails too, and
; the service would outlive the uninstall (still registered, pointing at the
; removed exe). ET_DeleteGuiService retries it elevated and reports if it is
; still there. This is NOT Windows-version specific - the whole chain (static
; import lock, per-user unelevated install, SYSTEM service) behaves the same on
; Windows 10 and 11; it only *looks* like a Win11 problem because the Windows
; Server machines where it "does not happen" simply had nothing running.
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
; taskkill / sc return non-zero when nothing matches - ignore that.
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
  ; Current main binary
  nsExec::ExecToLog 'taskkill /F /T /IM ET.exe'
  Pop $0
  ; Legacy binary name (pre-rename installs)
  nsExec::ExecToLog 'taskkill /F /T /IM easytier-gui.exe'
  Pop $0
  Sleep 400
!macroend

!macro ET_WaitUntilGuiGone
  ; Retry kill until neither current nor legacy main binary remains.
  DetailPrint "Waiting for ET.exe / easytier-gui.exe to release install files..."
  nsExec::ExecToLog 'cmd /c "for /L %i in (1,1,25) do @(tasklist /FI \"IMAGENAME eq ET.exe\" | find /I \"ET.exe\" >nul && (taskkill /F /T /IM ET.exe >nul 2>&1 & ping -n 2 127.0.0.1 >nul) || exit /b 0)"'
  Pop $0
  nsExec::ExecToLog 'cmd /c "for /L %i in (1,1,25) do @(tasklist /FI \"IMAGENAME eq easytier-gui.exe\" | find /I \"easytier-gui.exe\" >nul && (taskkill /F /T /IM easytier-gui.exe >nul 2>&1 & ping -n 2 127.0.0.1 >nul) || exit /b 0)"'
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

!macro ET_UnlockOneInstallExe EXEBASE
  ; Move one main-binary candidate aside so Tauri's post-uninstall FileExists
  ; check cannot false-fail. Prefer Rename (atomic unlock) over Delete /REBOOTOK,
  ; which leaves the file visible until reboot and still trips FileExists.
  ; Labels use __LINE__ so this macro can be inserted more than once per function.
  !define ET_UNLOCK_ID ${__LINE__}
  IfFileExists "$INSTDIR\${EXEBASE}.exe" 0 et_unlock_done_${ET_UNLOCK_ID}
    DetailPrint "Trying to unlock $INSTDIR\${EXEBASE}.exe ..."
    Delete "$INSTDIR\${EXEBASE}.exe.bak"
    ClearErrors
    Rename "$INSTDIR\${EXEBASE}.exe" "$INSTDIR\${EXEBASE}.exe.bak"
    IfErrors 0 et_unlock_renamed_${ET_UNLOCK_ID}
      ; Still locked: one more kill+wait then retry rename.
      !insertmacro ET_KillGuiProcesses
      Sleep 500
      ClearErrors
      Rename "$INSTDIR\${EXEBASE}.exe" "$INSTDIR\${EXEBASE}.exe.bak"
      IfErrors 0 et_unlock_renamed_${ET_UNLOCK_ID}
        ; Last resort: cmd move often succeeds when NSIS Rename fails.
        nsExec::ExecToLog 'cmd /c move /Y "$INSTDIR\${EXEBASE}.exe" "$INSTDIR\${EXEBASE}.exe.bak"'
        Pop $0
        IfFileExists "$INSTDIR\${EXEBASE}.exe" 0 et_unlock_renamed_${ET_UNLOCK_ID}
          Goto et_unlock_done_${ET_UNLOCK_ID}
    et_unlock_renamed_${ET_UNLOCK_ID}:
    Delete /REBOOTOK "$INSTDIR\${EXEBASE}.exe.bak"
  et_unlock_done_${ET_UNLOCK_ID}:
  !undef ET_UNLOCK_ID
!macroend

!macro ET_UnlockInstallExe
  ; Current name first; keep legacy so upgrades from older installs unlock too.
  !insertmacro ET_UnlockOneInstallExe "ET"
  !insertmacro ET_UnlockOneInstallExe "easytier-gui"
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

!macro ET_UnlockThirdPartyBinaries
  ; Second lock class, separate from the main exe.
  ;
  ; Packet.dll is a *static import* of the GUI exe (pnet_datalink declares
  ; `#[link(name = "Packet")]`), so Windows maps it at process start and keeps it
  ; locked for the entire process lifetime. wintun.dll is LoadLibrary'd while a
  ; TUN adapter exists, and WinDivert*.sys is mapped by the kernel driver.
  ;
  ; ET_UnlockInstallExe above only renames the main binary aside, so ANY surviving
  ; process still locks these files and the resource copy fails with
  ;   Error opening file for writing: $INSTDIR\Packet.dll
  ; The usual survivor is the SYSTEM-owned ET-Gui service: a per-user install
  ; cannot stop it (sc.exe stop -> access denied), so kill/service-stop silently
  ; fails and the install hard-errors on the DLL instead of the exe.
  ;
  ; Renaming the old file aside releases the path without needing the owner to
  ; exit (mapped image sections can be renamed, which is why the exe trick works).
  ; It also clears a read-only attribute left by an earlier install, and the
  ; /REBOOTOK delete covers a .bak that is still mapped.
  ;
  ; Used by BOTH hooks: PREINSTALL (as the fallback when the elevated service
  ; stop was declined, so the resource copy can still overwrite) and
  ; PREUNINSTALL (so the template's Delete + RMDir can actually remove them).
  ; When the owner is really gone the .bak is deleted immediately, so this
  ; leaves nothing behind in the normal case.
  ;
  ; Deliberately label-free: this macro is expanded inside a hook macro, and
  ; duplicate labels would break makensis if it is ever inserted twice.
  DetailPrint "Unlocking third-party binaries in $INSTDIR ..."

  Delete /REBOOTOK "$INSTDIR\Packet.dll.bak"
  Rename "$INSTDIR\Packet.dll" "$INSTDIR\Packet.dll.bak"
  nsExec::ExecToLog 'cmd /c if exist "$INSTDIR\Packet.dll" move /Y "$INSTDIR\Packet.dll" "$INSTDIR\Packet.dll.bak"'
  Pop $0
  Delete /REBOOTOK "$INSTDIR\Packet.dll.bak"

  Delete /REBOOTOK "$INSTDIR\wintun.dll.bak"
  Rename "$INSTDIR\wintun.dll" "$INSTDIR\wintun.dll.bak"
  nsExec::ExecToLog 'cmd /c if exist "$INSTDIR\wintun.dll" move /Y "$INSTDIR\wintun.dll" "$INSTDIR\wintun.dll.bak"'
  Pop $0
  Delete /REBOOTOK "$INSTDIR\wintun.dll.bak"

  Delete /REBOOTOK "$INSTDIR\WinDivert64.sys.bak"
  Rename "$INSTDIR\WinDivert64.sys" "$INSTDIR\WinDivert64.sys.bak"
  nsExec::ExecToLog 'cmd /c if exist "$INSTDIR\WinDivert64.sys" move /Y "$INSTDIR\WinDivert64.sys" "$INSTDIR\WinDivert64.sys.bak"'
  Pop $0
  Delete /REBOOTOK "$INSTDIR\WinDivert64.sys.bak"

  Delete /REBOOTOK "$INSTDIR\WinDivert32.sys.bak"
  Rename "$INSTDIR\WinDivert32.sys" "$INSTDIR\WinDivert32.sys.bak"
  nsExec::ExecToLog 'cmd /c if exist "$INSTDIR\WinDivert32.sys" move /Y "$INSTDIR\WinDivert32.sys" "$INSTDIR\WinDivert32.sys.bak"'
  Pop $0
  Delete /REBOOTOK "$INSTDIR\WinDivert32.sys.bak"

  ; Leave the error flag clean: the template continues with File/IfErrors.
  ClearErrors
!macroend

!macro ET_DeleteOneService SVC SUFFIX
  ; `sc delete` requires admin, but a per-user install/uninstall runs with
  ; RequestExecutionLevel user. The plain call therefore fails with access
  ; denied (exit 5) and the service SURVIVES the uninstall - still registered,
  ; still pointing at the removed exe (SCM then fails to start it at boot).
  ;
  ; The elevated retry stops it first: `sc delete` on a *running* service only
  ; marks it for deletion, which would make the verification below report a
  ; false failure. Failure actions are cleared so the SCM cannot restart it
  ; mid-sequence. `ping -n 3` is the file's usual ~2s sleep idiom.
  ;
  ; Labels are suffixed per service because this macro is inserted twice;
  ; reusing the same SUFFIX would collide.
  nsExec::ExecToLog '"$SYSDIR\sc.exe" query "${SVC}"'
  Pop $0
  StrCmp $0 0 0 et_svc_done_${SUFFIX}   ; 1060 = no such service, nothing to do

  nsExec::ExecToLog '"$SYSDIR\sc.exe" delete "${SVC}"'
  Pop $0
  StrCmp $0 0 et_svc_done_${SUFFIX} 0

  DetailPrint "Deleting service ${SVC} needs elevation; retrying with a UAC prompt..."
  ExecShellWait "runas" "$SYSDIR\cmd.exe" '/c sc failure "${SVC}" reset= 0 actions= // & sc stop "${SVC}" & ping -n 3 127.0.0.1 >nul & sc delete "${SVC}"' SW_HIDE
  Sleep 1500
  nsExec::ExecToLog '"$SYSDIR\sc.exe" query "${SVC}"'
  Pop $0
  StrCmp $0 0 0 et_svc_done_${SUFFIX}
  MessageBox MB_ICONEXCLAMATION|MB_OK "The Windows service '${SVC}' could not be removed.$\r$\n$\r$\nIt may be missing admin rights, or still be marked for deletion until the next reboot.$\r$\n$\r$\nTo remove it manually, run as administrator:$\r$\n$\r$\n    sc delete ${SVC}"

  et_svc_done_${SUFFIX}:
  ClearErrors
!macroend

!macro ET_DeleteGuiService
  DetailPrint "Removing ET Gui Windows service if present..."
  !insertmacro ET_DeleteOneService "ET-Gui" etgui
  !insertmacro ET_DeleteOneService "easytier-gui" legacy
!macroend

!macro ET_ElevateServiceStopForInstall
  ; Install-side counterpart of ET_DeleteGuiService.
  ;
  ; A per-user install runs with RequestExecutionLevel user, so ET_StopGuiService
  ; cannot stop the SYSTEM-owned ET-Gui service: every later step (kill, wait,
  ; file copy) would then be fighting a live process and fall back to
  ; unlock-by-rename, leaving a .bak plus a pending reboot delete. Stop it
  ; elevated instead - but ONLY when a service is actually still running, so a
  ; plain install without service mode never sees a UAC prompt.
  ;
  ; If elevation is declined we just continue: ET_UnlockInstallExe and
  ; ET_UnlockThirdPartyBinaries still make the copy succeed.
  ;
  ; Labels are safe here (and only here): NSIS_HOOK_PREINSTALL expands exactly
  ; once, into Section Install. The macro is NOT reusable from a hook that may
  ; expand more than once.
  nsExec::ExecToLog 'cmd /c "(sc query ET-Gui 2>nul | findstr /I \"RUNNING\" >nul || sc query easytier-gui 2>nul | findstr /I \"RUNNING\" >nul) && exit /b 1 || exit /b 0"'
  Pop $0
  StrCmp $0 1 0 et_install_svc_stop_done   ; 0 = nothing running, no elevation needed

  DetailPrint "ET Gui service is still running; requesting elevation to stop it..."
  ClearErrors
  ; start= demand is restored to auto by NSIS_HOOK_POSTINSTALL afterwards.
  ExecShellWait "runas" "$SYSDIR\cmd.exe" '/c sc failure ET-Gui reset= 0 actions= // & sc config ET-Gui start= demand & sc stop ET-Gui & sc failure easytier-gui reset= 0 actions= // & sc config easytier-gui start= demand & sc stop easytier-gui & ping -n 4 127.0.0.1 >nul' SW_HIDE
  IfErrors 0 et_install_svc_stop_done
    DetailPrint "Elevation declined or failed; falling back to unlock-by-rename."

  et_install_svc_stop_done:
  ClearErrors
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; First: get the SYSTEM service out of the way with one UAC prompt, so the
  ; stop/kill/wait below and the file copy run against a quiet $INSTDIR.
  !insertmacro ET_ElevateServiceStopForInstall
  !insertmacro ET_EnsureGuiStopped
  ; Fallback when the service could not be stopped (no elevation granted) or a
  ; stray process survives: rename the locked files aside so the copy still wins.
  !insertmacro ET_UnlockThirdPartyBinaries
!macroend

!macro ET_MigrateGuiServiceBinPath
  ; Service binPath embeds the absolute exe path at install time. After renaming
  ; the main binary to ET.exe, leave PathName pointing at easytier-gui.exe would
  ; break Service mode once the legacy file is removed.
  DetailPrint "Migrating ET Gui service PathName easytier-gui.exe -> ET.exe if needed..."
  nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "foreach ($$n in @(''ET-Gui'',''easytier-gui'')) { $$s = Get-CimInstance Win32_Service -Filter (\"Name=''$$n''\") -ErrorAction SilentlyContinue; if (-not $$s -or -not $$s.PathName) { continue }; if ($$s.PathName -notmatch ''(?i)easytier-gui\.exe'') { continue }; $$np = [regex]::Replace($$s.PathName, ''easytier-gui\.exe'', ''ET.exe'', ''IgnoreCase''); if ($$np -eq $$s.PathName) { continue }; & sc.exe config $$n binPath= $$np | Out-Null }"'
  Pop $0
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; We set start= demand while replacing binaries; restore auto-start if the
  ; service still exists (GUI service mode). Ignore missing service.
  DetailPrint "Restoring ET Gui service start type if present..."
  nsExec::ExecToLog 'sc.exe config ET-Gui start= auto'
  Pop $0
  nsExec::ExecToLog 'sc.exe config easytier-gui start= auto'
  Pop $0
  !insertmacro ET_MigrateGuiServiceBinPath
  ; Keep a same-bytes legacy shim: per-user installs often cannot elevate
  ; `sc config binPath=`, so ET-Gui may still point at easytier-gui.exe.
  ; Also covers old shortcuts. Safe to remove in a later release once migration
  ; has baked in.
  IfFileExists "$INSTDIR\ET.exe" 0 et_postinstall_shim_done
    DetailPrint "Writing legacy shim $INSTDIR\easytier-gui.exe (= ET.exe)..."
    nsExec::ExecToLog 'cmd /c copy /Y "$INSTDIR\ET.exe" "$INSTDIR\easytier-gui.exe"'
    Pop $0
  et_postinstall_shim_done:
  Delete /REBOOTOK "$INSTDIR\easytier-gui.exe.bak"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro ET_EnsureGuiStopped
  ; Locked third-party binaries would make the template's Delete fail and leave
  ; $INSTDIR behind; rename them aside so those deletes succeed (the .bak is
  ; removed at reboot while it is still mapped).
  !insertmacro ET_UnlockThirdPartyBinaries
  ; Before the template deletes the exe: a surviving registration would keep
  ; pointing at a removed binary and fail at boot.
  !insertmacro ET_DeleteGuiService
  ; Ensure main exe is gone/renamed so the parent installer does not see
  ; FileExists("$INSTDIR\${MAINBINARYNAME}.exe") and pop "Unable to uninstall!".
  !insertmacro ET_UnlockInstallExe
!macroend
