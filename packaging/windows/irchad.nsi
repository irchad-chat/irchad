; IrChad installer: per-user (no admin), Start menu shortcut, irchad:// handler.
;   makensis /DVERSION=0.1.0 /DEXE=path\to\irchad.exe packaging\windows\irchad.nsi
Unicode true
!include "MUI2.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef EXE
  !define EXE "..\..\target\release\irchad.exe"
!endif
!define UNINST "Software\Microsoft\Windows\CurrentVersion\Uninstall\IrChad"

Name "IrChad"
OutFile "irchad-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\IrChad"
RequestExecutionLevel user

!define MUI_FINISHPAGE_RUN "$INSTDIR\irchad.exe"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "IrChad"
  SetOutPath "$INSTDIR"
  File "/oname=irchad.exe" "${EXE}"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortCut "$SMPROGRAMS\IrChad.lnk" "$INSTDIR\irchad.exe"

  ; The app writes the same keys at startup; this makes links work before the first run.
  WriteRegStr HKCU "Software\Classes\irchad" "" "URL:IrChad"
  WriteRegStr HKCU "Software\Classes\irchad" "URL Protocol" ""
  WriteRegStr HKCU "Software\Classes\irchad\shell\open\command" "" '"$INSTDIR\irchad.exe" "%1"'

  WriteRegStr HKCU "${UNINST}" "DisplayName" "IrChad"
  WriteRegStr HKCU "${UNINST}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST}" "Publisher" "IrChad"
  WriteRegStr HKCU "${UNINST}" "DisplayIcon" "$INSTDIR\irchad.exe"
  WriteRegStr HKCU "${UNINST}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKCU "${UNINST}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\irchad.exe"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\IrChad.lnk"
  DeleteRegKey HKCU "Software\Classes\irchad"
  DeleteRegKey HKCU "${UNINST}"
SectionEnd
