# The read-only parts of installer\smoke-test.ps1. The smoke test itself installs MKLM and runs on
# CI machines only (design m5b D.9.3); dot-sourcing it runs nothing but its definitions.
# Pester 3.4 and 5: Describe / It and plain `throw` only.

Describe 'smoke-test.ps1' {

    It 'refuses to run without -OnThrowawayMachine' {
        $script = Join-Path $PSScriptRoot '..\smoke-test.ps1'
        $output = & powershell -NoProfile -ExecutionPolicy Bypass -File $script -Installer 'C:\nowhere\MKLM-Setup-0.2.1-x64.exe' 2>&1 | Out-String
        if ($LASTEXITCODE -eq 0) { throw "it ran: $output" }
        if ($output -notmatch 'throwaway') { throw "unexpected refusal: $output" }
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
