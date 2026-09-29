# The read-only parts of installer\smoke-test.ps1. The smoke test itself installs MKLM and runs on
# CI machines only (design m5b D.9.3); dot-sourcing it runs nothing but its definitions.
# Pester 3.4 and 5: Describe / It and plain `throw` only.
#
# Two rules for every test file here (design standard-layout D.4), because CI runs Pester under
# GitHub's `shell: powershell` wrapper ($ErrorActionPreference = 'stop' before the step, and
# `exit $LASTEXITCODE` after it):
# 1. A test that runs a native command resets `$global:LASTEXITCODE = 0` in a `finally`, so that
#    a deliberate failure it provoked does not become the step's exit code.
# 2. Never redirect a native command's stderr with `2>&1` under the inherited 'stop': Windows
#    PowerShell 5.1 turns every stderr line into a terminating NativeCommandError. Set
#    `$ErrorActionPreference = 'Continue'` inside that `It` first, or avoid the child process
#    (as the refusal test below does).

Describe 'smoke-test.ps1' {

    It 'refuses to run without -OnThrowawayMachine' {
        . (Join-Path $PSScriptRoot '..\smoke-test.ps1')
        $refused = $null
        try {
            Assert-OnThrowawayMachine -Confirmed $false
        }
        catch {
            $refused = $_.Exception.Message
        }
        if (-not $refused) { throw 'it did not refuse' }
        if ($refused -notmatch 'throwaway') { throw "unexpected refusal: $refused" }
        # Confirmed: no refusal.
        Assert-OnThrowawayMachine -Confirmed $true
    }

    It 'refuses before anything else when run as a script' {
        $path = Join-Path $PSScriptRoot '..\smoke-test.ps1'
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile((Resolve-Path $path).Path, [ref]$tokens, [ref]$errors)
        if ($errors.Count -ne 0) { throw "smoke-test.ps1 does not parse: $($errors[0])" }
        $body = $ast.EndBlock.Statements | Where-Object {
            $_ -is [System.Management.Automation.Language.IfStatementAst] -and
            $_.Clauses[0].Item1.Extent.Text -eq '$MyInvocation.InvocationName -ne ''.'''
        } | Select-Object -First 1
        if (-not $body) { throw 'the script has no body guarded by the dot-source check' }
        $first = $body.Clauses[0].Item2.Statements[0]
        if ($first.Extent.Text -notmatch '^Assert-OnThrowawayMachine\b') {
            throw "the body starts with: $($first.Extent.Text)"
        }
    }

    It 'reads no build ID from files without one' {
        . (Join-Path $PSScriptRoot '..\smoke-test.ps1')
        $kernel32 = Join-Path ([Environment]::SystemDirectory) 'kernel32.dll'
        if ($null -ne (Get-MklmBuildId -Path $kernel32)) { throw 'kernel32.dll has an MKLM build ID?' }
        if ($null -ne (Get-MklmBuildId -Path 'C:\nowhere\mklm.exe')) { throw 'a missing file has one?' }
    }

    It 'reads the build ID of a built MKLM executable when one is there' {
        . (Join-Path $PSScriptRoot '..\smoke-test.ps1')
        $root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
        $built = @(
            'target\debug\mklm-cli.exe',
            'target\release\mklm-cli.exe',
            'target\x86_64-pc-windows-msvc\release\mklm-cli.exe'
        ) | ForEach-Object { Join-Path $root $_ } | Where-Object { Test-Path $_ } | Select-Object -First 1
        if ($built) {
            $id = Get-MklmBuildId -Path $built
            if ($id -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?\+[0-9a-f]+$') { throw "build ID of ${built}: $id" }
        }
    }

    It 'tells a replaced file from a kept one only against another release (MECHANICS-3)' {
        . (Join-Path $PSScriptRoot '..\smoke-test.ps1')
        $previous = @('0.1.0+aaaa', '0.1.0+aaaa', '0.1.0+aaaa')
        $new = @('0.2.1+bbbb', '0.2.1+bbbb', '0.2.1+bbbb')
        if (-not (Test-InStep $previous)) { throw 'the previous release is in step' }
        if (Test-InStep @('0.1.0+aaaa', $null, '0.1.0+aaaa')) { throw 'a missing ID is not in step' }
        if (Test-InStep @('0.1.0+aaaa', '0.1.0+aaaa')) { throw 'two IDs are not in step' }
        if (Test-InStep $null) { throw 'nothing is not in step' }
        if (-not (Test-DistinctRelease -Previous $previous -New $new)) { throw 'two releases differ' }
        if (Test-DistinctRelease -Previous $new -New $new) { throw 'the same build cannot tell anything apart' }
        # Refused, nothing replaced.
        if (-not (Test-NothingReplaced -Ids $previous -Kept $previous)) { throw 'kept' }
        # A rollback that deleted the .old copies instead of renaming them back leaves the new
        # helper and CLI next to the previous GUI (the swap order is helper, CLI, GUI).
        $mixed = @('0.1.0+aaaa', '0.2.1+bbbb', '0.2.1+bbbb')
        if (Test-NothingReplaced -Ids $mixed -Kept $previous) { throw 'a half-replaced install passed' }
        # Everything replaced, or one file missing.
        if (Test-NothingReplaced -Ids $new -Kept $previous) { throw 'a replaced install passed' }
        if (Test-NothingReplaced -Ids @('0.1.0+aaaa', $null, '0.1.0+aaaa') -Kept $previous) { throw 'a missing file passed' }
    }

    It 'runs the held-file steps as upgrades from the previous release (MECHANICS-3)' {
        $text = [System.IO.File]::ReadAllText((Join-Path $PSScriptRoot '..\smoke-test.ps1'))
        $restore = $text.IndexOf('$code = Invoke-Setup -Path $PreviousInstaller')
        $step3 = $text.IndexOf('# 3. mklm-cli.exe held')
        if ($restore -lt 0 -or $step3 -lt 0 -or $restore -gt $step3) { throw 'the previous release does not go back before step 3' }
        $checks = [regex]::Matches($text, [regex]::Escape('Test-NothingReplaced -Ids $ids -Kept $previous')).Count
        if ($checks -ne 3) { throw "steps 3-5 compare with the previous release $checks time(s), not 3" }
        $step6 = $text.IndexOf('# 6. The ARM64 installer')
        $upgrade = $text.LastIndexOf('$code = Invoke-Setup -Path $Installer', $step6)
        if ($upgrade -lt $text.IndexOf('# 5. mklm.exe shared')) { throw 'the new build is not installed again before step 7' }
    }

    It 'gives the installer the update runner''s environment only' {
        . (Join-Path $PSScriptRoot '..\smoke-test.ps1')
        $environment = Get-RunnerEnvironment -TempDir 'C:\run\tmp'
        $names = @($environment.Keys)
        $expected = @('ComSpec', 'PATH', 'ProgramData', 'ProgramFiles', 'ProgramW6432', 'SystemDrive', 'SystemRoot', 'TEMP', 'TMP', 'windir')
        if (($names -join ',') -ne ($expected -join ',')) { throw ($names -join ',') }
        if ($environment.TEMP -ne 'C:\run\tmp' -or $environment.TMP -ne 'C:\run\tmp') { throw 'TEMP' }
        if ($environment.PATH -notmatch '\\Wbem$') { throw $environment.PATH }
    }
}
