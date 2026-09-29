# installer\build-installer.ps1: the release build lands where the script packs and checks the
# exes from (target\<triple>\release), whatever CARGO_TARGET_DIR the caller has (CLEAN-RUN-2).
# A fake cargo records the target folder it was given and fails, so nothing is built or packed.
# Pester 3.4 and 5: Describe / It and plain `throw` only.
# CI runs these under GitHub's powershell wrapper: see the two rules at the top of
# smoke-test.Tests.ps1 (design standard-layout D.4).

Describe 'build-installer.ps1' {

    It 'builds a release into the repository target folder and restores CARGO_TARGET_DIR' {
        $script = Join-Path $PSScriptRoot '..\build-installer.ps1'
        $root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
        $scratch = Join-Path ([IO.Path]::GetTempPath()) ('mklm-build-' + [guid]::NewGuid().ToString('n'))
        $bin = Join-Path $scratch 'bin'
        New-Item -ItemType Directory -Force $bin | Out-Null
        $recorded = Join-Path $scratch 'cargo-target-dir.txt'
        $saved = @{ PATH = $env:PATH; CARGO_TARGET_DIR = $env:CARGO_TARGET_DIR; OUT = $env:MKLM_TEST_CARGO_OUT }
        try {
            [IO.File]::WriteAllText((Join-Path $bin 'cargo.cmd'),
                "@echo off`r`n>`"%MKLM_TEST_CARGO_OUT%`" echo %CARGO_TARGET_DIR%`r`nexit /b 3`r`n")
            $makensis = Join-Path $bin 'makensis.cmd'
            [IO.File]::WriteAllText($makensis, "@echo v3.12`r`n")
            $elsewhere = Join-Path $scratch 'elsewhere'
            $env:PATH = "$bin;$($saved.PATH)"
            $env:CARGO_TARGET_DIR = $elsewhere
            $env:MKLM_TEST_CARGO_OUT = $recorded

            $failed = $null
            try {
                & $script -Arch x64 -Makensis $makensis | Out-Null
            }
            catch {
                $failed = $_.Exception.Message
            }
            if ($failed -notmatch 'cargo build failed \(3\)') { throw "expected the fake cargo's failure, got: $failed" }
            if (-not (Test-Path $recorded)) { throw 'the fake cargo did not run' }
            $given = ([IO.File]::ReadAllText($recorded)).Trim()
            $expected = Join-Path $root 'target'
            if ($given -ne $expected) { throw "cargo built into '$given', expected '$expected'" }
            if ($env:CARGO_TARGET_DIR -ne $elsewhere) { throw "CARGO_TARGET_DIR was not restored: '$env:CARGO_TARGET_DIR'" }
        }
        finally {
            $env:PATH = $saved.PATH
            $env:CARGO_TARGET_DIR = $saved.CARGO_TARGET_DIR
            $env:MKLM_TEST_CARGO_OUT = $saved.OUT
            $global:LASTEXITCODE = 0
            Remove-Item -Recurse -Force $scratch -ErrorAction SilentlyContinue
        }
    }
}
