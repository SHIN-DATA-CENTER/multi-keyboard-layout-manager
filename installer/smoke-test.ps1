<#
.SYNOPSIS
  The installer's smoke test (design m5b D.9.3): installs, upgrades, refuses and uninstalls on a
  THROWAWAY machine (a CI runner). Never run it on a machine you use: it installs MKLM into
  Program Files, holds its files open and uninstalls it.

.DESCRIPTION
  Runs as an administrator (CI runners are), with -OnThrowawayMachine, on windows-2025
  (build 26100 or later: the installer refuses older Windows). Steps:

  1. The latest published release's x64 installer (gh release download, or -PreviousInstaller)
     /S -> 0; HKLM\SOFTWARE\SHIN DATA CENTER exported; the new installer /S -> 0; the three
     executables carry the same build ID of the new version; HKLM\SOFTWARE\SHIN DATA CENTER is
     unchanged (a silent upgrade writes nothing there and runs no restore, D.9.5).
  2. The new installer again /S -> 0, with the update runner's minimal environment block only
     (ProcessStartInfo with an emptied environment; D.9.4, I.15).
  Before 3-5 the previous release goes back (/S -> 0), so that 3-5 are real upgrade attempts:
  "nothing replaced" means the three build IDs are still the previous release's (all there and
  the same). Re-running the build that is already installed could not tell a replaced file from a
  kept one (design m5b M.5, MECHANICS-3: a rollback that deleted the .old copies would pass).
  3. mklm-cli.exe held open without sharing -> 23 (MKLM_EXIT_CLI_RUNNING); nothing replaced.
  4. mklm.exe held open for reading, shared for reading and writing but not deleting -> 26
     (MKLM_EXIT_FILES_IN_USE: the rename fails, the swap is rolled back); nothing replaced and no
     .new / .old left.
  5. mklm.exe held open shared for reading only -> 24 (MKLM_EXIT_GUI_RUNNING: the installer's
     running check cannot open it); nothing replaced.
  Then the new installer /S -> 0 again: the upgrade goes through after the refused attempts.
  6. The ARM64 installer /S -> 21 (MKLM_EXIT_WRONG_ARCH) on this x64 machine (-Arm64Installer).
  7. uninstall.exe /S _?=<INSTDIR> -> 0; the executables are gone (the folder is removed here).

  Exit 0 when every step passed, else 1. Dot-source the script to get its functions without
  running anything (installer\tests uses Get-MklmBuildId).

.EXAMPLE
  # CI only (release.yml, installer.yml):
  .\installer\smoke-test.ps1 -OnThrowawayMachine -Installer dist\MKLM-Setup-0.2.1-x64.exe -Arm64Installer dist\MKLM-Setup-0.2.1-arm64.exe
#>
[CmdletBinding()]
param(
    [string]$Installer,
    [string]$Arm64Installer,
    [string]$PreviousInstaller,
    [string]$Repository = 'SHIN-DATA-CENTER/multi-keyboard-layout-manager',
    [switch]$OnThrowawayMachine
)

Set-StrictMode -Version 2.0

$script:VersionInfoSource = @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

