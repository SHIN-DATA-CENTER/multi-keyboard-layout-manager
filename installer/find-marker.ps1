<#
.SYNOPSIS
  Checks executables for the development-update marker "MKLM-UPDATE-DEV-OVERRIDES!" (design m5b
  A.10): -Expect Absent for release builds (release.yml), -Expect Present for the positive control
  (the development-cfg builds of ci.yml and build-installer.ps1 -Profile dev).

.DESCRIPTION
  The marker is a #[used] static that the development-only update overrides reference
  (mklm_update::dev::DEV_MARKER, cfg(all(debug_assertions, mklm_update_dev))). A release executable
  that carries it was built with those overrides and must never ship. Because "absent" proves
  nothing when the marker would not survive the linker anyway, the same check must also find it in
  the development builds (FIX-VERIFICATION-3).

  The search is an exact byte match over the whole file; nothing is executed, so ARM64 executables
  are checked on an x64 machine as well.

  Dot-source the script to get Test-DevMarker without running it (installer\tests).

.EXAMPLE
  .\installer\find-marker.ps1 -Expect Absent -Path target\x86_64-pc-windows-msvc\release\mklm.exe
  .\installer\find-marker.ps1 -Expect Present -Path (Get-ChildItem target\dev-update\debug\mklm*.exe)
#>
[CmdletBinding()]
param(
    [string[]]$Path,
    [ValidateSet('Absent', 'Present')] [string]$Expect = 'Absent'
)

Set-StrictMode -Version 2.0

$script:DevMarker = 'MKLM-UPDATE-DEV-OVERRIDES!'

# True when the file's bytes contain the marker. ISO-8859-1 maps every byte to one character, so an
# ordinal substring search over the decoded text is an exact byte search.
function Test-DevMarker {
    param([Parameter(Mandatory)] [string]$File)
    $bytes = [System.IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $File).ProviderPath)
    $text = [System.Text.Encoding]::GetEncoding(28591).GetString($bytes)
    return $text.IndexOf($script:DevMarker, [System.StringComparison]::Ordinal) -ge 0
}

if ($MyInvocation.InvocationName -ne '.') {
    $ErrorActionPreference = 'Stop'
    if (-not $Path -or $Path.Count -eq 0) { throw 'Pass -Path with the executables to check.' }
    $failed = 0
    foreach ($file in $Path) {
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
            Write-Host "find-marker: $file does not exist"
            $failed++
            continue
        }
        $found = Test-DevMarker -File $file
        $ok = if ($Expect -eq 'Present') { $found } else { -not $found }
        $state = if ($found) { 'present' } else { 'absent' }
        Write-Host ("find-marker: {0}: marker {1} ({2})" -f $file, $state, $(if ($ok) { 'ok' } else { "expected $($Expect.ToLowerInvariant())" }))
        if (-not $ok) { $failed++ }
    }
    if ($failed -gt 0) {
        Write-Host "find-marker: $failed file(s) failed (expected the marker $($Expect.ToLowerInvariant()); design m5b A.10)"
        exit 1
    }
    exit 0
}
