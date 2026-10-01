# Only the verified official minisign process sees keys and password input.
# This script is interactive: do not run it from a captured agent terminal.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
try {
    $repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
    $keyRoot = Join-Path $env:USERPROFILE 'MKLM-signing'
    $arch = if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() -eq 'Arm64') { 'aarch64' } else { 'x86_64' }
    $expectedHash = if ($arch -eq 'aarch64') { 'F39E065E649D5ED7075675ACCFE0ADA234175D63479DF650654EC4365D7C4513' } else { '5535BE9E4E123831EBE6EF324AAFE9DDE507015C176191F9E20C3AD60567F9E1' }
    $source = Join-Path $repo "target\release-tools\verified-0.12\minisign-win64\$arch\minisign.exe"
    if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -ne $expectedHash) { throw 'Verified minisign hash mismatch.' }
    if (Test-Path -LiteralPath $keyRoot) { throw "Refusing an existing key directory: $keyRoot. Do not delete existing keys; ask for a resume procedure." }
    Write-Host "Key destination: $keyRoot (outside the repository)."
    Write-Host 'Both keys will be on this PC. PC loss or compromise can affect both.'
    Write-Host 'Disconnect Wi-Fi / Ethernet and close MKLM, your editor, browser and coding-agent app.'
    Write-Host 'Keep ONLY this console open. Passwords are entered directly into minisign, not chat.'
    if ((Read-Host 'After completing those steps, type OFFLINE') -cne 'OFFLINE') { throw 'Cancelled; no keys created.' }
    if (Get-Process mklm,mklm-cli,mklm-helper -ErrorAction SilentlyContinue) { throw 'Close MKLM before starting.' }
    New-Item -ItemType Directory -Path $keyRoot | Out-Null
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    & "$env:SystemRoot\System32\icacls.exe" $keyRoot /inheritance:r /grant:r "*${sid}:(OI)(CI)F" '*S-1-5-18:(OI)(CI)F'
    if ($LASTEXITCODE -ne 0) { throw 'Could not restrict the key directory permissions.' }
    $tools = Join-Path $keyRoot 'tools'
    New-Item -ItemType Directory -Path $tools | Out-Null
    $signer = Join-Path $tools 'minisign.exe'
    Copy-Item -LiteralPath $source -Destination $signer
    if ((Get-FileHash -LiteralPath $signer -Algorithm SHA256).Hash -ne $expectedHash) { throw 'Copied minisign hash mismatch.' }
    foreach ($role in 'primary','backup') {
        $dir = Join-Path $keyRoot $role
        New-Item -ItemType Directory -Path $dir | Out-Null
        Write-Host "Generating $role key. Enter a strong password twice; use different passwords for the two keys."
        & $signer -G -p (Join-Path $dir "mklm-$role.pub") -s (Join-Path $dir "mklm-$role.key")
        if ($LASTEXITCODE -ne 0) { throw "minisign failed for $role. Preserve any generated keys; do not rerun blindly." }
    }
    $publicDir = Join-Path $repo 'target\release-keys'
    New-Item -ItemType Directory -Force -Path $publicDir | Out-Null
    foreach ($role in 'primary','backup') {
        # Copy PUBLIC keys only. Never copy/read the .key files.
        Copy-Item -LiteralPath (Join-Path $keyRoot "$role\mklm-$role.pub") -Destination (Join-Path $publicDir "mklm-$role.pub")
    }
    Write-Host 'Both keys generated. Keep passwords outside chat and make an independent secure backup.'
    Write-Host 'You may reconnect the network and tell the coding agent: keys created.'
} catch {
    Write-Host "Stopped: $($_.Exception.Message)" -ForegroundColor Red
}
[void](Read-Host 'Press Enter to close this console')
