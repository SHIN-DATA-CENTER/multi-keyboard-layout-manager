<#
.SYNOPSIS
  Builds the binaries for one architecture and the NSIS installer around them.

.DESCRIPTION
  -Profile release (the default): release binaries, dist\MKLM-Setup-<version>-<arch>.exe, and
  dist\SHA256SUMS for every installer in dist. The version comes from [workspace.package] in
  Cargo.toml. Unsigned for now: code signing, when it comes, signs the three exes before makensis
  and the installer after it (see SIGNING below). Built into target\<triple>\release even when
  CARGO_TARGET_DIR is set (that is where the exes are packed and checked from).

  -Profile dev (the update rehearsal, design m5b F.6): the three exes in the dev profile with the
  development update overrides (RUSTFLAGS=--cfg mklm_update_dev passed to this script's cargo only,
  in its own target folder target\dev-update), checked to carry the development marker (the
  positive control of design m5b A.10: fails without it), packed into
  dist-dev\<version>\MKLM-Setup-<version>-<arch>.exe with a SHA256SUMS of that folder only.
  MKLM_UPDATE_DEV_PUBKEY must be set (the rehearsal's development public key, built in with
  option_env!). Never a release: the executables are debug builds and say so (VS_FF_DEBUG).

.EXAMPLE
  .\installer\build-installer.ps1 -Arch x64
  .\installer\build-installer.ps1 -Arch arm64
  .\installer\build-installer.ps1 -Arch x64 -Profile dev
#>
[CmdletBinding()]
param(
    [ValidateSet('x64', 'arm64')] [string]$Arch = 'x64',
    [ValidateSet('release', 'dev')] [string]$Profile = 'release',
    [string]$Makensis,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$target = @{ x64 = 'x86_64-pc-windows-msvc'; arm64 = 'aarch64-pc-windows-msvc' }[$Arch]

$cargoToml = Get-Content -Raw (Join-Path $root 'Cargo.toml')
if ($cargoToml -notmatch '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') {
    throw 'No version in [workspace.package] of Cargo.toml.'
}
$version = $Matches[1]
# VIProductVersion needs four numbers: drop any pre-release or build suffix, append .0.
$viVersion = ($version -replace '[-+].*$', '') + '.0'

if (-not $Makensis) {
    $candidates = @(
        (Get-Command makensis.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source),
        (Join-Path ${env:ProgramFiles(x86)} 'NSIS\makensis.exe'),
        (Join-Path $env:ProgramFiles 'NSIS\makensis.exe')
    ) | Where-Object { $_ -and (Test-Path $_) }
    $Makensis = $candidates | Select-Object -First 1
}
if (-not $Makensis) { throw 'makensis.exe not found (install NSIS 3.12 or later, or pass -Makensis).' }
$nsisVersion = (& $Makensis -VERSION).Trim()
if ([version]($nsisVersion.TrimStart('v') -replace '[^0-9.].*$', '') -lt [version]'3.11') {
    throw "NSIS $nsisVersion is too old: 3.11 or later is required (CVE-2025-43715)."
}

$executables = 'mklm.exe', 'mklm-cli.exe', 'mklm-helper.exe'
if ($Profile -eq 'dev') {
    if (-not $env:MKLM_UPDATE_DEV_PUBKEY) {
        throw 'Set MKLM_UPDATE_DEV_PUBKEY to the base64 line of the rehearsal key (design m5b F.6 step 1) in this PowerShell only.'
    }
    $targetDir = Join-Path $root 'target\dev-update'
    $src = Join-Path $targetDir "$target\debug"
    if (-not $SkipBuild) {
        # Only this cargo gets the flag and the target folder; the caller's environment is restored.
        $saved = @{ RUSTFLAGS = $env:RUSTFLAGS; CARGO_TARGET_DIR = $env:CARGO_TARGET_DIR }
        try {
            $env:RUSTFLAGS = '--cfg mklm_update_dev'
            $env:CARGO_TARGET_DIR = $targetDir
            & cargo build --locked --target $target -p mklm -p mklm-cli -p mklm-helper
            if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)." }
        }
        finally {
            $env:RUSTFLAGS = $saved.RUSTFLAGS
            $env:CARGO_TARGET_DIR = $saved.CARGO_TARGET_DIR
        }
    }
    $dist = Join-Path $root "dist-dev\$version"
}
else {
    $targetDir = Join-Path $root 'target'
    $src = Join-Path $targetDir "$target\release"
    if (-not $SkipBuild) {
        # The exes are packed and checked from $src, so this cargo builds there whatever the
        # caller's CARGO_TARGET_DIR says (else the new exes land elsewhere and older ones under
        # target\ are packed and checked instead); the caller's environment is restored.
        $savedTargetDir = $env:CARGO_TARGET_DIR
        try {
            $env:CARGO_TARGET_DIR = $targetDir
            & cargo build --release --locked --target $target -p mklm -p mklm-cli -p mklm-helper
            if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)." }
        }
        finally {
            $env:CARGO_TARGET_DIR = $savedTargetDir
        }
    }
    $dist = Join-Path $root 'dist'
}
foreach ($exe in $executables) {
    if (-not (Test-Path (Join-Path $src $exe))) { throw "$exe is missing in $src." }
}

# Design m5b A.10: the development marker is in every development build (positive control) and in
# no release build.
$markerExpected = if ($Profile -eq 'dev') { 'Present' } else { 'Absent' }
$findMarker = Join-Path $PSScriptRoot 'find-marker.ps1'
foreach ($exe in $executables) {
    & powershell -NoProfile -ExecutionPolicy Bypass -File $findMarker -Expect $markerExpected -Path (Join-Path $src $exe)
    if ($LASTEXITCODE -ne 0) {
        throw "The development marker check of $exe failed (expected $($markerExpected.ToLowerInvariant()))."
    }
}

# The GUI's icon is generated by its build script into OUT_DIR; use the newest one.
$icon = Get-ChildItem (Join-Path $src 'build') -Filter mklm.ico -Recurse -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1

# SIGNING: sign mklm.exe, mklm-cli.exe and mklm-helper.exe here (before they are packed), and the
# installer after makensis. Not done yet: MKLM is unsigned for now (plan 5).

New-Item -ItemType Directory -Force $dist | Out-Null
$outFile = Join-Path $dist "MKLM-Setup-$version-$Arch.exe"
$defines = @(
    "/DVERSION=$version", "/DVIVERSION=$viVersion", "/DARCH=$Arch",
    "/DSRCDIR=$src", "/DROOT=$root", "/DOUTFILE=$outFile"
)
if ($icon) { $defines += "/DICON=$($icon.FullName)" }
& $Makensis /V2 /INPUTCHARSET UTF8 @defines (Join-Path $PSScriptRoot 'nsis\mklm.nsi')
if ($LASTEXITCODE -ne 0) { throw "makensis failed ($LASTEXITCODE)." }

$sums = Get-ChildItem $dist -Filter 'MKLM-Setup-*.exe' | Sort-Object Name | ForEach-Object {
    '{0}  {1}' -f (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $_.Name
}
[System.IO.File]::WriteAllLines((Join-Path $dist 'SHA256SUMS'), [string[]]$sums)
Write-Host "Built $outFile ($([math]::Round((Get-Item $outFile).Length / 1MB, 1)) MB, $Profile) with NSIS $nsisVersion"
