; Tauri NSIS hooks for the privileged Windows Core Service lifecycle.
; Unsigned engineering installers keep the desktop app usable but never activate SCM.

!define WEBDYNAMITE_HOOK_DIR "${__FILEDIR__}"

!macro WEBDYNAMITE_EMBED_LIFECYCLE_SCRIPT
  SetOutPath "$PLUGINSDIR"
  File /oname=wd-install-core-service.ps1 "${WEBDYNAMITE_HOOK_DIR}\..\..\scripts\windows\install-core-service.ps1"
  File /oname=manage-core-service.ps1 "${WEBDYNAMITE_HOOK_DIR}\..\..\scripts\windows\manage-core-service.ps1"
  File /oname=provision-service-layout.ps1 "${WEBDYNAMITE_HOOK_DIR}\..\..\scripts\windows\provision-service-layout.ps1"
  File /oname=provision-trusted-service-config.ps1 "${WEBDYNAMITE_HOOK_DIR}\..\..\scripts\windows\provision-trusted-service-config.ps1"
!macroend

!macro WEBDYNAMITE_ABORT_ON_FAILURE
  ${If} $0 != 0
    MessageBox MB_ICONSTOP|MB_OK "WebDynamite Core Service setup failed (exit code $0). The installer cannot safely continue. Review the installer details and Windows service state before retrying."
    Abort
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro WEBDYNAMITE_EMBED_LIFECYCLE_SCRIPT
  ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\wd-install-core-service.ps1" -Mode StopForUpgrade -InstallRoot "$INSTDIR"' $0
  !insertmacro WEBDYNAMITE_ABORT_ON_FAILURE
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro WEBDYNAMITE_EMBED_LIFECYCLE_SCRIPT
  ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\wd-install-core-service.ps1" -Mode Install -InstallRoot "$INSTDIR"' $0
  !insertmacro WEBDYNAMITE_ABORT_ON_FAILURE
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; The scripts are installed as resources so the uninstaller can call them
  ; before removing the resources directory. File extraction is not attempted
  ; from the uninstaller's separate executable.
  ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\resources\install-core-service.ps1" -Mode Uninstall -InstallRoot "$INSTDIR"' $0
  !insertmacro WEBDYNAMITE_ABORT_ON_FAILURE
!macroend
