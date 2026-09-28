<#
.SYNOPSIS
  Before a release build (release.yml): nothing may add compiler flags or swap the compiler behind
  the build's back (design m5b A.10, guard 1; FIX-VERIFICATION-3).

.DESCRIPTION
  The development-only update overrides compile in only with --cfg mklm_update_dev, which only
  RUSTFLAGS-like inputs can pass. This check fails the build when any such input exists:

  - environment variables: RUSTFLAGS, CARGO_ENCODED_RUSTFLAGS, CARGO_BUILD_RUSTFLAGS,
    MKLM_UPDATE_DEV_PUBKEY, RUSTC, RUSTC_WRAPPER, RUSTC_WORKSPACE_WRAPPER, CARGO_BUILD_RUSTC,
    CARGO_BUILD_RUSTC_WRAPPER, CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER, every CARGO_PROFILE_*, and
    every CARGO_TARGET_*_RUSTFLAGS;
  - the repository's .cargo\config.toml with anything but an [alias] table (or a rustflags key),
    and a legacy .cargo\config next to it;
  - any other Cargo configuration file Cargo would read: %CARGO_HOME%\config.toml and
    %CARGO_HOME%\config (CARGO_HOME defaults to %USERPROFILE%\.cargo), and .cargo\config.toml or
    .cargo\config in every folder above the repository (Cargo reads them all, from the current
    folder up to the root, and merges them).

  The workflow commands themselves must not pass --config (checked by review, design m5b G.6).

  Dot-source the script to get Test-BuildEnvironment without running it (installer\tests).

.EXAMPLE
  .\installer\check-build-env.ps1
#>
[CmdletBinding()]
param(
    [string]$Root
)

Set-StrictMode -Version 2.0

$script:ForbiddenVariables = @(
    'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_RUSTFLAGS', 'MKLM_UPDATE_DEV_PUBKEY',
    'RUSTC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_RUSTC',
    'CARGO_BUILD_RUSTC_WRAPPER', 'CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER'
)

# The problems with the repository's own .cargo\config.toml: only [alias] is allowed, and no
# rustflags anywhere (not even as an alias's text would be harmless, but keep one simple rule).
function Test-RepositoryCargoConfig {
    param([Parameter(Mandatory)] [string]$Text)
    $problems = New-Object System.Collections.Generic.List[string]
    $number = 0
    foreach ($line in ($Text -split "`r?`n")) {
        $number++
        $code = ($line -replace '#.*$', '').Trim()
        if ($code.Length -eq 0) { continue }
        if ($code -match '^\[\[?\s*([^\]]+?)\s*\]\]?$') {
            $table = $Matches[1].Trim().Trim('"')
            if ($table -ne 'alias') {
                $problems.Add("line ${number}: table [$table] (only [alias] is allowed)")
            }
            continue
        }
        if ($code -match '(?i)rustflags|rustc') {
            $problems.Add("line ${number}: $code")
        }
    }
    return , $problems
}

# Every problem of the build environment: `Variables` maps names to values (the process
# environment by default), `Root` is the repository, `CargoHome` Cargo's home.
function Test-BuildEnvironment {
    param(
        [Parameter(Mandatory)] [hashtable]$Variables,
        [Parameter(Mandatory)] [string]$Root,
        [Parameter(Mandatory)] [string]$CargoHome
    )
    $problems = New-Object System.Collections.Generic.List[string]
    foreach ($name in $Variables.Keys) {
        $upper = $name.ToUpperInvariant()
        $forbidden = ($script:ForbiddenVariables -contains $upper) -or
        $upper.StartsWith('CARGO_PROFILE_') -or
        ($upper.StartsWith('CARGO_TARGET_') -and $upper.EndsWith('_RUSTFLAGS'))
        if ($forbidden) {
            $problems.Add("environment variable $name is set")
        }
    }
    $repositoryConfig = Join-Path $Root '.cargo\config.toml'
    if (Test-Path -LiteralPath $repositoryConfig -PathType Leaf) {
        foreach ($problem in (Test-RepositoryCargoConfig -Text ([System.IO.File]::ReadAllText($repositoryConfig)))) {
            $problems.Add("${repositoryConfig}: $problem")
        }
    }
    $legacy = Join-Path $Root '.cargo\config'
    if (Test-Path -LiteralPath $legacy) {
        $problems.Add("$legacy exists (Cargo reads it too)")
    }
    foreach ($name in 'config.toml', 'config') {
        $file = Join-Path $CargoHome $name
        if (Test-Path -LiteralPath $file) {
            $problems.Add("$file exists (Cargo's home configuration)")
        }
    }
    $folder = Split-Path -Parent ([System.IO.Path]::GetFullPath($Root))
    while ($folder) {
        foreach ($name in 'config.toml', 'config') {
            $file = Join-Path (Join-Path $folder '.cargo') $name
            if (Test-Path -LiteralPath $file) {
                $problems.Add("$file exists (a folder above the repository)")
            }
        }
        $parent = Split-Path -Parent $folder
        if (-not $parent -or $parent -eq $folder) { break }
        $folder = $parent
    }
    return , $problems
}

if ($MyInvocation.InvocationName -ne '.') {
    $ErrorActionPreference = 'Stop'
    if (-not $Root) { $Root = Split-Path -Parent $PSScriptRoot }
    $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
    $variables = @{}
    foreach ($item in Get-ChildItem Env:) { $variables[$item.Name] = $item.Value }
    $found = Test-BuildEnvironment -Variables $variables -Root $Root -CargoHome $cargoHome
    if ($found.Count -gt 0) {
        foreach ($problem in $found) { Write-Host "check-build-env: $problem" }
        Write-Host "check-build-env: $($found.Count) problem(s): a release build must not take extra compiler flags (design m5b A.10)"
        exit 1
    }
    Write-Host 'check-build-env: no extra compiler flags, wrappers or Cargo configuration (design m5b A.10)'
    exit 0
}
