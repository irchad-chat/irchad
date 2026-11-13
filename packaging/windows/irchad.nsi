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

; IrChad running from $INSTDIR locks irchad.exe: offer to close it (silent
; installs just do). Forced, since closing its window may only hide it in the tray.
!macro CLOSE_RUNNING un
Function ${un}CloseRunning
  StrCpy $R0 0
check:
  IfFileExists "$INSTDIR\irchad.exe" 0 done
  ; A running exe can't be opened for writing.
  ClearErrors
  FileOpen $0 "$INSTDIR\irchad.exe" a
  IfErrors 0 free
  StrCmp $R0 0 0 stuck
  MessageBox MB_OKCANCEL|MB_ICONEXCLAMATION "IrChad is running. Close it and continue?" /SD IDOK IDOK close
  Abort
close:
  StrCpy $R0 1
  ; Only the copy in $INSTDIR, passed through the environment so the path needs no quoting.
  System::Call 'Kernel32::SetEnvironmentVariable(t "IRCHAD_EXE", t "$INSTDIR\irchad.exe")i'
  nsExec::Exec `powershell -NoProfile -NonInteractive -Command "Get-Process irchad -EA 0 | ? { $$_.Path -eq $$env:IRCHAD_EXE } | Stop-Process -Force -PassThru | Wait-Process -Timeout 10 -EA 0"`
  Pop $0
  Sleep 500
  Goto check
stuck:
  MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "IrChad didn't close. Quit it from the tray icon, then retry." /SD IDCANCEL IDRETRY check
  Abort
free:
  FileClose $0
done:
FunctionEnd
!macroend
!insertmacro CLOSE_RUNNING ""
!insertmacro CLOSE_RUNNING "un."

Section "IrChad"
  Call CloseRunning
  SetOutPath "$INSTDIR"
  File "/oname=irchad.exe" "${EXE}"
  File "irchad.ico"
  ; release.yml/nightly.yml generate these (packaging/licenses.sh's Windows twin).
  File "licenses\LICENSE"
  File "licenses\THIRD_PARTY_LICENSES.txt"
  File "licenses\LIBWEBRTC_LICENSES.md"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortCut "$SMPROGRAMS\IrChad.lnk" "$INSTDIR\irchad.exe" "" "$INSTDIR\irchad.ico"

  ; The app writes the same keys at startup; this makes links work before the first run.
  WriteRegStr HKCU "Software\Classes\irchad" "" "URL:IrChad"
  WriteRegStr HKCU "Software\Classes\irchad" "URL Protocol" ""
  WriteRegStr HKCU "Software\Classes\irchad\shell\open\command" "" '"$INSTDIR\irchad.exe" "%1"'

  WriteRegStr HKCU "${UNINST}" "DisplayName" "IrChad"
  WriteRegStr HKCU "${UNINST}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST}" "Publisher" "IrChad"
  WriteRegStr HKCU "${UNINST}" "DisplayIcon" "$INSTDIR\irchad.ico"
  WriteRegStr HKCU "${UNINST}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKCU "${UNINST}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  Call un.CloseRunning
  Delete "$INSTDIR\irchad.exe"
  Delete "$INSTDIR\irchad.ico"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\THIRD_PARTY_LICENSES.txt"
  Delete "$INSTDIR\LIBWEBRTC_LICENSES.md"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\IrChad.lnk"
  DeleteRegKey HKCU "Software\Classes\irchad"
  DeleteRegKey HKCU "${UNINST}"
SectionEnd
