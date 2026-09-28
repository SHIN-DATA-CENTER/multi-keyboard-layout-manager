<#
.SYNOPSIS
  Static checks of installer\nsis\mklm.nsi (design m5b D.9.3): the rules that keep a silent
  (/S) install from stopping at a dialog, give every refusal a fixed exit code, and keep the
  two-stage replacement of the executables intact.

.DESCRIPTION
  Rules (each violation names its rule and line):
  1. Every MessageBox has /SD (a silent run never waits for a click).
  2. Every Quit directly follows a SetErrorLevel. .onInit and un.onInit have no Abort.
  3. SetErrorLevel only appears directly before a Quit (except the uninstaller's 3010, which
     reports "restart needed" on the success path).
  4. Lines that run uninstall.exe or "--uninstall-restore" are only in un. functions and the
     uninstall section (a silent upgrade never runs them, D.9.5).
  5. The three executables are only written as File "/oname=...\<name>.new"; AllowSkipFiles on
     appears only right before those File lines (ClearErrors allowed) and is switched off right
     after them; the install sections have no /REBOOTOK.
  6. No process-name lookups (FindProcDLL, nsProcess, Processes::, KillProc, tasklist,
     taskkill): a running MKLM is found by its installed path only (RELIABILITY-12).

  Dot-source the script to get Test-NsiScript without running it (installer\tests).

.EXAMPLE
  .\installer\check-nsi.ps1
  .\installer\check-nsi.ps1 -Path some\other.nsi
#>
[CmdletBinding()]
param(
    [string]$Path
)

Set-StrictMode -Version 2.0

$script:MklmExecutables = @('mklm.exe', 'mklm-cli.exe', 'mklm-helper.exe')
$script:ExecInstructions = @('Exec', 'ExecWait', 'ExecShell', 'ExecShellWait', 'ExecDos::exec', 'nsExec::Exec', 'nsExec::ExecToLog', 'nsExec::ExecToStack')
$script:ProcessNameLookups = @('FindProcDLL', 'nsProcess', 'Processes::', 'KillProc', 'tasklist', 'taskkill')

# The code part of one line: without a trailing ; or # comment (outside quotes). NSIS treats ; and
# # as comments at the start of a line or after whitespace.
function Remove-NsiComment {
    param([string]$Line)
    $quote = [char]0
    $previous = ' '
    for ($i = 0; $i -lt $Line.Length; $i++) {
        $c = $Line[$i]
        if ($quote -ne [char]0) {
            if ($c -eq $quote) { $quote = [char]0 }
        }
        elseif ($c -eq '"' -or $c -eq "'" -or $c -eq '`') {
            $quote = $c
        }
        elseif (($c -eq ';' -or $c -eq '#') -and [char]::IsWhiteSpace($previous)) {
            return $Line.Substring(0, $i)
        }
        $previous = $c
    }
    return $Line
}

# The logical lines of a script: number, code (trimmed, without comments; block comments removed),
# and the function or section they are in.
function Get-NsiLines {
    param([string]$Text)
    $lines = New-Object System.Collections.Generic.List[object]
    $inBlockComment = $false
    $scope = ''
    $scopeKind = ''
    $number = 0
    foreach ($raw in ($Text -split "`r?`n")) {
        $number++
        $line = $raw
        if ($inBlockComment) {
            $end = $line.IndexOf('*/')
            if ($end -lt 0) { continue }
            $line = $line.Substring($end + 2)
            $inBlockComment = $false
        }
        while ($true) {
            $start = $line.IndexOf('/*')
            if ($start -lt 0) { break }
            $end = $line.IndexOf('*/', $start + 2)
            if ($end -lt 0) {
                $line = $line.Substring(0, $start)
                $inBlockComment = $true
                break
            }
            $line = $line.Substring(0, $start) + ' ' + $line.Substring($end + 2)
        }
        $code = (Remove-NsiComment $line).Trim()
        if ($code.Length -eq 0) { continue }
        $first = ($code -split '\s+', 2)[0]
        if ($first -ieq 'Function') {
            $scope = (($code -split '\s+', 2)[1]).Trim('"')
            $scopeKind = 'Function'
        }
        elseif ($first -ieq 'Section') {
            $rest = ($code -split '\s+', 2)
            $name = ''
            if ($rest.Count -gt 1) {
                $tokens = [regex]::Matches($rest[1], '"[^"]*"|\S+') | ForEach-Object { $_.Value.Trim('"') }
                $name = ($tokens | Where-Object { $_ -notmatch '^/o$' } | Select-Object -First 1)
            }
            $scope = $name
            $scopeKind = 'Section'
        }
        $lines.Add([pscustomobject]@{
                Number    = $number
                Code      = $code
                Keyword   = $first
                Scope     = $scope
                ScopeKind = $scopeKind
            })
        if ($first -ieq 'FunctionEnd' -or $first -ieq 'SectionEnd') {
            $scope = ''
            $scopeKind = ''
        }
    }
    return , $lines
}

