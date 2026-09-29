# installer\check-build-env.ps1 (design m5b A.10 guard 1): environment variables and Cargo
# configuration files that could add --cfg mklm_update_dev to a release build.
# Pester 3.4 and 5: Describe / It and plain `throw` only. Everything runs on scratch folders and
# scratch hashtables; the process environment is never changed.
# CI runs these under GitHub's powershell wrapper: see the two rules at the top of
# smoke-test.Tests.ps1 (design standard-layout D.4).

Describe 'check-build-env.ps1' {

    It 'passes a clean environment and the repository alias file' {
        . (Join-Path $PSScriptRoot '..\check-build-env.ps1')
        $scratch = Join-Path ([IO.Path]::GetTempPath()) ('mklm-env-' + [guid]::NewGuid().ToString('n'))
        $root = Join-Path $scratch 'work\repo'
        $cargoHome = Join-Path $scratch 'cargo-home'
        New-Item -ItemType Directory -Force (Join-Path $root '.cargo'), $cargoHome | Out-Null
        try {
            [IO.File]::WriteAllText((Join-Path $root '.cargo\config.toml'), "# aliases only`n[alias]`nxtask = `"run --package xtask --locked --`"`n")
            $clean = @{ PATH = 'C:\Windows'; CARGO_HOME = $cargoHome; CARGO_TERM_COLOR = 'always'; RUSTUP_TOOLCHAIN = 'stable' }
            # Only what lies in the scratch folder is this test's: the folders above it belong to
            # the machine the test runs on.
            $all = Test-BuildEnvironment -Variables $clean -Root $root -CargoHome $cargoHome
            $found = @($all | Where-Object { $_ -like "*$scratch*" -or $_ -like 'environment variable*' })
            if ($found.Count -ne 0) { throw ($found -join '; ') }
        }
        finally {
            Remove-Item -Recurse -Force $scratch -ErrorAction SilentlyContinue
        }
    }

    It 'refuses every variable that adds flags or swaps the compiler' {
        . (Join-Path $PSScriptRoot '..\check-build-env.ps1')
        $scratch = Join-Path ([IO.Path]::GetTempPath()) ('mklm-env-' + [guid]::NewGuid().ToString('n'))
        $root = Join-Path $scratch 'repo'
        New-Item -ItemType Directory -Force $root | Out-Null
        try {
            foreach ($name in @(
                    'RUSTFLAGS', 'rustflags', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_RUSTFLAGS',
                    'MKLM_UPDATE_DEV_PUBKEY', 'RUSTC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER',
                    'CARGO_BUILD_RUSTC', 'CARGO_BUILD_RUSTC_WRAPPER', 'CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER',
                    'CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS', 'CARGO_PROFILE_RELEASE_OPT_LEVEL',
                    'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS', 'CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_RUSTFLAGS')) {
                $found = Test-BuildEnvironment -Variables @{ $name = '--cfg mklm_update_dev' } -Root $root -CargoHome (Join-Path $scratch 'none')
                if ($found.Count -ne 1) { throw "$name was not refused ($($found -join '; '))" }
            }
            # Similar names that add nothing are fine.
            $found = Test-BuildEnvironment -Variables @{ CARGO_TARGET_DIR = 'x'; RUSTDOCFLAGS_X = 'y'; RUSTUP_HOME = 'z' } -Root $root -CargoHome (Join-Path $scratch 'none')
            if ($found.Count -ne 0) { throw ($found -join '; ') }
        }
        finally {
            Remove-Item -Recurse -Force $scratch -ErrorAction SilentlyContinue
        }
    }

    It 'refuses Cargo configuration beyond the repository alias table' {
        . (Join-Path $PSScriptRoot '..\check-build-env.ps1')
        foreach ($text in @(
                "[alias]`nx = `"build`"`n[build]`nrustflags = [`"--cfg`", `"mklm_update_dev`"]`n",
                "[target.x86_64-pc-windows-msvc]`nrustflags = [`"--cfg`"]`n",
                "[profile.release]`ndebug-assertions = true`n",
                "[alias]`nb = `"rustc -- --cfg mklm_update_dev`"`n")) {
            $problems = Test-RepositoryCargoConfig -Text $text
            if ($problems.Count -eq 0) { throw "accepted: $text" }
        }
        $problems = Test-RepositoryCargoConfig -Text "# comment [build]`n[alias]`nxtask = `"run --package xtask --locked --`"`n"
        if ($problems.Count -ne 0) { throw ($problems -join '; ') }
    }

    It 'finds Cargo configuration in CARGO_HOME and in folders above the repository' {
        . (Join-Path $PSScriptRoot '..\check-build-env.ps1')
        $scratch = Join-Path ([IO.Path]::GetTempPath()) ('mklm-env-' + [guid]::NewGuid().ToString('n'))
        $root = Join-Path $scratch 'a\b\repo'
        $cargoHome = Join-Path $scratch 'cargo-home'
        New-Item -ItemType Directory -Force (Join-Path $root '.cargo'), $cargoHome, (Join-Path $scratch 'a\.cargo') | Out-Null
        try {
            [IO.File]::WriteAllText((Join-Path $scratch 'a\.cargo\config.toml'), "[build]`nrustflags = []`n")
            [IO.File]::WriteAllText((Join-Path $cargoHome 'config'), "[build]`n")
            [IO.File]::WriteAllText((Join-Path $root '.cargo\config'), "[alias]`n")
            $all = Test-BuildEnvironment -Variables @{} -Root $root -CargoHome $cargoHome
            $found = @($all | Where-Object { $_ -like "*$scratch*" })
            $text = $found -join "`n"
            foreach ($expected in @('a\.cargo\config.toml', 'cargo-home\config', 'repo\.cargo\config')) {
                if ($text.IndexOf($expected, [StringComparison]::OrdinalIgnoreCase) -lt 0) {
                    throw "missing $expected in: $text"
                }
            }
            if ($found.Count -ne 3) { throw $text }
        }
        finally {
            Remove-Item -Recurse -Force $scratch -ErrorAction SilentlyContinue
        }
    }
}