public static class MklmVersionInfo {
    [DllImport("version.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern int GetFileVersionInfoSizeW(string file, out int handle);
    [DllImport("version.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool GetFileVersionInfoW(string file, int handle, int length, IntPtr data);
    [DllImport("version.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool VerQueryValueW(IntPtr block, string subBlock, out IntPtr buffer, out int length);

    // The MKLMBuildId string of a file's VERSIONINFO, or null.
    public static string BuildId(string path) {
        int ignored;
        int size = GetFileVersionInfoSizeW(path, out ignored);
        if (size == 0) { return null; }
        IntPtr block = Marshal.AllocHGlobal(size);
        try {
            if (!GetFileVersionInfoW(path, 0, size, block)) { return null; }
            List<string> translations = new List<string>();
            IntPtr pointer;
            int length;
            if (VerQueryValueW(block, "\\VarFileInfo\\Translation", out pointer, out length)) {
                for (int offset = 0; offset + 4 <= length; offset += 4) {
                    int language = (ushort)Marshal.ReadInt16(pointer, offset);
                    int codePage = (ushort)Marshal.ReadInt16(pointer, offset + 2);
                    translations.Add(language.ToString("x4") + codePage.ToString("x4"));
                }
            }
            translations.Add("040904b0");
            foreach (string translation in translations) {
                if (VerQueryValueW(block, "\\StringFileInfo\\" + translation + "\\MKLMBuildId", out pointer, out length) && length > 0) {
                    return Marshal.PtrToStringUni(pointer);
                }
            }
            return null;
        } finally {
            Marshal.FreeHGlobal(block);
        }
    }
}
'@

if (-not ('MklmVersionInfo' -as [type])) {
    Add-Type -TypeDefinition $script:VersionInfoSource -Language CSharp
}

$script:Executables = @('mklm.exe', 'mklm-cli.exe', 'mklm-helper.exe')

# The build ID in a file's VERSIONINFO (MKLMBuildId), or $null.
function Get-MklmBuildId {
    param([Parameter(Mandatory)] [string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $null }
    return [MklmVersionInfo]::BuildId((Resolve-Path -LiteralPath $Path).ProviderPath)
}

# The build IDs of the three installed executables, in the order of $script:Executables.
function Get-InstalledBuildIds {
    param([Parameter(Mandatory)] [string]$InstallDir)
    return @($script:Executables | ForEach-Object { Get-MklmBuildId -Path (Join-Path $InstallDir $_) })
}

# True when the three build IDs are all there and the same: one release in step.
function Test-InStep {
    param([object[]]$Ids)
    if ($null -eq $Ids -or $Ids.Count -ne 3) { return $false }
    foreach ($id in $Ids) {
        if ([string]::IsNullOrEmpty([string]$id) -or [string]$id -ne [string]$Ids[0]) { return $false }
    }
    return $true
}

# True when a refused upgrade replaced nothing (design m5b D.9.3): the three installed build IDs
# are in step and exactly the ones installed before the attempt (`Kept`). Meaningful only when
# `Kept` is not the attempted build's (Test-DistinctRelease).
function Test-NothingReplaced {
    param([object[]]$Ids, [object[]]$Kept)
    if (-not (Test-InStep $Ids) -or -not (Test-InStep $Kept)) { return $false }
    return [string]$Ids[0] -eq [string]$Kept[0]
}

# True when `Previous` is a release in step whose build differs from `New` (in step too), so that
# a replaced file shows as a changed build ID.
function Test-DistinctRelease {
    param([object[]]$Previous, [object[]]$New)
    if (-not (Test-InStep $Previous) -or -not (Test-InStep $New)) { return $false }
    return [string]$Previous[0] -ne [string]$New[0]
}

# The environment the update runner gives the installer (mklm_win::elevation::runner_environment,
# design m5b D.9.4), with `TempDir` as TEMP and TMP.
function Get-RunnerEnvironment {
    param([Parameter(Mandatory)] [string]$TempDir)
    $windows = [Environment]::GetFolderPath('Windows')
    $system32 = [Environment]::SystemDirectory
    return [ordered]@{
        ComSpec      = Join-Path $system32 'cmd.exe'
        PATH         = "$system32;$windows;$system32\Wbem"
        ProgramData  = [Environment]::GetFolderPath('CommonApplicationData')
        ProgramFiles = [Environment]::GetFolderPath('ProgramFiles')
        ProgramW6432 = [Environment]::GetFolderPath('ProgramFiles')
        SystemDrive  = $windows.Substring(0, 2)
        SystemRoot   = $windows
        TEMP         = $TempDir
        TMP          = $TempDir
        windir       = $windows
    }
}

# Runs an installer (or uninstaller) with its arguments and returns its exit code. With
# -Environment, the process gets exactly those variables and System32 as its folder.
function Invoke-Setup {
    param(
        [Parameter(Mandatory)] [string]$Path,
        [string[]]$Arguments = @('/S'),
        [System.Collections.IDictionary]$Environment
    )
    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = $Path
    $info.Arguments = ($Arguments -join ' ')
    $info.UseShellExecute = $false
    $info.WorkingDirectory = [Environment]::SystemDirectory
    if ($Environment) {
        $info.EnvironmentVariables.Clear()
        foreach ($name in $Environment.Keys) { $info.EnvironmentVariables[$name] = $Environment[$name] }
    }
    $process = [System.Diagnostics.Process]::Start($info)
    if (-not $process.WaitForExit(30 * 60 * 1000)) {
        throw "$Path did not end within 30 minutes"
    }
    return $process.ExitCode
}

# HKLM\SOFTWARE\SHIN DATA CENTER as reg export writes it, or '<absent>'.
function Get-VendorKeyExport {
    param([Parameter(Mandatory)] [string]$Scratch)
    $file = Join-Path $Scratch ('vendor-{0}.reg' -f [guid]::NewGuid().ToString('n'))
    & reg.exe export 'HKLM\SOFTWARE\SHIN DATA CENTER' $file /y /reg:64 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $file)) { return '<absent>' }
    return [System.IO.File]::ReadAllText($file)
}

# The refusal that keeps the smoke test off machines people use. The body below calls it first, so
# that installer\tests can check the refusal by dot-sourcing the script (which runs nothing) instead
# of running it as a child process (design standard-layout D.4).
function Assert-OnThrowawayMachine {
    param([bool]$Confirmed)
    if (-not $Confirmed) {
        throw 'This installs, upgrades and uninstalls MKLM. Run it on a throwaway machine (CI) only, with -OnThrowawayMachine.'
    }
}

if ($MyInvocation.InvocationName -ne '.') {
    Assert-OnThrowawayMachine -Confirmed $OnThrowawayMachine.IsPresent
    $ErrorActionPreference = 'Stop'
    $principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'The smoke test runs as an administrator (a CI runner).'
    }
    if (-not $Installer -or -not (Test-Path -LiteralPath $Installer)) { throw 'Pass -Installer (the new x64 installer).' }
    $Installer = (Resolve-Path -LiteralPath $Installer).ProviderPath
    if ((Split-Path -Leaf $Installer) -notmatch '^MKLM-Setup-(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)-x64\.exe$') {
        throw "$Installer is not named MKLM-Setup-<version>-x64.exe"
    }
    $version = $Matches[1]
    $installDir = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'SHIN DATA CENTER\MKLM'
    $scratch = Join-Path ([IO.Path]::GetTempPath()) ('mklm-smoke-' + [guid]::NewGuid().ToString('n'))
    New-Item -ItemType Directory -Force $scratch | Out-Null
    $failures = New-Object System.Collections.Generic.List[string]
    function Test-Step {
        param([string]$Name, [bool]$Passed, [string]$Detail)
        if ($Passed) {
            Write-Host "PASS $Name"
        }
        else {
            Write-Host "FAIL $Name ($Detail)"
            $failures.Add("$Name ($Detail)")
        }
    }
    function Test-NewVersionInstalled {
        param([string]$Name)
        $ids = Get-InstalledBuildIds -InstallDir $installDir
        $same = ($ids.Count -eq 3) -and ($null -ne $ids[0]) -and ($ids[0] -eq $ids[1]) -and ($ids[0] -eq $ids[2])
        Test-Step "${Name}: the three build IDs are the new version's" ($same -and $ids[0].StartsWith("$version+")) ($ids -join ', ')
        return $ids
    }

    # 1. Upgrade from the latest published release.
    if (-not $PreviousInstaller) {
        $previousDir = Join-Path $scratch 'previous'
        New-Item -ItemType Directory -Force $previousDir | Out-Null
        & gh release download --repo $Repository --pattern 'MKLM-Setup-*-x64.exe' --dir $previousDir
        if ($LASTEXITCODE -ne 0) { throw "gh release download failed ($LASTEXITCODE)" }
        $PreviousInstaller = (Get-ChildItem $previousDir -Filter 'MKLM-Setup-*-x64.exe' | Select-Object -First 1).FullName
    }
    Test-Step '1: the previous release installs silently' ((Invoke-Setup -Path $PreviousInstaller) -eq 0) $PreviousInstaller
    $vendorBefore = Get-VendorKeyExport -Scratch $scratch
    $code = Invoke-Setup -Path $Installer
    Test-Step '1: the new installer upgrades silently' ($code -eq 0) "exit $code"
    $installed = Test-NewVersionInstalled '1'
    $vendorAfter = Get-VendorKeyExport -Scratch $scratch
    Test-Step '1: HKLM\SOFTWARE\SHIN DATA CENTER is unchanged' ($vendorBefore -eq $vendorAfter) 'the export differs'

    # 2. The same version again, with the update runner's minimal environment.
    $runnerTemp = Join-Path $scratch 'runner-tmp'
    New-Item -ItemType Directory -Force $runnerTemp | Out-Null
    $code = Invoke-Setup -Path $Installer -Environment (Get-RunnerEnvironment -TempDir $runnerTemp)
    Test-Step '2: the installer runs with the minimal environment' ($code -eq 0) "exit $code"
    $installed = Test-NewVersionInstalled '2'

    # 3-5 are real upgrade attempts from the previous release: a replaced file then shows as the
    # new build ID instead of the previous one (D.9.3 "3 つのビルド ID が変わっていないこと").
    $code = Invoke-Setup -Path $PreviousInstaller
    Test-Step '3-5: the previous release goes back for the held-file runs' ($code -eq 0) "exit $code"
    $previous = Get-InstalledBuildIds -InstallDir $installDir
    Test-Step '3-5: the previous release is installed and in step' (Test-InStep $previous) ($previous -join ', ')
    if (-not (Test-DistinctRelease -Previous $previous -New $installed)) {
        # Same version and same hashed sources (installer.yml before the version is bumped): the
        # IDs cannot tell the two builds apart; the exit codes and the leftovers are still checked.
        Write-Host "WARN 3-5: the previous release has the new build's IDs ($($previous -join ', ')); 'nothing was replaced' cannot see a replaced file"
    }

    # 3. mklm-cli.exe held without sharing: the running check refuses (23).
    $held = [System.IO.File]::Open((Join-Path $installDir 'mklm-cli.exe'), 'Open', 'Read', 'None')
    try { $code = Invoke-Setup -Path $Installer } finally { $held.Dispose() }
    Test-Step '3: a held mklm-cli.exe is refused with 23' ($code -eq 23) "exit $code"
    $ids = Get-InstalledBuildIds -InstallDir $installDir
    Test-Step '3: nothing was replaced' (Test-NothingReplaced -Ids $ids -Kept $previous) ($ids -join ', ')

    # 4. mklm.exe held for reading, shared for reading and writing but not deleting: the running
    #    check passes, the rename fails, the swap is rolled back (26): the helper and the CLI, swapped
    #    already, must be the previous release's again.
    $held = [System.IO.File]::Open((Join-Path $installDir 'mklm.exe'), 'Open', 'Read', 'ReadWrite')
    try { $code = Invoke-Setup -Path $Installer } finally { $held.Dispose() }
    Test-Step '4: a mklm.exe held without delete sharing is refused with 26' ($code -eq 26) "exit $code"
    $ids = Get-InstalledBuildIds -InstallDir $installDir
    Test-Step '4: nothing was replaced' (Test-NothingReplaced -Ids $ids -Kept $previous) ($ids -join ', ')
    $leftovers = @(Get-ChildItem $installDir -File | Where-Object { $_.Name -match '\.(new|old)$' })
    Test-Step '4: no .new or .old is left' ($leftovers.Count -eq 0) (($leftovers | ForEach-Object Name) -join ', ')

    # 5. mklm.exe shared for reading only: the running check cannot open it (24).
    $held = [System.IO.File]::Open((Join-Path $installDir 'mklm.exe'), 'Open', 'Read', 'Read')
    try { $code = Invoke-Setup -Path $Installer } finally { $held.Dispose() }
    Test-Step '5: a mklm.exe shared for reading only is refused with 24' ($code -eq 24) "exit $code"
    $ids = Get-InstalledBuildIds -InstallDir $installDir
    Test-Step '5: nothing was replaced' (Test-NothingReplaced -Ids $ids -Kept $previous) ($ids -join ', ')

    # The upgrade goes through once nothing holds the files (and step 7 removes the new build).
    $code = Invoke-Setup -Path $Installer
    Test-Step '5: the new installer upgrades after the refused attempts' ($code -eq 0) "exit $code"
    $installed = Test-NewVersionInstalled '5'

    # 6. The ARM64 installer on x64 (21).
    if ($Arm64Installer) {
        if ($env:PROCESSOR_ARCHITECTURE -eq 'AMD64') {
            $code = Invoke-Setup -Path (Resolve-Path -LiteralPath $Arm64Installer).ProviderPath
            Test-Step '6: the ARM64 installer is refused on x64 with 21' ($code -eq 21) "exit $code"
        }
        else {
            Write-Host 'SKIP 6: not an x64 machine'
        }
    }
    else {
        Write-Host 'SKIP 6: no -Arm64Installer'
    }

    # 7. Uninstall, waited for (_?= keeps the uninstaller in place and returns its code).
    $uninstaller = Join-Path $installDir 'uninstall.exe'
    $code = Invoke-Setup -Path $uninstaller -Arguments @('/S', "_?=$installDir")
    Test-Step '7: the uninstaller ends with 0' ($code -eq 0) "exit $code"
    $remaining = @($script:Executables | Where-Object { Test-Path (Join-Path $installDir $_) })
    Test-Step '7: the executables are gone' ($remaining.Count -eq 0) ($remaining -join ', ')
    Remove-Item -Recurse -Force $installDir -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $scratch -ErrorAction SilentlyContinue

    if ($failures.Count -gt 0) {
        Write-Host "smoke-test: $($failures.Count) step(s) failed"
        exit 1
    }
    Write-Host 'smoke-test: every step passed (design m5b D.9.3)'
    exit 0
}