# True for the uninstaller's code: un. functions and the uninstall section.
function Test-UninstallScope {
    param($Line)
    if ($Line.ScopeKind -eq 'Function') { return $Line.Scope -like 'un.*' }
    if ($Line.ScopeKind -eq 'Section') { return $Line.Scope -ieq 'Uninstall' -or $Line.Scope -like 'un.*' }
    return $false
}

function New-Violation {
    param([int]$Rule, $Line, [string]$Message)
    [pscustomobject]@{ Rule = $Rule; Line = $Line.Number; Message = $Message; Code = $Line.Code }
}

# The quoted and unquoted arguments of an instruction.
function Get-NsiTokens {
    param([string]$Code)
    [regex]::Matches($Code, '"[^"]*"|''[^'']*''|`[^`]*`|\S+') | ForEach-Object {
        $value = $_.Value
        if ($value.Length -ge 2 -and ('"', "'", '`' -contains $value[0]) -and $value[-1] -eq $value[0]) {
            $value.Substring(1, $value.Length - 2)
        }
        else { $value }
    }
}

# The file an instruction names, when it is one of the three executables.
function Get-ExecutableName {
    param([string]$Text)
    foreach ($name in $script:MklmExecutables) {
        if ($Text -match ('(^|[\\/"])' + [regex]::Escape($name) + '($|["\s])')) { return $name }
    }
    return $null
}

