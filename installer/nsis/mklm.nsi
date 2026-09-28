; Multi Keyboard Layout Manager (MKLM) installer - NSIS 3.12 or later (3.11 fixed CVE-2025-43715).
; Build it with installer\build-installer.ps1, which passes the defines below.
;
; What it does (plan 4.1, adapted from MSI to NSIS by the user's decision of 2026-09-28):
; - Per-machine install into %ProgramFiles%\SHIN DATA CENTER\MKLM (admin-only writable, so the
;   elevated mklm-helper.exe the GUI and CLI start from their own directory cannot be replaced by
;   a standard user).
; - Start menu shortcut and an "Apps" (ARP) entry with Publisher "SHIN DATA CENTER".
; - Refuses Windows older than 11 24H2 (build 26100), 32-bit Windows, and the ARM64 package on x64.
; - Upgrade in place: quits a running MKLM first; refuses while mklm-helper.exe is changing settings.
; - Uninstall: optionally puts the keyboard values back to their state before MKLM (per the
;   "restore on uninstall" machine setting, asked interactively), then removes the files.
; - Writes nothing under HKLM\SOFTWARE\SHIN DATA CENTER: those keys are created by the helper with
;   a protected DACL that it verifies (design m2 G.1); keys created here with inherited ACLs would
;   fail that check.

Unicode true
ManifestDPIAware true
ManifestSupportedOS Win10
RequestExecutionLevel admin
SetCompressor /SOLID lzma

!ifndef VERSION
  !error "Pass /DVERSION=<x.y.z>"
!endif
!ifndef VIVERSION
  !error "Pass /DVIVERSION=<x.y.z.w> (numeric)"
!endif
!ifndef ARCH
  !error "Pass /DARCH=x64 or /DARCH=arm64"
!endif
!ifndef SRCDIR
  !error "Pass /DSRCDIR=<directory with mklm.exe, mklm-cli.exe, mklm-helper.exe>"
!endif
!ifndef ROOT
  !error "Pass /DROOT=<repository root>"
!endif
!ifndef OUTFILE
  !define OUTFILE "MKLM-Setup-${VERSION}-${ARCH}.exe"
!endif

!define PRODUCT "Multi Keyboard Layout Manager"
!define PUBLISHER "SHIN DATA CENTER"
!define ARP_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\SHINDATACENTER.MKLM"
!define SETTINGS_KEY "SOFTWARE\SHIN DATA CENTER\MKLM\Settings"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define RUN_VALUE "SHINDATACENTER.MKLM"
!define MIN_BUILD 26100
!define REPO_URL "https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager"

!include "MUI2.nsh"
!include "x64.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"

Name "${PRODUCT}"
OutFile "${OUTFILE}"
InstallDir "$PROGRAMFILES64\SHIN DATA CENTER\MKLM"
BrandingText "${PUBLISHER}"
ShowInstDetails show
ShowUninstDetails show

VIProductVersion "${VIVERSION}"
VIFileVersion "${VIVERSION}"
VIAddVersionKey /LANG=0 "ProductName" "${PRODUCT}"
VIAddVersionKey /LANG=0 "CompanyName" "${PUBLISHER}"
VIAddVersionKey /LANG=0 "LegalCopyright" "© 2026 ${PUBLISHER}"
VIAddVersionKey /LANG=0 "FileDescription" "${PRODUCT} Setup (${ARCH})"
VIAddVersionKey /LANG=0 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=0 "ProductVersion" "${VERSION}"

!ifdef ICON
  !define MUI_ICON "${ICON}"
  !define MUI_UNICON "${ICON}"
!endif
!define MUI_ABORTWARNING

!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "$(RUN_TEXT)"
!define MUI_FINISHPAGE_RUN_FUNCTION LaunchUnelevated

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
; Offers the restart when restoring the keyboard settings needs one (SetRebootFlag).
!insertmacro MUI_UNPAGE_FINISH

; The first language is the fallback for other Windows UI languages (English, like the GUI);
; NSIS picks Japanese on a Japanese UI.
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "Japanese"

LangString RUN_TEXT ${LANG_JAPANESE} "MKLM を起動する"
LangString RUN_TEXT ${LANG_ENGLISH} "Start MKLM"
LangString OLD_WINDOWS ${LANG_JAPANESE} "MKLM には Windows 11 24H2（ビルド ${MIN_BUILD}）以降が必要です。この PC のビルドは $0 です。"
LangString OLD_WINDOWS ${LANG_ENGLISH} "MKLM needs Windows 11 24H2 (build ${MIN_BUILD}) or later. This PC is build $0."
LangString WRONG_ARCH_ARM64 ${LANG_JAPANESE} "このインストーラーは ARM64 版の Windows 用です。x64 版のインストーラー（MKLM-Setup-${VERSION}-x64.exe）を使ってください。"
LangString WRONG_ARCH_ARM64 ${LANG_ENGLISH} "This installer is for ARM64 Windows. Use the x64 installer (MKLM-Setup-${VERSION}-x64.exe)."
LangString WRONG_ARCH_32 ${LANG_JAPANESE} "MKLM は 64 ビット版の Windows でだけ動きます。"
LangString WRONG_ARCH_32 ${LANG_ENGLISH} "MKLM runs on 64-bit Windows only."
LangString HELPER_BUSY ${LANG_JAPANESE} "MKLM がキーボードの設定を変更している最中です。変更が終わってから、もう一度実行してください。"
LangString HELPER_BUSY ${LANG_ENGLISH} "MKLM is changing keyboard settings right now. Run this again when it has finished."
LangString CLI_BUSY ${LANG_JAPANESE} "mklm-cli が実行中です。コマンドが終わってから、もう一度実行してください。"
LangString CLI_BUSY ${LANG_ENGLISH} "mklm-cli is running. Run this again when the command has finished."
LangString NOT_INSTALLED_HERE ${LANG_JAPANESE} "MKLM が想定の場所（$PROGRAMFILES64\SHIN DATA CENTER\MKLM）にありません: $INSTDIR"
LangString NOT_INSTALLED_HERE ${LANG_ENGLISH} "MKLM is not in its expected folder ($PROGRAMFILES64\SHIN DATA CENTER\MKLM): $INSTDIR"
LangString RESTORE_BUSY ${LANG_JAPANESE} "別の MKLM がキーボードの設定を変更中だったため、元に戻せませんでした。MKLM の削除は続けます。元に戻す方法は %ProgramData%\SHIN DATA CENTER\MKLM\Recovery にある README.txt を見てください。"
LangString RESTORE_BUSY ${LANG_ENGLISH} "Another MKLM was changing keyboard settings, so they could not be put back. Removing MKLM continues. See README.txt in %ProgramData%\SHIN DATA CENTER\MKLM\Recovery for how to put them back."
LangString CLOSE_MKLM ${LANG_JAPANESE} "MKLM が実行中です。MKLM を終了してから［再試行］を押してください（タスク バーの「^」の中にある MKLM のアイコンを右クリックして「終了」）。"
LangString CLOSE_MKLM ${LANG_ENGLISH} "MKLM is running. Quit MKLM, then choose Retry (right-click the MKLM icon in the taskbar corner and choose Quit)."
LangString ASK_RESTORE ${LANG_JAPANESE} "キーボードの設定を、MKLM を使い始める前の状態に戻しますか？$\r$\n$\r$\n［はい］MKLM が変更した値を元に戻します（外部で変更された値はそのままにします）。$\r$\n［いいえ］今の設定のまま、MKLM だけを削除します。"
LangString ASK_RESTORE ${LANG_ENGLISH} "Put the keyboard settings back to how they were before MKLM?$\r$\n$\r$\nYes: undo the values MKLM changed (values changed outside MKLM are left alone).$\r$\nNo: keep the current settings and remove only MKLM."
LangString RESTORE_FAILED ${LANG_JAPANESE} "キーボードの設定を元に戻せませんでした（終了コード $1）。MKLM の削除は続けます。元に戻す方法は %ProgramData%\SHIN DATA CENTER\MKLM\Recovery にある README.txt を見てください。"
LangString RESTORE_FAILED ${LANG_ENGLISH} "The keyboard settings could not be put back (exit code $1). Removing MKLM continues. See README.txt in %ProgramData%\SHIN DATA CENTER\MKLM\Recovery for how to put them back."

; --- shared helpers (installer and uninstaller) --------------------------------------------------

; Leaves 1 in $R9 when the installed file whose path is on the stack is running (a running image
; cannot be opened for writing), else 0. Only the installed copies matter: they are what gets
; replaced or removed; a copy of MKLM elsewhere is not touched.
!macro IS_LOCKED_FN prefix
Function ${prefix}IsLocked
  Exch $R8
  Push $R7
  StrCpy $R9 0
  ${If} ${FileExists} "$R8"
    ClearErrors
    FileOpen $R7 "$R8" a
    ${If} ${Errors}
      StrCpy $R9 1
    ${Else}
      FileClose $R7
    ${EndIf}
  ${EndIf}
  Pop $R7
  Pop $R8
FunctionEnd
!macroend
!insertmacro IS_LOCKED_FN ""
!insertmacro IS_LOCKED_FN "un."

; Refuses while the installed helper runs; asks a running GUI to quit (single-instance "quit") and,
; if it is still there, lets the user close it (Retry) or give up (Cancel -> Quit).
!macro CLOSE_MKLM_FN prefix
Function ${prefix}CloseMklm
  Push "$INSTDIR\mklm-helper.exe"
  Call ${prefix}IsLocked
  ${If} $R9 == 1
    MessageBox MB_OK|MB_ICONSTOP "$(HELPER_BUSY)" /SD IDOK
    Quit
  ${EndIf}
  Push "$INSTDIR\mklm-cli.exe"
  Call ${prefix}IsLocked
  ${If} $R9 == 1
    MessageBox MB_OK|MB_ICONSTOP "$(CLI_BUSY)" /SD IDOK
    Quit
  ${EndIf}
  Push "$INSTDIR\mklm.exe"
  Call ${prefix}IsLocked
  ${If} $R9 == 1
    ExecWait '"$INSTDIR\mklm.exe" --quit'
    ; Up to 5 s for the GUI to finish quitting.
    StrCpy $R5 0
    ${Do}
      Sleep 500
      IntOp $R5 $R5 + 1
      Push "$INSTDIR\mklm.exe"
      Call ${prefix}IsLocked
      ${If} $R9 == 0
        ${ExitDo}
      ${EndIf}
    ${LoopUntil} $R5 >= 10
  ${EndIf}
  ${Do}
    Push "$INSTDIR\mklm.exe"
    Call ${prefix}IsLocked
    ${If} $R9 == 0
      ${ExitDo}
    ${EndIf}
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(CLOSE_MKLM)" /SD IDCANCEL IDRETRY retry
    Quit
    retry:
  ${Loop}
FunctionEnd
!macroend
!insertmacro CLOSE_MKLM_FN ""
!insertmacro CLOSE_MKLM_FN "un."

; --- installer ------------------------------------------------------------------------------------

Function .onInit
  SetRegView 64
  ; Always Program Files (admin-only writable): /D= (and winget --location) must not move the
  ; elevated helper, or the tools the uninstaller runs elevated, to a folder users can write.
  StrCpy $INSTDIR "$PROGRAMFILES64\SHIN DATA CENTER\MKLM"
  !if "${ARCH}" == "arm64"
    ${IfNot} ${IsNativeARM64}
      MessageBox MB_OK|MB_ICONSTOP "$(WRONG_ARCH_ARM64)" /SD IDOK
      Abort
    ${EndIf}
  !else
    ${IfNot} ${IsNativeAMD64}
    ${AndIfNot} ${IsNativeARM64}
      MessageBox MB_OK|MB_ICONSTOP "$(WRONG_ARCH_32)" /SD IDOK
      Abort
    ${EndIf}
  !endif
  ReadRegStr $0 HKLM "SOFTWARE\Microsoft\Windows NT\CurrentVersion" "CurrentBuildNumber"
  ${If} $0 < ${MIN_BUILD}
    MessageBox MB_OK|MB_ICONSTOP "$(OLD_WINDOWS)" /SD IDOK
    Abort
  ${EndIf}
  Call CloseMklm
FunctionEnd

Section "MKLM" SecMain
  SectionIn RO
  ; Again right before the files are replaced: MKLM may have been started while the welcome page
  ; waited.
  Call CloseMklm
  SetShellVarContext all
  SetOutPath "$INSTDIR"
  File "${SRCDIR}\mklm.exe"
  File "${SRCDIR}\mklm-cli.exe"
  File "${SRCDIR}\mklm-helper.exe"
  File /oname=LICENSE.txt "${ROOT}\LICENSE"
  File /oname=recovery.md "${ROOT}\docs\recovery.md"
  File /oname=install-guide.md "${ROOT}\docs\install-guide.ja.md"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateShortcut "$SMPROGRAMS\${PRODUCT}.lnk" "$INSTDIR\mklm.exe" "" "$INSTDIR\mklm.exe" 0

  WriteRegStr HKLM "${ARP_KEY}" "DisplayName" "${PRODUCT}"
  WriteRegStr HKLM "${ARP_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${ARP_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKLM "${ARP_KEY}" "DisplayIcon" '"$INSTDIR\mklm.exe",0'
  WriteRegStr HKLM "${ARP_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${ARP_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "${ARP_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegStr HKLM "${ARP_KEY}" "URLInfoAbout" "${REPO_URL}"
  WriteRegStr HKLM "${ARP_KEY}" "HelpLink" "${REPO_URL}/blob/main/docs/install-guide.ja.md"
  WriteRegStr HKLM "${ARP_KEY}" "Comments" "${ARCH}"
  WriteRegDWORD HKLM "${ARP_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${ARP_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKLM "${ARP_KEY}" "EstimatedSize" "$0"
SectionEnd

; The installer runs elevated; starting the GUI directly would make it elevated too (and an
; elevated GUI never writes the per-user sign-in start value, design m3 F.3). Explorer starts it
; as the signed-in user instead.
Function LaunchUnelevated
  Exec '"$WINDIR\explorer.exe" "$INSTDIR\mklm.exe"'
FunctionEnd

; --- uninstaller ----------------------------------------------------------------------------------

Function un.onInit
  SetRegView 64
  ; The uninstaller runs the tools in $INSTDIR elevated: only the Program Files copy is trusted.
  ${If} "$INSTDIR" != "$PROGRAMFILES64\SHIN DATA CENTER\MKLM"
    MessageBox MB_OK|MB_ICONSTOP "$(NOT_INSTALLED_HERE)" /SD IDOK
    Quit
  ${EndIf}
  Call un.CloseMklm
FunctionEnd

Section "Uninstall"
  ; Again: MKLM may have been started while the confirmation page waited.
  Call un.CloseMklm
  SetShellVarContext all

  ; "Restore on uninstall" (HKLM machine setting, written only by the helper): absent means on.
  StrCpy $2 1
  ClearErrors
  ReadRegDWORD $0 HKLM "${SETTINGS_KEY}" "RestoreOnUninstall"
  ${IfNot} ${Errors}
  ${AndIf} $0 == 0
    StrCpy $2 0
  ${EndIf}
  ${IfNot} ${Silent}
    ${If} $2 == 1
      MessageBox MB_YESNO|MB_ICONQUESTION "$(ASK_RESTORE)" IDYES +2
      StrCpy $2 0
    ${Else}
      MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "$(ASK_RESTORE)" IDNO +2
      StrCpy $2 1
    ${EndIf}
  ${EndIf}

  ; The helper's second fixed command line (design m2 C.12, C14): a silent restore of every value
  ; to its baseline, conflicts skipped, no keyboard reset, no question. The uninstaller is elevated,
  ; so it starts without UAC. Exit 0 / 3010 (restart needed) / 6 (busy) / other (failed).
  ${If} $2 == 1
  ${AndIf} ${FileExists} "$INSTDIR\mklm-helper.exe"
    DetailPrint "mklm-helper --uninstall-restore"
    ClearErrors
    ExecWait '"$INSTDIR\mklm-helper.exe" --uninstall-restore' $1
    ${If} ${Errors}
      StrCpy $1 "error"
    ${EndIf}
    DetailPrint "exit code: $1"
    ${If} $1 == 3010
      ; The finish page offers the restart; a silent uninstall reports it to its caller.
      SetRebootFlag true
      SetErrorLevel 3010
    ${ElseIf} $1 == 6
      MessageBox MB_OK|MB_ICONEXCLAMATION "$(RESTORE_BUSY)" /SD IDOK
    ${ElseIf} $1 != 0
      MessageBox MB_OK|MB_ICONEXCLAMATION "$(RESTORE_FAILED)" /SD IDOK
    ${EndIf}
  ${EndIf}

  Delete "$SMPROGRAMS\${PRODUCT}.lnk"
  ; /REBOOTOK: a helper that has not quite exited yet is removed at the next restart instead of
  ; being left behind.
  Delete /REBOOTOK "$INSTDIR\mklm.exe"
  Delete /REBOOTOK "$INSTDIR\mklm-cli.exe"
  Delete /REBOOTOK "$INSTDIR\mklm-helper.exe"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\recovery.md"
  Delete "$INSTDIR\install-guide.md"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  RMDir "$PROGRAMFILES64\SHIN DATA CENTER"

  DeleteRegKey HKLM "${ARP_KEY}"
  ; The sign-in start value of the account running the uninstaller (the GUI writes it per user).
  ; It would point at the removed mklm.exe; other accounts' values stay and do nothing.
  DeleteRegValue HKCU "${RUN_KEY}" "${RUN_VALUE}"
  ; The journal, baselines and Recovery files stay: they describe what MKLM changed.
SectionEnd
