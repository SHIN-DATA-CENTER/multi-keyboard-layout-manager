# The rules of installer\check-nsi.ps1 (design m5b D.9.3, F.2) against deliberately broken script
# fragments, and the real installer\nsis\mklm.nsi.
#
# Written for the Pester 3.4 that ships with Windows PowerShell 5.1 and for Pester 5: only
# Describe / It and plain `throw` (no Should syntax, which differs between them), and the script
# under test is dot-sourced inside each It (Pester 5 runs It blocks apart from the file's top level).
# CI runs these under GitHub's powershell wrapper: see the two rules at the top of
# smoke-test.Tests.ps1 (design standard-layout D.4).

Describe 'check-nsi.ps1' {

    It 'accepts the real installer script' {
        . (Join-Path $PSScriptRoot '..\check-nsi.ps1')
        $text = [System.IO.File]::ReadAllText((Join-Path $PSScriptRoot '..\nsis\mklm.nsi'))
        $found = Test-NsiScript -Text $text
        if ($found.Count -ne 0) {
            throw ('mklm.nsi: ' + (($found | ForEach-Object { "line $($_.Line) rule $($_.Rule): $($_.Message)" }) -join '; '))
        }
    }

    It 'accepts a small correct script' {
        . (Join-Path $PSScriptRoot '..\check-nsi.ps1')
        $text = @'
Function .onInit
  MessageBox MB_OK "$(TEXT)" /SD IDOK ; a comment with Quit and taskkill in it
  SetErrorLevel ${MKLM_EXIT_OS_TOO_OLD}
  Quit
FunctionEnd
Section "MKLM" SecMain
  AllowSkipFiles on
  ClearErrors
  File "/oname=$INSTDIR\mklm.exe.new" "${SRCDIR}\mklm.exe"
  AllowSkipFiles off
SectionEnd
Section "Uninstall"
  ExecWait '"$INSTDIR\mklm-helper.exe" --uninstall-restore' $1
  SetErrorLevel 3010
  Delete /REBOOTOK "$INSTDIR\mklm.exe"
SectionEnd
'@
        $found = Test-NsiScript -Text $text
        if ($found.Count -ne 0) { throw ($found | Out-String) }
    }

    It 'finds each broken rule' {
        . (Join-Path $PSScriptRoot '..\check-nsi.ps1')
        $cases = @(
            @{ Rule = 1; Text = "Function f`n  MessageBox MB_OK `"x`"`nFunctionEnd" },
            @{ Rule = 1; Text = "Function f`n  MessageBox MB_YESNO `"x`" IDYES +2`nFunctionEnd" },
            @{ Rule = 2; Text = "Function f`n  MessageBox MB_OK `"x`" /SD IDOK`n  Quit`nFunctionEnd" },
            @{ Rule = 2; Text = "Function .onInit`n  MessageBox MB_OK `"x`" /SD IDOK`n  Abort`nFunctionEnd" },
            @{ Rule = 2; Text = "Function un.onInit`n  Abort`nFunctionEnd" },
            @{ Rule = 3; Text = "Function f`n  SetErrorLevel 21`n  MessageBox MB_OK `"x`" /SD IDOK`n  Quit`nFunctionEnd" },
            @{ Rule = 3; Text = "Section `"MKLM`"`n  SetErrorLevel 3010`nSectionEnd" },
            @{ Rule = 4; Text = "Section `"MKLM`"`n  ExecWait '`"`$INSTDIR\uninstall.exe`" /S'`nSectionEnd" },
            @{ Rule = 4; Text = "Function .onInit`n  nsExec::Exec '`"`$INSTDIR\mklm-helper.exe`" --uninstall-restore'`nFunctionEnd" },
            @{ Rule = 5; Text = "Section `"MKLM`"`n  File `"`${SRCDIR}\mklm.exe`"`nSectionEnd" },
            @{ Rule = 5; Text = "Section `"MKLM`"`n  File /oname=mklm-helper.exe `"`${SRCDIR}\mklm-helper.exe`"`nSectionEnd" },
            @{ Rule = 5; Text = "Section `"MKLM`"`n  File `"/oname=`$INSTDIR\mklm-cli.exe.tmp`" `"`${SRCDIR}\mklm-cli.exe`"`nSectionEnd" },
            @{ Rule = 5; Text = "Section `"MKLM`"`n  AllowSkipFiles on`n  File /oname=LICENSE.txt `"`${ROOT}\LICENSE`"`n  AllowSkipFiles off`nSectionEnd" },
            @{ Rule = 5; Text = "Section `"MKLM`"`n  AllowSkipFiles on`n  File `"/oname=`$INSTDIR\mklm.exe.new`" `"`${SRCDIR}\mklm.exe`"`nSectionEnd" },
            @{ Rule = 5; Text = "Section `"MKLM`"`n  Delete /REBOOTOK `"`$INSTDIR\mklm.exe.old`"`nSectionEnd" },
            @{ Rule = 6; Text = "Function f`n  nsProcess::_FindProcess `"mklm.exe`"`nFunctionEnd" },
            @{ Rule = 6; Text = "Function f`n  ExecWait 'taskkill /IM mklm.exe'`nFunctionEnd" },
            @{ Rule = 6; Text = "Function f`n  FindProcDLL::FindProc `"mklm.exe`"`nFunctionEnd" },
            @{ Rule = 6; Text = "Function f`n  Processes::FindProcess `"mklm`"`nFunctionEnd" }
        )
        foreach ($case in $cases) {
            $found = Test-NsiScript -Text $case.Text
            $rules = @($found | ForEach-Object { $_.Rule })
            if ($rules -notcontains $case.Rule) {
                throw "rule $($case.Rule) not found in: $($case.Text) (found: $($rules -join ','))"
            }
        }
    }

    It 'reports the line of a violation' {
        . (Join-Path $PSScriptRoot '..\check-nsi.ps1')
        $text = "; header`r`n`r`nFunction f`r`n  Quit`r`nFunctionEnd`r`n"
        $found = Test-NsiScript -Text $text
        if ($found.Count -ne 1 -or $found[0].Line -ne 4 -or $found[0].Rule -ne 2) {
            throw ($found | Out-String)
        }
    }

    It 'ignores comments and block comments' {
        . (Join-Path $PSScriptRoot '..\check-nsi.ps1')
        $text = @'
; Quit
# MessageBox MB_OK "x"
/* SetErrorLevel 3
   taskkill */
Function f
  DetailPrint "a ; not a comment inside quotes" ; Abort
FunctionEnd
'@
        $found = Test-NsiScript -Text $text
        if ($found.Count -ne 0) { throw ($found | Out-String) }
    }

    It 'fails the old in-place installer script' {
        . (Join-Path $PSScriptRoot '..\check-nsi.ps1')
        $old = @'
Function .onInit
  MessageBox MB_OK|MB_ICONSTOP "$(OLD_WINDOWS)" /SD IDOK
  Abort
FunctionEnd
Section "MKLM" SecMain
  File "${SRCDIR}\mklm.exe"
  File "${SRCDIR}\mklm-cli.exe"
  File "${SRCDIR}\mklm-helper.exe"
SectionEnd
'@
        $found = Test-NsiScript -Text $old
        $rules = @($found | ForEach-Object { $_.Rule })
        if (($rules | Where-Object { $_ -eq 5 }).Count -ne 3 -or $rules -notcontains 2) {
            throw "found rules: $($rules -join ',')"
        }
    }
}