# Every violation of the rules above in the text of an .nsi script.
function Test-NsiScript {
    param([Parameter(Mandatory)] [string]$Text)
    $lines = Get-NsiLines $Text
    $violations = New-Object System.Collections.Generic.List[object]
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        $keyword = $line.Keyword
        $previous = if ($i -gt 0) { $lines[$i - 1] } else { $null }
        $next = if ($i + 1 -lt $lines.Count) { $lines[$i + 1] } else { $null }

        # Rule 1.
        if ($keyword -ieq 'MessageBox') {
            $tokens = @(Get-NsiTokens $line.Code)
            if (-not ($tokens | Where-Object { $_ -ieq '/SD' })) {
                $violations.Add((New-Violation 1 $line 'MessageBox without /SD'))
            }
        }
        # Rule 2.
        if ($keyword -ieq 'Quit') {
            if ($null -eq $previous -or $previous.Keyword -ine 'SetErrorLevel') {
                $violations.Add((New-Violation 2 $line 'Quit without SetErrorLevel right before it'))
            }
        }
        if ($keyword -ieq 'Abort' -and $line.ScopeKind -eq 'Function' -and ($line.Scope -ieq '.onInit' -or $line.Scope -ieq 'un.onInit')) {
            $violations.Add((New-Violation 2 $line "Abort in $($line.Scope) (use SetErrorLevel and Quit)"))
        }
        # Rule 3.
        if ($keyword -ieq 'SetErrorLevel') {
            $value = (@(Get-NsiTokens $line.Code))[1]
            $restartNeeded = $value -eq '3010' -and (Test-UninstallScope $line)
            if (-not $restartNeeded -and ($null -eq $next -or $next.Keyword -ine 'Quit')) {
                $violations.Add((New-Violation 3 $line 'SetErrorLevel not directly followed by Quit'))
            }
        }
        # Rule 4.
        $runs = $script:ExecInstructions | Where-Object { $keyword -ieq $_ }
        if ($runs -and ($line.Code -match '(?i)uninstall\.exe|--uninstall-restore') -and -not (Test-UninstallScope $line)) {
            $violations.Add((New-Violation 4 $line 'runs the uninstaller or --uninstall-restore outside the uninstaller'))
        }
        # Rule 5a.
        if ($keyword -ieq 'File') {
            $tokens = @(Get-NsiTokens $line.Code | Select-Object -Skip 1)
            $oname = $tokens | Where-Object { $_ -like '/oname=*' } | Select-Object -First 1
            $sources = @($tokens | Where-Object { $_ -notlike '/*' })
            $source = if ($sources.Count -gt 0) { $sources[-1] } else { '' }
            $executable = Get-ExecutableName $source
            if (-not $executable -and $oname) {
                $target = $oname.Substring(7)
                foreach ($name in $script:MklmExecutables) {
                    if ($target -match ('(^|\\)' + [regex]::Escape($name) + '$')) { $executable = $name }
                }
            }
            if ($executable) {
                $wanted = '\' + $executable + '.new'
                if (-not $oname -or -not $oname.EndsWith($wanted, [System.StringComparison]::OrdinalIgnoreCase)) {
                    $violations.Add((New-Violation 5 $line "$executable is written in place (use /oname=...$wanted)"))
                }
            }
        }
        # Rule 5b.
        if ($keyword -ieq 'AllowSkipFiles' -and $line.Code -match '(?i)^AllowSkipFiles\s+on$') {
            $j = $i + 1
            $files = 0
            $ok = $true
            while ($j -lt $lines.Count) {
                $candidate = $lines[$j]
                if ($candidate.Keyword -ieq 'ClearErrors' -and $files -eq 0) { $j++; continue }
                if ($candidate.Keyword -ieq 'File' -and $candidate.Code -match '(?i)/oname=[^"\s]*\.new"?(\s|$)') { $files++; $j++; continue }
                break
            }
            $closing = if ($j -lt $lines.Count) { $lines[$j] } else { $null }
            if ($files -eq 0) { $ok = $false }
            if ($null -eq $closing -or $closing.Code -notmatch '(?i)^AllowSkipFiles\s+off$') { $ok = $false }
            if (-not $ok) {
                $violations.Add((New-Violation 5 $line 'AllowSkipFiles on must come right before the .new File lines and be switched off right after them'))
            }
        }
        # Rule 5c.
        if ($line.Code -match '(?i)/REBOOTOK' -and -not (Test-UninstallScope $line) -and $line.ScopeKind -eq 'Section') {
            $violations.Add((New-Violation 5 $line '/REBOOTOK in an install section'))
        }
        # Rule 6.
        foreach ($lookup in $script:ProcessNameLookups) {
            if ($line.Code.IndexOf($lookup, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
                $violations.Add((New-Violation 6 $line "process-name lookup ($lookup): find MKLM by its installed path"))
            }
        }
    }
    return , $violations
}

if ($MyInvocation.InvocationName -ne '.') {
    $ErrorActionPreference = 'Stop'
    if (-not $Path) { $Path = Join-Path $PSScriptRoot 'nsis\mklm.nsi' }
    $text = [System.IO.File]::ReadAllText((Resolve-Path $Path))
    $found = Test-NsiScript -Text $text
    if ($found.Count -gt 0) {
        foreach ($violation in $found) {
            Write-Host ("{0}({1}): rule {2}: {3}`n    {4}" -f $Path, $violation.Line, $violation.Rule, $violation.Message, $violation.Code)
        }
        Write-Host "check-nsi: $($found.Count) violation(s)"
        exit 1
    }
    Write-Host "check-nsi: $Path passes rules 1-6 (design m5b D.9.3)"
    exit 0
}
