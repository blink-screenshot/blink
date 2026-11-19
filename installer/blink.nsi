; Blink installer (NSIS). Per-user install, no admin rights needed.
;   makensis /DVERSION=0.2.0 /DEXE=..\target\release\Blink.exe installer\blink.nsi
;
; The in-app updater runs it as:  Blink-Setup-x.y.z.exe /S /RESTART
;   /S        silent (no pages)
;   /RESTART  start Blink again when done
Unicode true
!ifndef VERSION
  !define VERSION "0.2.0"
!endif
!ifndef EXE
  !define EXE "..\target\release\Blink.exe"
!endif
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Blink"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"

Name "Blink"
OutFile "..\dist\Blink-Setup-${VERSION}.exe"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\Blink"
InstallDirRegKey HKCU "Software\Blink" "InstallDir"
SetCompressor /SOLID lzma
ManifestDPIAware true
BrandingText "Blink ${VERSION}"

!include "MUI2.nsh"
!include "FileFunc.nsh"
!define MUI_ICON "..\assets\blink.ico"
!define MUI_UNICON "..\assets\blink.ico"
!define MUI_ABORTWARNING
!define MUI_COMPONENTSPAGE_NODESC
!define MUI_FINISHPAGE_RUN "$INSTDIR\Blink.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start Blink now"

!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Blink" SecMain
  SectionIn RO
  ; Close a running copy so the exe can be replaced.
  nsExec::Exec 'taskkill /F /IM Blink.exe'
  Sleep 400
  SetOutPath "$INSTDIR"
  File "/oname=Blink.exe" "${EXE}"
  ; Blink is GPL-3.0-or-later; the license and the notices of the components it includes
  ; go with every copy.
  File "/oname=LICENSE.txt" "..\LICENSE"
  File "/oname=THIRD-PARTY-NOTICES.md" "..\THIRD-PARTY-NOTICES.md"
  ; Windows caches the app name it shows in notifications. Older versions had none, so the
  ; cache says "Blink.exe"; clear it so the name from the new version info is used.
  DeleteRegValue HKCU "Software\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache" "$INSTDIR\Blink.exe.FriendlyAppName"
  DeleteRegValue HKCU "Software\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache" "$INSTDIR\Blink.exe.ApplicationCompany"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateShortcut "$SMPROGRAMS\Blink.lnk" "$INSTDIR\Blink.exe"
  WriteRegStr HKCU "Software\Blink" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "Blink"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "Blink contributors"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\Blink.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Section "Start with Windows" SecAutostart
  WriteRegStr HKCU "${RUN_KEY}" "Blink" '"$INSTDIR\Blink.exe"'
SectionEnd

Section /o "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\Blink.lnk" "$INSTDIR\Blink.exe"
SectionEnd

; Updating over an existing install keeps the user's "start with Windows" choice.
Function .onInit
  ReadRegStr $0 HKCU "Software\Blink" "InstallDir"
  StrCmp $0 "" done
  ReadRegStr $1 HKCU "${RUN_KEY}" "Blink"
  StrCmp $1 "" 0 done
  SectionSetFlags ${SecAutostart} 0
  done:
FunctionEnd

Function .onInstSuccess
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/RESTART" $R1
  IfErrors +2
  Exec '"$INSTDIR\Blink.exe"'
FunctionEnd

Section "Uninstall"
  nsExec::Exec 'taskkill /F /IM Blink.exe'
  Sleep 400
  Delete "$INSTDIR\Blink.exe"
  Delete "$INSTDIR\Uninstall.exe"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\THIRD-PARTY-NOTICES.md"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Blink.lnk"
  Delete "$DESKTOP\Blink.lnk"
  DeleteRegValue HKCU "${RUN_KEY}" "Blink"
  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\Blink"
  ; Settings (%APPDATA%\Blink) and your screenshots are left in place.
SectionEnd
