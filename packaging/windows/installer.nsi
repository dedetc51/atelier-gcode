Unicode true
ManifestDPIAware true
RequestExecutionLevel user
SetCompressor /SOLID lzma
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "WordFunc.nsh"
!include "x64.nsh"

!define APP "Atelier G-code"
!define APP_EXE "atelier-gcode-${VERSION}.exe"
!define REGKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\AtelierGcode"
Name "${APP}"
OutFile "${OUTPUT}"
InstallDir "$LOCALAPPDATA\Programs\Atelier G-code"
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${APP}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "Installation de l’Atelier G-code"
VIAddVersionKey "LegalCopyright" "Contributeurs de l’Atelier G-code"

!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_PARAMETERS "--mises-a-jour"
!define MUI_FINISHPAGE_RUN_TEXT "Ouvrir l’Atelier G-code"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "French"

!macro Prepare
  SetShellVarContext current
  SetRegView 64
  System::Call 'kernel32::CreateMutexW(p 0, i 0, w "Local\AtelierGcode.Setup") p .r0 ?e'
  Pop $1
  ${If} $0 == 0
  ${OrIf} $1 == 183
    MessageBox MB_OK "Une installation de l’Atelier est déjà ouverte." /SD IDOK
    SetErrorLevel 16
    Abort
  ${EndIf}
!macroend

Function .onInit
  !insertmacro Prepare
  ${IfNot} ${RunningX64}
    MessageBox MB_OK "Cette version nécessite Windows 64 bits." /SD IDOK
    SetErrorLevel 1
    Abort
  ${EndIf}
  ReadRegStr $0 HKCU "${REGKEY}" "InstallLocation"
  ReadRegStr $1 HKCU "${REGKEY}" "DisplayVersion"
  ${GetParameters} $2
  ClearErrors
  ${GetOptions} "$2" "/UPDATE" $3
  ${IfNot} ${Errors}
  ${AndIf} $0 == ""
    ; The application may have been uninstalled while the download was running.
    SetErrorLevel 2
    Abort
  ${EndIf}
  ${If} $0 != ""
    StrCpy $INSTDIR "$0"
  ${EndIf}
  ${If} $1 != ""
    ${VersionCompare} "$1" "${VERSION}" $2
    ${If} $2 == 1
      MessageBox MB_OK "Une version plus récente est déjà installée." /SD IDOK
      SetErrorLevel 3
      Abort
    ${EndIf}
  ${EndIf}
FunctionEnd

Section "Atelier G-code"
  SetOutPath "$INSTDIR"
  ; Stage the executable before switching the Start menu shortcut.
  IfFileExists "$INSTDIR\${APP_EXE}" files_ready
  ClearErrors
  File "/oname=installation.tmp" "${BINARY}"
  IfErrors failed
  Rename "$INSTDIR\installation.tmp" "$INSTDIR\${APP_EXE}"
  IfErrors failed
  files_ready:
  ClearErrors
  IfFileExists "$INSTDIR\session.lock" marker_ready
  FileOpen $0 "$INSTDIR\session.lock" w
  IfErrors failed
  FileClose $0
  marker_ready:
  WriteUninstaller "$INSTDIR\Desinstaller.exe"
  IfErrors failed
  CreateShortcut "$SMPROGRAMS\Atelier G-code.lnk" "$INSTDIR\${APP_EXE}" "--mises-a-jour"
  IfErrors failed
  WriteRegStr HKCU "${REGKEY}" "DisplayName" "${APP}"
  WriteRegStr HKCU "${REGKEY}" "DisplayIcon" "$INSTDIR\${APP_EXE}"
  WriteRegStr HKCU "${REGKEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${REGKEY}" "UninstallString" '"$INSTDIR\Desinstaller.exe"'
  WriteRegStr HKCU "${REGKEY}" "QuietUninstallString" '"$INSTDIR\Desinstaller.exe" /S'
  WriteRegStr HKCU "${REGKEY}" "URLInfoAbout" "https://github.com/dedetc51/atelier-gcode"
  WriteRegDWORD HKCU "${REGKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${REGKEY}" "NoRepair" 1
  WriteRegStr HKCU "${REGKEY}" "DisplayVersion" "${VERSION}"
  IfErrors failed

  ; Windows keeps a running EXE locked. A later installation can clean it up.
  FindFirst $0 $1 "$INSTDIR\atelier-gcode-*.exe"
  ${DoWhile} $1 != ""
    ${If} $1 != "${APP_EXE}"
      Delete "$INSTDIR\$1"
    ${EndIf}
    FindNext $0 $1
  ${Loop}
  FindClose $0
  SetErrorLevel 0
  Goto done
  failed:
    Delete "$INSTDIR\installation.tmp"
    SetErrorLevel 1
    Abort "L’installation a échoué. Ferme les fenêtres de l’Atelier et réessaie."
  done:
SectionEnd

Function un.onInit
  !insertmacro Prepare
FunctionEnd

Section "Uninstall"
  ClearErrors
  Delete "$INSTDIR\session.lock"
  IfFileExists "$INSTDIR\session.lock" in_use
  Delete "$INSTDIR\atelier-gcode-*.exe"
  IfFileExists "$INSTDIR\atelier-gcode-*.exe" in_use
  Delete "$SMPROGRAMS\Atelier G-code.lnk"
  Delete "$INSTDIR\installation.tmp"
  Delete "$INSTDIR\update.lock"
  DeleteRegKey HKCU "${REGKEY}"
  Delete "$INSTDIR\Desinstaller.exe"
  RMDir "$INSTDIR"
  SetErrorLevel 0
  Goto done
  in_use:
    IfFileExists "$INSTDIR\session.lock" notify
    FileOpen $0 "$INSTDIR\session.lock" w
    FileClose $0
    notify:
    MessageBox MB_OK "Ferme l’Atelier G-code, puis relance la désinstallation." /SD IDOK
    SetErrorLevel 1
    Abort
  done:
SectionEnd
