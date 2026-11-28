; Tauri NSIS hooks for the privileged Windows Core Service lifecycle.
; Unsigned engineering installers keep the desktop app usable but never activate SCM.

!define WEBDYNAMITE_HOOK_DIR "${__FILEDIR__}"

!macro WEBDYNAMITE_RUN_SERVICE_LIFECYCLE MODE
  SetOutPath "$PLUGINSDIR"
  File /oname=wd-install-core-service.ps1 "${WEBDYNAMITE_HOOK_DIR}\..\..\scripts\windows\install-core-service.ps1"
  File /oname=wd-manage-core-service.ps1 "${WEBDYNAMITE_HOOK_DIR}\..\..\scripts\windows\manage-core-service.ps1"
  File /oname=wd-provision-service-layout.ps1 "${WEBDYNAMITE_HOOK_DIR}\..\..\scripts\windows\provision-service-layout.ps1"
  File /oname=wd-provision-trusted-service-config.ps1 "${WEBDYNAMITE_HOOK_DIR}\..\..\scripts\windows\provision-trusted-service-config.ps1"
  ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\wd-install-core-service.ps1" -Mode ${MODE} -InstallRoot "$INSTDIR"' $0
  ${If} $0 != 0
    MessageBox MB_ICONSTOP|MB_OK "WebDynamite Core Service setup failed (exit code $0). The installer cannot safely continue. Review the installer details and Windows service state before retrying."
    Abort
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro WEBDYNAMITE_RUN_SERVICE_LIFECYCLE "StopForUpgrade"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro WEBDYNAMITE_RUN_SERVICE_LIFECYCLE "Install"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro WEBDYNAMITE_RUN_SERVICE_LIFECYCLE "Uninstall"
!macroend
