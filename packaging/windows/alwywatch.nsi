; SPDX-License-Identifier: AGPL-3.0-or-later
; AlwyWatch Windows installer (per-user, no admin rights needed).
; Built by scripts/package.sh windows:
;   makensis -DVERSION=0.1.0 -DSRC=<dir with alwywatch.exe [+ libmpv-2.dll]> -DOUT=<setup.exe> alwywatch.nsi
Unicode true
SetCompressor /SOLID lzma
RequestExecutionLevel user

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!define APP "AlwyWatch"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP}"

Name "${APP}"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\Programs\${APP}"
InstallDirRegKey HKCU "Software\${APP}" "InstallDir"
Icon "alwywatch.ico"
UninstallIcon "alwywatch.ico"
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${APP}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "AGPL-3.0-or-later"
VIAddVersionKey "FileDescription" "${APP} installer"

!include "MUI2.nsh"
!define MUI_ICON "alwywatch.ico"
!define MUI_UNICON "alwywatch.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\alwywatch.exe"
!insertmacro MUI_PAGE_LICENSE "${SRC}\LICENSE.txt"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "French"

Section "Install"
  SetOutPath "$INSTDIR"
  ; Close a running copy first so files can be replaced on upgrade.
  nsExec::Exec 'taskkill /IM alwywatch.exe /F'
  File "${SRC}\alwywatch.exe"
  File "${SRC}\LICENSE.txt"
  ; libmpv (playback) is optional at build time: without it the app browses and says so.
  File /nonfatal "${SRC}\libmpv-2.dll"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortCut "$SMPROGRAMS\${APP}.lnk" "$INSTDIR\alwywatch.exe"
  WriteRegStr HKCU "Software\${APP}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${APP}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "Alwy"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\alwywatch.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  nsExec::Exec 'taskkill /IM alwywatch.exe /F'
  Delete "$INSTDIR\alwywatch.exe"
  Delete "$INSTDIR\libmpv-2.dll"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${APP}.lnk"
  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\${APP}"
  ; User data (%APPDATA%\AlwyWatch: cache, tokens, downloads list) is left in place on purpose.
SectionEnd
