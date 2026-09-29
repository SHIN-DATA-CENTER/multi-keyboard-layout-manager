<#
.SYNOPSIS
  The CI step of the installer checks (design m5b D.9.3, G.6; standard-layout D.4): the NSIS
  script rules (check-nsi.ps1), then the Pester tests of installer\tests. Exits 0 only when both
  pass, and always with an explicit exit code.

.DESCRIPTION
  ci.yml runs this file with `shell: powershell`, which wraps the step as
  `$ErrorActionPreference = 'stop'` before it and `exit $LASTEXITCODE` after it. Two things went
  wrong under that wrapper before this file existed (design standard-layout D.4):
  - a leftover $LASTEXITCODE of the last native command a test ran (a deliberate failure) became
    the step's result although every test passed; the explicit `exit` below decides it instead;
  - a test that redirected a child's stderr with `2>&1` stopped at the first stderr line
    (NativeCommandError). The tests no longer do that; see the two rules at the top of
    installer\tests\smoke-test.Tests.ps1.

  The failure check reads FailedCount and TotalCount (Pester 3.4 and 5), plus Pester 5's
  FailedContainersCount and FailedBlocksCount when the result has them, so that a test file that
  does not even load is a failure too.

  To reproduce CI locally (design standard-layout D.4 step 5; exit code 0 expected):
    powershell -NoProfile -ExecutionPolicy Bypass -Command "$ErrorActionPreference = 'stop'; . './installer/tests/Invoke-InstallerChecks.ps1'; if ((Test-Path -LiteralPath variable:\LASTEXITCODE)) { exit $LASTEXITCODE }"
#>
[CmdletBinding()]
param()

# The same condition as GitHub's wrapper, also when run by hand.
$ErrorActionPreference = 'Stop'

$installer = Split-Path -Parent $PSScriptRoot

& (Join-Path $installer 'check-nsi.ps1')
if ($LASTEXITCODE -ne 0) {
    Write-Host "check-nsi.ps1 failed ($LASTEXITCODE)"
    exit 1
}

$result = Invoke-Pester -Path $PSScriptRoot -PassThru
$problems = @()
if ($result.FailedCount -gt 0) { $problems += "$($result.FailedCount) test(s) failed" }
if ($result.TotalCount -eq 0) { $problems += 'no test ran' }
foreach ($name in 'FailedContainersCount', 'FailedBlocksCount') {
    if ($result.PSObject.Properties[$name] -and $result.$name -gt 0) {
        $problems += "$name = $($result.$name)"
    }
}
if ($problems.Count -gt 0) {
    Write-Host ('installer checks failed: ' + ($problems -join '; '))
    exit 1
}
Write-Host "installer checks passed: check-nsi.ps1 and $($result.TotalCount) Pester test(s)"
exit 0
